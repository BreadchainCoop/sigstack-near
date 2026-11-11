//! Match inbound messages to handlers and send replies.

use crate::bot_identity::BotIdentity;
use crate::commands::CommandHandler;
use crate::entitlement_gate::EntitlementGate;
use signal_bot_core::REPLY_BREAK;
use signal_client::{BotMessage, SignalClient, SignalError, TextMode};
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, error};

/// Signal clients hide a typing indicator after about 15 seconds, so it's refreshed sooner.
const TYPING_REFRESH: Duration = Duration::from_secs(10);

/// The bot's typing indicator while a command runs, so a slow reply (a TDX quote, a NEAR AI
/// call) doesn't look like the bot is dead. Only for `!` commands: every message in a group
/// reaches the default handler, and the bot shouldn't appear to type at each one.
struct Typing(Option<JoinHandle<()>>);

impl Typing {
    /// Turns the indicator on and keeps it on, all in the background: a Signal API call can
    /// take seconds, and the handler shouldn't wait for one before it starts.
    fn start(signal: &SignalClient, message: &BotMessage) -> Self {
        if !message.text.trim_start().starts_with('!') {
            return Self(None);
        }
        let (signal, message) = (signal.clone(), message.clone());
        Self(Some(tokio::spawn(async move {
            loop {
                if let Err(e) = signal.set_typing(&message, true).await {
                    debug!("Typing indicator failed: {e}");
                    return;
                }
                tokio::time::sleep(TYPING_REFRESH).await;
            }
        })))
    }

    /// Stops refreshing and clears the indicator in the background, so the next message
    /// isn't held up either.
    fn stop(self, signal: &SignalClient, message: &BotMessage) {
        if let Some(refresh) = self.0 {
            refresh.abort();
            let (signal, message) = (signal.clone(), message.clone());
            tokio::spawn(async move {
                if let Err(e) = signal.set_typing(&message, false).await {
                    debug!("Clearing typing indicator failed: {e}");
                }
            });
        }
    }
}

/// Result of attempting to dispatch one inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// A handler matched. `sent_reply` is false when the handler owns sending
    /// (`handles_own_reply`) or when the outbound send failed after logging.
    Matched {
        label: &'static str,
        sent_reply: bool,
        used_quote: bool,
    },
    /// No handler claimed the message.
    NoMatch,
    /// Entitlement gate denied before execute (reply may still have been sent).
    Denied {
        label: &'static str,
        sent_reply: bool,
    },
}

/// Sends a handler's reply: one Signal message per [`REPLY_BREAK`]-separated part, in
/// order, stopping at the first failed send. A reply without a break is sent as is.
async fn send_reply(
    signal: &SignalClient,
    message: &BotMessage,
    response: &str,
    quote: bool,
    mode: TextMode,
) -> Result<(), SignalError> {
    let parts: Vec<&str> = if response.contains(REPLY_BREAK) {
        response
            .split(REPLY_BREAK)
            .filter(|part| !part.trim().is_empty())
            .collect()
    } else {
        vec![response]
    };
    for part in parts {
        if quote {
            signal
                .reply_quoted_with_mode(message, part, None, mode)
                .await?;
        } else {
            signal.reply_with_mode(message, part, mode).await?;
        }
    }
    Ok(())
}

/// Note bot identity, find the first matching handler, execute, and reply when needed.
pub async fn dispatch_message(
    handlers: &[Box<dyn CommandHandler>],
    signal: &SignalClient,
    bot_identity: &BotIdentity,
    message: &BotMessage,
    entitlements: &EntitlementGate,
) -> DispatchOutcome {
    bot_identity.note_inbound(message);

    let handler = handlers.iter().find(|h| h.matches(message));

    let Some(handler) = handler else {
        if message.is_voice_note() || !message.text.trim().is_empty() {
            debug!(
                source = %message.source,
                is_group = message.is_group,
                voice = message.is_voice_note(),
                "No handler matched message"
            );
        }
        return DispatchOutcome::NoMatch;
    };

    let quote_reply = handler.reply_with_quote();
    let own_reply = handler.handles_own_reply();
    let label = handler.label();
    debug!(
        handler = label,
        source = %message.source,
        is_group = message.is_group,
        voice = message.is_voice_note(),
        has_quote = message.quote.is_some(),
        own_reply,
        quote_reply,
        "Dispatching to handler"
    );

    if let Err(deny) = entitlements.allow(message).await {
        let response = deny.message();
        debug!(handler = label, reason = ?deny, "Entitlement gate denied");
        // Never call execute (including handles_own_reply) so NEAR/voice do not run.
        let send_result = if quote_reply {
            signal.reply_quoted(message, response, None).await
        } else {
            signal.reply(message, response).await
        };
        if let Err(e) = &send_result {
            error!("Failed to send entitlement deny reply: {}", e);
        }
        return DispatchOutcome::Denied {
            label,
            sent_reply: send_result.is_ok(),
        };
    }

    let typing = Typing::start(signal, message);
    let outcome =
        execute_and_reply(handler.as_ref(), signal, message, quote_reply, own_reply).await;
    typing.stop(signal, message);
    outcome
}

