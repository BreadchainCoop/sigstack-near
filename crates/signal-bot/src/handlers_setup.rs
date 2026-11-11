//! Build the unified command handler stack (one Signal number).

use crate::bot_identity::BotIdentity;
use crate::commands::*;
use crate::config::Config;
use crate::entitlements_store::EntitlementsStore;
use crate::error::AppResult;
use crate::group_preferences_store::GroupPreferencesStore;
use crate::transcribe_prefs::GroupTranscribePrefs;
use crate::transcript_fanout::SuiteTranscriptFanout;
use anyhow::Context;
use dstack_client::DstackClient;
use near_ai_client::verify::{BackgroundVerifier, ModelVerifier, VerifierConfig};
use near_ai_client::NearAiClient;
use signal_bot_voice::{build_voice_handlers, SharedTranscriptFanout};
use signal_client::SignalClient;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};
use whisper_client::WhisperClient;

/// Result of wiring the unified bot: handlers plus long-lived stores.
pub struct BuiltHandlers {
    pub handlers: Vec<Box<dyn CommandHandler>>,
    /// Encrypted entitlements store. Held for process lifetime; consumed by
    /// `!link` / `!enable-sigstack` and entitlement gating when enforce is on.
    pub entitlements: Arc<EntitlementsStore>,
}

