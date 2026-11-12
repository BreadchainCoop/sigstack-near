//! Message receiver with polling.

use crate::client::SignalClient;
use crate::types::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::sleep;
use tokio_stream::Stream;
use tracing::{debug, error, info, warn};

/// Message receiver that polls all registered accounts for new messages.
pub struct MessageReceiver {
    client: SignalClient,
    poll_interval: Duration,
    /// How often to refresh the account list
    account_refresh_interval: Duration,
    /// Notified when a received envelope carries a group change (see `with_group_updates`).
    group_updates: Option<Arc<Notify>>,
}

impl MessageReceiver {
    /// Create a new message receiver.
    pub fn new(client: SignalClient, poll_interval: Duration) -> Self {
        Self::with_intervals(client, poll_interval, Duration::from_secs(300))
    }

    /// Create a receiver with custom poll and account-refresh intervals (useful in tests).
    pub fn with_intervals(
        client: SignalClient,
        poll_interval: Duration,
        account_refresh_interval: Duration,
    ) -> Self {
        Self {
            client,
            poll_interval,
            account_refresh_interval,
            group_updates: None,
        }
    }

    /// Notify `notify` whenever a received envelope carries a group change, such as an
    /// invite. Lets a pending-invite check run right away instead of on a short poll.
    pub fn with_group_updates(mut self, notify: Arc<Notify>) -> Self {
        self.group_updates = Some(notify);
        self
    }