/// Runs the handler and sends its reply (or the error fallback) unless it sends its own.
async fn execute_and_reply(
    handler: &dyn CommandHandler,
    signal: &SignalClient,
    message: &BotMessage,
    quote_reply: bool,
    own_reply: bool,
) -> DispatchOutcome {
    let label = handler.label();
    match handler.execute(message).await {
        Ok(response) => {
            if own_reply {
                return DispatchOutcome::Matched {
                    label,
                    sent_reply: false,
                    used_quote: quote_reply,
                };
            }
            let mode = if handler.styled_reply() {
                TextMode::Styled
            } else {
                TextMode::Normal
            };
            let send_result = send_reply(signal, message, &response, quote_reply, mode).await;
            if let Err(e) = send_result {
                error!("Failed to send reply: {}", e);
                return DispatchOutcome::Matched {
                    label,
                    sent_reply: false,
                    used_quote: quote_reply,
                };
            }
            DispatchOutcome::Matched {
                label,
                sent_reply: true,
                used_quote: quote_reply,
            }
        }
        Err(e) => {
            error!("Handler error: {}", e);
            if own_reply {
                return DispatchOutcome::Matched {
                    label,
                    sent_reply: false,
                    used_quote: quote_reply,
                };
            }
            let fallback = "Sorry, something went wrong.";
            let send_result = if quote_reply {
                signal.reply_quoted(message, fallback, None).await
            } else {
                signal.reply(message, fallback).await
            };
            let sent_reply = send_result.is_ok();
            if let Err(send_err) = send_result {
                error!("Failed to send error fallback: {}", send_err);
            }
            DispatchOutcome::Matched {
                label,
                sent_reply,
                used_quote: quote_reply,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entitlements_store::EntitlementsStore;
    use crate::error::{AppError, AppResult};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct StubHandler {
        trigger: &'static str,
        label: &'static str,
        quote: bool,
        own_reply: bool,
        fail: bool,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl CommandHandler for StubHandler {
        fn trigger(&self) -> Option<&str> {
            Some(self.trigger)
        }

        fn label(&self) -> &'static str {
            self.label
        }

        fn reply_with_quote(&self) -> bool {
            self.quote
        }

        fn handles_own_reply(&self) -> bool {
            self.own_reply
        }

        async fn execute(&self, _message: &BotMessage) -> AppResult<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(AppError::Config(anyhow::anyhow!("stub failure")))
            } else {
                Ok("stub-ok".into())
            }
        }
    }

    fn dm(text: &str) -> BotMessage {
        BotMessage {
            source: "+15550002222".into(),
            source_number: Some("+15550002222".into()),
            source_name: None,
            text: text.into(),
            timestamp: 1,
            message_timestamp: 42,
            is_group: false,
            group_id: None,
            group_name: None,
            receiving_account: "+15550001111".into(),
            attachments: vec![],
            quote: None,
        }
    }

    fn group(text: &str, source: &str, group_id: &str) -> BotMessage {
        let mut m = dm(text);
        m.source = source.into();
        m.source_number = Some(source.into());
        m.is_group = true;
        m.group_id = Some(group_id.into());
        m
    }

    /// Replies with two styled parts.
    struct TwoPartHandler;

    #[async_trait]
    impl CommandHandler for TwoPartHandler {
        fn trigger(&self) -> Option<&str> {
            Some("!verify")
        }

        fn styled_reply(&self) -> bool {
            true
        }

        async fn execute(&self, _message: &BotMessage) -> AppResult<String> {
            Ok(format!("**first**{REPLY_BREAK}second{REPLY_BREAK}  "))
        }
    }

    async fn mount_typing(server: &MockServer) {
        for verb in ["PUT", "DELETE"] {
            Mock::given(method(verb))
                .and(path("/v1/typing-indicator/%2B15550001111"))
                .respond_with(ResponseTemplate::new(204))
                .expect(1)
                .mount(server)
                .await;
        }
    }

    async fn received(server: &MockServer) -> Vec<(String, String)> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| (r.method.to_string(), r.url.path().to_string()))
            .collect()
    }

    fn open_gate() -> EntitlementGate {
        EntitlementGate {
            store: EntitlementsStore::new_in_memory(),
            enforce: false,
        }
    }

    async fn signal_mock() -> (MockServer, SignalClient) {
        let server = MockServer::start().await;
        let client = SignalClient::new(server.uri()).unwrap();
        (server, client)
    }

    #[tokio::test]
    async fn no_match_when_no_handler_claims_message() {
        let (_server, signal) = signal_mock().await;
        let identity = BotIdentity::new();
        let handlers: Vec<Box<dyn CommandHandler>> = vec![Box::new(StubHandler {
            trigger: "!help",
            label: "help",
            quote: false,
            own_reply: false,
            fail: false,
            calls: Arc::new(AtomicUsize::new(0)),
        })];

        let outcome =
            dispatch_message(&handlers, &signal, &identity, &dm("hello"), &open_gate()).await;
        assert_eq!(outcome, DispatchOutcome::NoMatch);
    }

    #[tokio::test]
    async fn plain_reply_sends_via_v2_send() {
        let (server, signal) = signal_mock().await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .and(body_partial_json(json!({
                "message": "stub-ok",
                "number": "+15550001111",
                "recipients": ["+15550002222"]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let calls = Arc::new(AtomicUsize::new(0));
        let handlers: Vec<Box<dyn CommandHandler>> = vec![Box::new(StubHandler {
            trigger: "!help",
            label: "help",
            quote: false,
            own_reply: false,
            fail: false,
            calls: calls.clone(),
        })];
        let identity = BotIdentity::new();

        let outcome =
            dispatch_message(&handlers, &signal, &identity, &dm("!help"), &open_gate()).await;
        assert_eq!(
            outcome,
            DispatchOutcome::Matched {
                label: "help",
                sent_reply: true,
                used_quote: false,
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(identity.is_bot_message(&BotMessage {
            source: "+15550001111".into(),
            source_number: Some("+15550001111".into()),
            source_name: None,
            text: String::new(),
            timestamp: 0,
            message_timestamp: 0,
            is_group: false,
            group_id: None,
            group_name: None,
            receiving_account: "+15550001111".into(),
            attachments: vec![],
            quote: None,
        }));
    }

    // Each part goes out as its own styled message, in order; blank parts are skipped.
    #[tokio::test]
    async fn styled_multi_part_reply_sends_each_part() {
        let (server, signal) = signal_mock().await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(2)
            .mount(&server)
            .await;

        let handlers: Vec<Box<dyn CommandHandler>> = vec![Box::new(TwoPartHandler)];
        let identity = BotIdentity::new();
        let outcome = dispatch_message(
            &handlers,
            &signal,
            &identity,
            &dm("!verify x"),
            &open_gate(),
        )
        .await;
        assert!(matches!(
            outcome,
            DispatchOutcome::Matched {
                sent_reply: true,
                ..
            }
        ));

        let bodies: Vec<serde_json::Value> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/v2/send")
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0]["message"], "**first**");
        assert_eq!(bodies[1]["message"], "second");
        assert!(bodies.iter().all(|b| b["text_mode"] == "styled"));
    }

    /// Polls the mock until `pred` holds for its requests, for up to a second.
    async fn eventually(server: &MockServer, pred: impl Fn(&[(String, String)]) -> bool) -> bool {
        for _ in 0..50 {
            if pred(&received(server).await) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    // The guard turns typing on, then clears it once stopped.
    #[tokio::test]
    async fn typing_guard_turns_on_then_clears() {
        let (server, signal) = signal_mock().await;
        mount_typing(&server).await;
        let message = dm("!verify x");

        let typing = Typing::start(&signal, &message);
        assert!(eventually(&server, |r| r.iter().any(|(m, _)| m == "PUT")).await);
        typing.stop(&signal, &message);
        assert!(eventually(&server, |r| r.iter().any(|(m, _)| m == "DELETE")).await);
    }

    // A slow Signal API (seconds per call in signal-cli's normal mode) must not delay the
    // reply: the typing call runs in the background.
    #[tokio::test]
    async fn slow_typing_does_not_delay_the_reply() {
        let (server, signal) = signal_mock().await;
        Mock::given(method("PUT"))
            .and(path("/v1/typing-indicator/%2B15550001111"))
            .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let handlers: Vec<Box<dyn CommandHandler>> = vec![Box::new(StubHandler {
            trigger: "!help",
            label: "help",
            quote: false,
            own_reply: false,
            fail: false,
            calls: Arc::new(AtomicUsize::new(0)),
        })];
        let identity = BotIdentity::new();
        let started = std::time::Instant::now();
        let outcome =
            dispatch_message(&handlers, &signal, &identity, &dm("!help"), &open_gate()).await;

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(
            outcome,
            DispatchOutcome::Matched {
                sent_reply: true,
                ..
            }
        ));
    }

    // Ordinary messages that reach a handler don't make the bot look like it's typing.
    #[tokio::test]
    async fn plain_messages_do_not_type() {
        let (server, signal) = signal_mock().await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;

        let handlers: Vec<Box<dyn CommandHandler>> = vec![Box::new(StubHandler {
            trigger: "hel",
            label: "chat",
            quote: false,
            own_reply: false,
            fail: false,
            calls: Arc::new(AtomicUsize::new(0)),
        })];
        let identity = BotIdentity::new();
        dispatch_message(&handlers, &signal, &identity, &dm("hello"), &open_gate()).await;

        let methods: Vec<String> = received(&server).await.into_iter().map(|r| r.0).collect();
        assert_eq!(methods, vec!["POST".to_string()]);
    }

    #[tokio::test]
    async fn quote_reply_includes_quote_fields() {
        let (server, signal) = signal_mock().await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .and(body_partial_json(json!({
                "message": "stub-ok",
                "quote_timestamp": 42,
                "quote_author": "+15550002222"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let handlers: Vec<Box<dyn CommandHandler>> = vec![Box::new(StubHandler {
            trigger: "!voice",
            label: "voice",
            quote: true,
            own_reply: false,
            fail: false,
            calls: Arc::new(AtomicUsize::new(0)),
        })];
        let identity = BotIdentity::new();

        let outcome =
            dispatch_message(&handlers, &signal, &identity, &dm("!voice"), &open_gate()).await;
        assert_eq!(
            outcome,
            DispatchOutcome::Matched {
                label: "voice",
                sent_reply: true,
                used_quote: true,
            }
        );
    }

    #[tokio::test]
    async fn own_reply_skips_outbound_send() {
        let (server, signal) = signal_mock().await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(0)
            .mount(&server)
            .await;

        let handlers: Vec<Box<dyn CommandHandler>> = vec![Box::new(StubHandler {
            trigger: "!me",
            label: "translate_me",
            quote: false,
            own_reply: true,
            fail: false,
            calls: Arc::new(AtomicUsize::new(0)),
        })];
        let identity = BotIdentity::new();

        let outcome =
            dispatch_message(&handlers, &signal, &identity, &dm("!me"), &open_gate()).await;
        assert_eq!(
            outcome,
            DispatchOutcome::Matched {
                label: "translate_me",
                sent_reply: false,
                used_quote: false,
            }
        );
    }

    #[tokio::test]
    async fn handler_error_sends_fallback_message() {
        let (server, signal) = signal_mock().await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .and(body_partial_json(json!({
                "message": "Sorry, something went wrong."
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let handlers: Vec<Box<dyn CommandHandler>> = vec![Box::new(StubHandler {
            trigger: "!boom",
            label: "boom",
            quote: false,
            own_reply: false,
            fail: true,
            calls: Arc::new(AtomicUsize::new(0)),
        })];
        let identity = BotIdentity::new();

        let outcome =
            dispatch_message(&handlers, &signal, &identity, &dm("!boom"), &open_gate()).await;
        assert_eq!(
            outcome,
            DispatchOutcome::Matched {
                label: "boom",
                sent_reply: true,
                used_quote: false,
            }
        );
    }

    #[tokio::test]
    async fn first_matching_handler_wins() {
        let (server, signal) = signal_mock().await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));
        let handlers: Vec<Box<dyn CommandHandler>> = vec![
            Box::new(StubHandler {
                trigger: "!x",
                label: "first",
                quote: false,
                own_reply: false,
                fail: false,
                calls: first.clone(),
            }),
            Box::new(StubHandler {
                trigger: "!x",
                label: "second",
                quote: false,
                own_reply: false,
                fail: false,
                calls: second.clone(),
            }),
        ];
        let identity = BotIdentity::new();

        let outcome =
            dispatch_message(&handlers, &signal, &identity, &dm("!x"), &open_gate()).await;
        assert_eq!(
            outcome,
            DispatchOutcome::Matched {
                label: "first",
                sent_reply: true,
                used_quote: false,
            }
        );
        assert_eq!(first.load(Ordering::SeqCst), 1);
        assert_eq!(second.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn enforce_denies_help_without_link_and_skips_execute() {
        let (server, signal) = signal_mock().await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .and(body_partial_json(json!({
                "message": crate::entitlement_gate::DENY_NEED_LINK
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let calls = Arc::new(AtomicUsize::new(0));
        let handlers: Vec<Box<dyn CommandHandler>> = vec![Box::new(StubHandler {
            trigger: "!help",
            label: "help",
            quote: false,
            own_reply: false,
            fail: false,
            calls: calls.clone(),
        })];
        let identity = BotIdentity::new();
        let gate = EntitlementGate {
            store: EntitlementsStore::new_in_memory(),
            enforce: true,
        };

        let outcome = dispatch_message(&handlers, &signal, &identity, &dm("!help"), &gate).await;
        assert_eq!(
            outcome,
            DispatchOutcome::Denied {
                label: "help",
                sent_reply: true,
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn enforce_allows_help_after_link_in_dm() {
        let (server, signal) = signal_mock().await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .and(body_partial_json(json!({ "message": "stub-ok" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let store = EntitlementsStore::new_in_memory();
        store.redeem_reusable_alpha("+15550002222".into()).unwrap();
        let gate = EntitlementGate {
            store,
            enforce: true,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let handlers: Vec<Box<dyn CommandHandler>> = vec![Box::new(StubHandler {
            trigger: "!help",
            label: "help",
            quote: false,
            own_reply: false,
            fail: false,
            calls: calls.clone(),
        })];
        let identity = BotIdentity::new();

        let outcome = dispatch_message(&handlers, &signal, &identity, &dm("!help"), &gate).await;
        assert_eq!(
            outcome,
            DispatchOutcome::Matched {
                label: "help",
                sent_reply: true,
                used_quote: false,
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn enforce_denies_group_help_until_enable_then_allows_member() {
        let (_server, signal) = signal_mock().await;
        // Group outbound uses group APIs; assert execute gating only (not send success).
        let store = EntitlementsStore::new_in_memory();
        store.redeem_reusable_alpha("uuid-ada".into()).unwrap();
        let gate = EntitlementGate {
            store: store.clone(),
            enforce: true,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let handlers: Vec<Box<dyn CommandHandler>> = vec![Box::new(StubHandler {
            trigger: "!help",
            label: "help",
            quote: false,
            own_reply: false,
            fail: false,
            calls: calls.clone(),
        })];
        let identity = BotIdentity::new();

        let denied = dispatch_message(
            &handlers,
            &signal,
            &identity,
            &group("!help", "uuid-bob", "g1"),
            &gate,
        )
        .await;
        assert!(matches!(denied, DispatchOutcome::Denied { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        store.enable_sigstack("uuid-ada", "g1".into()).unwrap();
        let allowed = dispatch_message(
            &handlers,
            &signal,
            &identity,
            &group("!help", "uuid-bob", "g1"),
            &gate,
        )
        .await;
        assert!(
            matches!(allowed, DispatchOutcome::Matched { label: "help", .. }),
            "{allowed:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn enforce_skips_own_reply_execute_when_denied() {
        let (server, signal) = signal_mock().await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let calls = Arc::new(AtomicUsize::new(0));
        let handlers: Vec<Box<dyn CommandHandler>> = vec![Box::new(StubHandler {
            trigger: "!me",
            label: "translate_me",
            quote: false,
            own_reply: true,
            fail: false,
            calls: calls.clone(),
        })];
        let identity = BotIdentity::new();
        let gate = EntitlementGate {
            store: EntitlementsStore::new_in_memory(),
            enforce: true,
        };

        let outcome = dispatch_message(&handlers, &signal, &identity, &dm("!me"), &gate).await;
        assert!(matches!(outcome, DispatchOutcome::Denied { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