/// Unified bot: Language Threads → voice → in-chat → hub menus → quote translate → verify/help.
///
/// `bot_username` is the live Signal handle when known (e.g. `sigstack.01`), else the
/// configured nickname — used in `!link` / `!enable-sigstack` organizer copy.
pub async fn build_handlers(
    config: &Config,
    signal: Arc<SignalClient>,
    dstack: Arc<DstackClient>,
    bot_identity: Arc<BotIdentity>,
    bot_username: String,
) -> AppResult<BuiltHandlers> {
    let near_cfg = config
        .near_ai
        .as_ref()
        .context("NEAR AI config missing after validation")?;

    let mut near_ai = NearAiClient::new(
        &near_cfg.api_key,
        &near_cfg.base_url,
        &near_cfg.model,
        near_cfg.timeout,
    )
    .context("Failed to create NEAR AI client")?;
    if near_cfg.verify_responses {
        let verifier = Arc::new(
            ModelVerifier::new(VerifierConfig::new(&near_cfg.base_url), &near_cfg.api_key)
                .context("Failed to create NEAR AI verifier")?,
        );
        // Attest both models up front; chat responses are then checked as they arrive.
        for model in [near_cfg.model.clone(), config.whisper.model.clone()] {
            let verifier = verifier.clone();
            tokio::spawn(async move { verifier.attest(&model).await });
        }
        info!("Verifying NEAR AI responses and model attestations");
        near_ai = near_ai
            .with_no_aliasing(true)
            .with_observer(Arc::new(BackgroundVerifier(verifier)));
    }
    let near_ai = Arc::new(near_ai);

    if near_ai.health_check().await {
        info!("NEAR AI healthy - Model: {}", near_cfg.model);
    } else {
        warn!("NEAR AI health check failed - will retry on requests");
    }

    let whisper = Arc::new(
        WhisperClient::new(
            &config.whisper.service_url,
            config.whisper.timeout,
            &near_cfg.api_key,
            &config.whisper.model,
        )
        .context("Failed to create Whisper client")?,
    );

    if whisper.health_check().await {
        info!(
            "NEAR Whisper healthy at {} (model={})",
            config.whisper.service_url, config.whisper.model
        );
    } else {
        warn!(
            "NEAR Whisper health check failed at {} — will retry on requests",
            config.whisper.service_url
        );
    }

    let group_prefs = GroupPreferencesStore::open(
        dstack.clone(),
        PathBuf::from(&config.group_preferences.storage_path),
        config.group_preferences.persist,
        config.translate_all.max_messages_per_minute,
        config.group_preferences.legacy_compose_hashes(),
    )
    .await;

    if config.group_preferences.persist {
        info!(
            "Group preferences persistence enabled: {}",
            config.group_preferences.storage_path
        );
    }

    let entitlements = EntitlementsStore::open(
        dstack.clone(),
        PathBuf::from(&config.entitlements.storage_path),
        config.entitlements.persist,
        config.entitlements.legacy_compose_hashes(),
    )
    .await;

    if config.entitlements.persist {
        info!(
            "Entitlements persistence enabled: {}",
            config.entitlements.storage_path
        );
    }

    let translate_me = TranslateMeHandler::new(
        group_prefs.clone(),
        near_ai.clone(),
        signal.clone(),
        bot_identity.clone(),
    );

    let translate_all = if config.translate_all.enabled {
        Some(
            TranslateAllHandler::with_prefix(
                group_prefs.clone(),
                near_ai.clone(),
                signal.clone(),
                DEFAULT_TRANSCRIPT_PREFIX,
            )
            .with_bot_identity(bot_identity.clone()),
        )
    } else {
        None
    };

    let fanout: SharedTranscriptFanout = Arc::new(SuiteTranscriptFanout {
        translate_all: translate_all.clone(),
        translate_me: translate_me.clone(),
    });

    let mut handlers: Vec<Box<dyn CommandHandler>> = Vec::new();

    handlers.push(Box::new(translate_me));
    info!(
        "Language Threads / Bilingual Threads enabled: !translate-me-thread / !leave / !enable-in-chat (max {}/min)",
        config.translate_all.max_messages_per_minute
    );

    handlers.extend(build_voice_handlers(
        whisper,
        signal.clone(),
        config.whisper.reply_prefix.clone(),
        config.whisper.max_attachment_bytes,
        Arc::new(GroupTranscribePrefs(group_prefs.clone())),
        Some(fanout),
    ));
    info!("Voice transcription enabled: !transcribe* / auto voice notes");

    if let Some(in_chat) = translate_all {
        handlers.push(Box::new(in_chat));
        info!(
            "In-chat translation enabled: !translate-all-on / !translate-me-on / !enable-threads"
        );
    }

    handlers.push(Box::new(TranslationMenuHandler::new(
        config.translate_all.enabled,
    )));
    handlers.push(Box::new(TranslationThreadsMenuHandler::new()));
    handlers.push(Box::new(TranslationInChatMenuHandler::new(
        config.translate_all.enabled,
    )));
    handlers.push(Box::new(HelpThreadsHandler::new()));
    handlers.push(Box::new(HelpInChatHandler::new()));
    handlers.push(Box::new(HelpTranscriptionHandler::new()));
    handlers.push(Box::new(TranscriptionMenuHandler::new()));
    handlers.push(Box::new(InChatMenuHandler::new(
        config.translate_all.enabled,
    )));

    handlers.push(Box::new(TranslateHandler::new(
        near_ai.clone(),
        signal.clone(),
        "📝 Transcript:",
    )));
    handlers.push(Box::new(TranslateLangsHandler::new()));
    handlers.push(Box::new(TranslateLangsInChatHandler::new()));
    handlers.push(Box::new(RenameHandler::new(
        group_prefs.clone(),
        signal.clone(),
    )));
    handlers.push(Box::new(BotUsernameHandler::new(
        signal.clone(),
        config.signal.phone_number.clone(),
        config
            .bot
            .signal_username
            .clone()
            .unwrap_or_else(|| "sigstack".into()),
    )));
    handlers.push(Box::new(LinkHandler::new(
        entitlements.clone(),
        bot_username,
    )));
    handlers.push(Box::new(CommandsHandler::new(group_prefs.clone())));
    handlers.push(Box::new(VerifyHandler::new(dstack.clone()).with_links(
        config.bot.github_repo.clone(),
        config.bot.verifier_url.clone(),
    )));
    handlers.push(Box::new(HelpHandler::new()));
    handlers.push(Box::new(InfoHandler::new(group_prefs)));
    handlers.push(Box::new(PrivacyHandler::new()));

    info!("Unified bot: hub menus + voice + in-chat + Language Threads");
    Ok(BuiltHandlers {
        handlers,
        entitlements,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        BotConfig, DstackConfig, EntitlementsConfig, GroupPreferencesConfig, NearAiConfig,
        SignalConfig, TranslateAllConfig, WhisperConfig,
    };
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn base_config() -> Config {
        Config {
            signal: SignalConfig {
                service_url: "http://127.0.0.1:9".into(),
                poll_interval: Duration::from_millis(50),
                phone_number: None,
            },
            near_ai: None,
            bot: BotConfig::default(),
            dstack: DstackConfig {
                socket_path: "/tmp/sigstack-bot-test-dstack.sock".into(),
            },
            whisper: WhisperConfig::default(),
            translate_all: TranslateAllConfig {
                enabled: true,
                max_messages_per_minute: 30,
            },
            group_preferences: GroupPreferencesConfig {
                persist: false,
                storage_path: "/tmp/sigstack-bot-test-prefs.enc".into(),
                ..Default::default()
            },
            entitlements: EntitlementsConfig {
                persist: false,
                storage_path: "/tmp/sigstack-bot-test-entitlements.enc".into(),
                ..Default::default()
            },
            agent: Default::default(),
        }
    }

    fn labels(handlers: &[Box<dyn CommandHandler>]) -> Vec<&'static str> {
        handlers.iter().map(|h| h.label()).collect()
    }

    async fn mock_near() -> MockServer {
        let near = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&near)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&near)
            .await;
        near
    }

    fn with_near(mut config: Config, near: &MockServer) -> Config {
        config.near_ai = Some(NearAiConfig {
            api_key: "test-key".into(),
            base_url: near.uri(),
            model: "test-model".into(),
            timeout: Duration::from_secs(5),
            verify_responses: false,
        });
        config.whisper.service_url = near.uri();
        config.whisper.enabled = true;
        config
    }

    #[tokio::test]
    async fn builds_with_response_verification() {
        let near = mock_near().await;
        let mut config = with_near(base_config(), &near);
        config.near_ai.as_mut().unwrap().verify_responses = true;

        let signal = Arc::new(SignalClient::new("http://127.0.0.1:9").unwrap());
        let dstack = Arc::new(DstackClient::new(&config.dstack.socket_path));
        let built = build_handlers(
            &config,
            signal,
            dstack,
            BotIdentity::new(),
            "sigstack.test".into(),
        )
        .await
        .expect("handlers with verification");
        assert_eq!(built.handlers.len(), 24);
    }

    #[tokio::test]
    async fn registers_unified_handlers_with_translate_all() {
        let near = mock_near().await;
        let config = with_near(base_config(), &near);

        let signal = Arc::new(SignalClient::new("http://127.0.0.1:9").unwrap());
        let dstack = Arc::new(DstackClient::new(&config.dstack.socket_path));
        let identity = BotIdentity::new();

        let built = build_handlers(&config, signal, dstack, identity, "sigstack.test".into())
            .await
            .expect("translation handlers");
        let handlers = built.handlers;

        assert_eq!(handlers.len(), 24);
        let got = labels(&handlers);
        assert!(got.contains(&"translate_me"));
        assert!(got.contains(&"voice"));
        assert!(got.contains(&"manual_transcribe"));
        assert!(got.contains(&"transcribe"));
        assert!(got.contains(&"translate_all"));
        assert!(got.contains(&"translation_menu"));
        assert!(got.contains(&"translation_threads_menu"));
        assert!(got.contains(&"translation_in_chat_menu"));
        assert!(got.contains(&"help_threads"));
        assert!(got.contains(&"help_in_chat"));
        assert!(got.contains(&"help_transcription"));
        assert!(got.contains(&"transcription_menu"));
        assert!(!got.contains(&"transcription_pairing"));
        assert!(got.contains(&"in_chat_menu"));
        assert!(!got.contains(&"translate_parallel"));
        assert!(!got.contains(&"parallel_menu"));
        assert!(!got.contains(&"models"));
        assert!(got.contains(&"translate"));
        assert!(got.contains(&"translate_langs"));
        assert!(got.contains(&"translate_langs_in_chat"));
        assert!(got.contains(&"rename"));
        assert!(got.contains(&"bot_username"));
        assert!(got.contains(&"link"));
        assert!(got.contains(&"commands"));
        assert!(!got.contains(&"set_language"));
        assert!(got.contains(&"help"));
        assert!(got.contains(&"info"));
        assert!(got.contains(&"privacy"));
        assert_eq!(got[0], "translate_me");
        assert_eq!(&got[1..4], &["voice", "manual_transcribe", "transcribe"]);
        assert_eq!(got[4], "translate_all");
    }

    #[tokio::test]
    async fn translation_omits_translate_all_when_disabled() {
        let near = mock_near().await;
        let mut config = with_near(base_config(), &near);
        config.translate_all.enabled = false;

        let signal = Arc::new(SignalClient::new("http://127.0.0.1:9").unwrap());
        let dstack = Arc::new(DstackClient::new(&config.dstack.socket_path));
        let identity = BotIdentity::new();

        let built = build_handlers(&config, signal, dstack, identity, "sigstack.test".into())
            .await
            .expect("translation handlers");
        let handlers = built.handlers;

        assert_eq!(handlers.len(), 23);
        let got = labels(&handlers);
        assert!(!got.contains(&"translate_all"));
        assert!(got.contains(&"translate_me"));
        assert!(got.contains(&"voice"));
        assert!(got.contains(&"manual_transcribe"));
        assert!(got.contains(&"transcribe"));
        assert!(got.contains(&"transcription_menu"));
        assert!(!got.contains(&"transcription_pairing"));
        assert!(got.contains(&"in_chat_menu"));
        assert!(got.contains(&"translation_threads_menu"));
        assert!(got.contains(&"help_threads"));
        assert!(got.contains(&"help_in_chat"));
        assert!(got.contains(&"help_transcription"));
        assert!(got.contains(&"bot_username"));
        assert!(got.contains(&"info"));
    }

    #[tokio::test]
    async fn translation_requires_near_ai_config() {
        let config = base_config();
        let signal = Arc::new(SignalClient::new("http://127.0.0.1:9").unwrap());
        let dstack = Arc::new(DstackClient::new(&config.dstack.socket_path));
        let identity = BotIdentity::new();

        let result =
            build_handlers(&config, signal, dstack, identity, "sigstack.test".into()).await;
        assert!(result.is_err(), "missing NEAR AI should fail");
        assert!(
            result.err().unwrap().to_string().contains("NEAR AI"),
            "error should mention NEAR AI"
        );
    }
}