    /// Start receiving messages from all registered accounts as an async stream.
    pub fn stream(self) -> impl Stream<Item = BotMessage> {
        async_stream::stream! {
            let mut accounts: Vec<String> = Vec::new();
            let mut last_account_refresh = std::time::Instant::now();

            loop {
                // Refresh account list periodically or on first run
                if accounts.is_empty()
                    || last_account_refresh.elapsed() >= self.account_refresh_interval
                {
                    match self.client.list_accounts().await {
                        Ok(new_accounts) => {
                            if new_accounts != accounts {
                                info!("Polling {} accounts: {:?}", new_accounts.len(), new_accounts);
                            }
                            accounts = new_accounts;
                            last_account_refresh = std::time::Instant::now();
                        }
                        Err(e) => {
                            error!("Failed to list accounts: {}", e);
                            if accounts.is_empty() {
                                // Can't proceed without accounts
                                sleep(Duration::from_secs(5)).await;
                                continue;
                            }
                            // Continue with cached accounts
                        }
                    }
                }

                if accounts.is_empty() {
                    warn!("No registered accounts found, waiting...");
                    sleep(Duration::from_secs(10)).await;
                    continue;
                }

                // Poll each account for messages
                for account in &accounts {
                    match self.client.receive(account).await {
                        Ok(messages) => {
                            for msg in messages {
                                if msg.is_group_update() {
                                    if let Some(notify) = &self.group_updates {
                                        debug!("Group update on {}; checking invites", account);
                                        notify.notify_one();
                                    }
                                }
                                if let Some(bot_msg) = BotMessage::from_incoming(&msg) {
                                    if bot_msg.is_voice_note() {
                                        if let Some(audio) = bot_msg.primary_audio_attachment() {
                                            debug!(
                                                "Voice note on {} from {} (attachment {}, {} bytes expected)",
                                                account,
                                                bot_msg.source,
                                                audio.id,
                                                audio.size.unwrap_or(0)
                                            );
                                        }
                                    } else {
                                        debug!(
                                            "Received on {}: '{}' from {}",
                                            account,
                                            &bot_msg.text[..bot_msg.text.len().min(50)],
                                            bot_msg.source
                                        );
                                    }
                                    yield bot_msg;
                                }
                            }
                        }
                        Err(e) => {
                            error!("Receive error for {}: {}", account, e);
                            // Continue to next account
                        }
                    }
                }

                sleep(self.poll_interval).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use tokio_stream::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn stream_yields_text_messages_from_receive() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/accounts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(["+15550001111"])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/receive/%2B15550001111"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "envelope": {
                    "source": "+15550002222",
                    "sourceNumber": "+15550002222",
                    "timestamp": 1000,
                    "dataMessage": {
                        "message": "hello receiver",
                        "timestamp": 1000,
                        "attachments": []
                    }
                },
                "account": "+15550001111"
            }])))
            .mount(&server)
            .await;

        let client = SignalClient::new(server.uri()).unwrap();
        let receiver = MessageReceiver::with_intervals(
            client,
            Duration::from_millis(50),
            Duration::from_secs(60),
        );
        let mut stream = Box::pin(receiver.stream());

        let msg = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("timed out waiting for message")
            .expect("stream ended");
        assert_eq!(msg.text, "hello receiver");
        assert_eq!(msg.source, "+15550002222");
        assert_eq!(msg.receiving_account, "+15550001111");
    }

    #[tokio::test]
    async fn stream_continues_after_receive_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/accounts"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!(["+15550001111", "+15550003333"])),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/receive/%2B15550001111"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/receive/%2B15550003333"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "envelope": {
                    "source": "+15550004444",
                    "sourceNumber": "+15550004444",
                    "timestamp": 2000,
                    "dataMessage": {
                        "message": "from second account",
                        "timestamp": 2000,
                        "attachments": []
                    }
                },
                "account": "+15550003333"
            }])))
            .mount(&server)
            .await;

        let client = SignalClient::new(server.uri()).unwrap();
        let receiver = MessageReceiver::with_intervals(
            client,
            Duration::from_millis(50),
            Duration::from_secs(60),
        );
        let mut stream = Box::pin(receiver.stream());
        let msg = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("timed out")
            .expect("message");
        assert_eq!(msg.text, "from second account");
        assert_eq!(msg.receiving_account, "+15550003333");
    }

    #[tokio::test]
    async fn stream_yields_voice_notes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/accounts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(["+15550001111"])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/receive/%2B15550001111"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "envelope": {
                    "source": "+15550002222",
                    "sourceNumber": "+15550002222",
                    "timestamp": 3000,
                    "dataMessage": {
                        "timestamp": 3000,
                        "attachments": [{
                            "contentType": "audio/aac",
                            "id": "voice-1",
                            "size": 1234
                        }]
                    }
                },
                "account": "+15550001111"
            }])))
            .mount(&server)
            .await;

        let client = SignalClient::new(server.uri()).unwrap();
        let receiver = MessageReceiver::with_intervals(
            client,
            Duration::from_millis(50),
            Duration::from_secs(60),
        );
        let mut stream = Box::pin(receiver.stream());
        let msg = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert!(msg.is_voice_note());
        assert_eq!(msg.primary_audio_attachment().unwrap().id, "voice-1");
    }

    #[tokio::test]
    async fn group_update_notifies_and_yields_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/accounts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(["+15550001111"])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/receive/%2B15550001111"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "envelope": {
                    "source": "+15550002222",
                    "timestamp": 4000,
                    "dataMessage": {
                        "timestamp": 4000,
                        "groupInfo": { "groupId": "internal==", "type": "UPDATE", "revision": 1 }
                    }
                },
                "account": "+15550001111"
            }])))
            .mount(&server)
            .await;

        let notify = Arc::new(Notify::new());
        let client = SignalClient::new(server.uri()).unwrap();
        let receiver = MessageReceiver::with_intervals(
            client,
            Duration::from_millis(50),
            Duration::from_secs(60),
        )
        .with_group_updates(notify.clone());
        let mut stream = Box::pin(receiver.stream());

        tokio::select! {
            _ = notify.notified() => {}
            msg = stream.next() => panic!("group update yielded a message: {msg:?}"),
            _ = tokio::time::sleep(Duration::from_secs(2)) => panic!("not notified"),
        }
        // The receive mock keeps returning the update; none of those reach the stream.
        let next = tokio::time::timeout(Duration::from_millis(300), stream.next()).await;
        assert!(next.is_err(), "group update yielded a message: {next:?}");
    }
}
