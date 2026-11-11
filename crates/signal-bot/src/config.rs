//! Application configuration loaded from environment variables.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::time::Duration;

/// Application configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Signal configuration
    #[serde(default)]
    pub signal: SignalConfig,

    /// NEAR AI configuration (required: chat + Whisper STT)
    #[serde(default)]
    pub near_ai: Option<NearAiConfig>,

    /// Bot configuration
    #[serde(default)]
    pub bot: BotConfig,

    /// Dstack configuration
    #[serde(default)]
    pub dstack: DstackConfig,

    /// Whisper transcription configuration
    #[serde(default)]
    pub whisper: WhisperConfig,

    /// In-chat auto-translate (`!translate-all-on` / `!translate-me-on`) configuration
    #[serde(default)]
    pub translate_all: TranslateAllConfig,

    /// Encrypted persistence for per-group bot preferences
    #[serde(default)]
    pub group_preferences: GroupPreferencesConfig,

    /// Encrypted persistence for paid/alpha entitlements (separate from feature prefs)
    #[serde(default)]
    pub entitlements: EntitlementsConfig,

    /// Agent contract registration (`AGENT__*`); off unless `AGENT__CONTRACT_ID` is set
    #[serde(default)]
    pub agent: near_agent::AgentConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SignalConfig {
    /// Signal CLI REST API endpoint
    #[serde(default = "default_signal_service")]
    pub service_url: String,

    /// Poll interval for messages
    #[serde(default = "default_poll_interval", with = "humantime_serde")]
    pub poll_interval: Duration,

    /// This bot's Signal phone (ops/registration; identity still learned from inbound).
    #[serde(default)]
    pub phone_number: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NearAiConfig {
    /// NEAR AI API key
    pub api_key: String,

    /// API base URL
    #[serde(default = "default_near_ai_url")]
    pub base_url: String,

    /// Default model
    #[serde(default = "default_model")]
    pub model: String,

    /// Request timeout
    #[serde(default = "default_timeout", with = "humantime_serde")]
    pub timeout: Duration,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BotConfig {
    /// Signal username nickname (e.g. `sigstack`; Signal adds `.NN`).
    /// Empty string disables startup username ensure.
    #[serde(default = "default_signal_username_opt")]
    pub signal_username: Option<String>,

    /// GitHub repository URL
    #[serde(default)]
    pub github_repo: Option<String>,

    /// Log level
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DstackConfig {
    /// Dstack guest agent socket path
    #[serde(default = "default_dstack_socket")]
    pub socket_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WhisperConfig {
    /// Master switch for voice transcription
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// OpenAI-compatible STT base URL (NEAR AI `/v1`)
    #[serde(default = "default_whisper_service")]
    pub service_url: String,

    /// STT model (e.g. `openai/whisper-large-v3`)
    #[serde(default = "default_whisper_model")]
    pub model: String,

    /// Max time per transcribe request
    #[serde(default = "default_whisper_timeout", with = "humantime_serde")]
    pub timeout: Duration,

    /// Reject attachments larger than this (rough proxy for max voice length)
    #[serde(default = "default_whisper_max_attachment_bytes")]
    pub max_attachment_bytes: usize,

    /// Prefix line before transcript text in quote-replies
    #[serde(default = "default_whisper_reply_prefix")]
    pub reply_prefix: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TranslateAllConfig {
    /// Master switch for group auto-translate
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Max auto-translate replies per group per minute (NEAR AI protection)
    #[serde(default = "default_translate_all_max_per_minute")]
    pub max_messages_per_minute: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupPreferencesConfig {
    /// Persist group transcription + translate-all settings (TEE-encrypted)
    #[serde(default = "default_true")]
    pub persist: bool,

    /// Encrypted preferences file path (Docker volume in production)
    #[serde(default = "default_group_preferences_path")]
    pub storage_path: String,

    /// Previous dstack `compose_hash` values (comma-separated). Used only when
    /// DeriveKey is missing so AppInfo-encrypted `group_prefs.enc` still decrypts
    /// after a compose/image bump; the bot then re-saves with an app-id-only key.
    #[serde(default)]
    pub legacy_compose_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntitlementsConfig {
    /// Persist entitlements snapshot (TEE-encrypted)
    #[serde(default = "default_true")]
    pub persist: bool,

    /// Encrypted entitlements file path (same `/data` volume as group prefs)
    #[serde(default = "default_entitlements_path")]
    pub storage_path: String,

    /// When true, gate product commands on linked entitlements / enabled groups.
    /// Default false so local/`cargo test` stay open; set true in Phala prod.
    #[serde(default)]
    pub enforce: bool,

    /// Previous dstack `compose_hash` values (comma-separated). Used only when
    /// DeriveKey is missing so AppInfo-encrypted `entitlements.enc` still decrypts
    /// after a compose/image bump; the bot then re-saves with an app-id-only key.
    #[serde(default)]
    pub legacy_compose_hash: String,
}

// Default implementations
impl Default for SignalConfig {
    fn default() -> Self {
        Self {
            service_url: default_signal_service(),
            poll_interval: default_poll_interval(),
            phone_number: None,
        }
    }
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            signal_username: Some(default_signal_username()),
            github_repo: None,
            log_level: default_log_level(),
        }
    }
}

impl Default for DstackConfig {
    fn default() -> Self {
        Self {
            socket_path: default_dstack_socket(),
        }
    }
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            service_url: default_whisper_service(),
            model: default_whisper_model(),
            timeout: default_whisper_timeout(),
            max_attachment_bytes: default_whisper_max_attachment_bytes(),
            reply_prefix: default_whisper_reply_prefix(),
        }
    }
}

impl Default for TranslateAllConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            max_messages_per_minute: default_translate_all_max_per_minute(),
        }
    }
}

impl Default for GroupPreferencesConfig {
    fn default() -> Self {
        Self {
            persist: default_true(),
            storage_path: default_group_preferences_path(),
            legacy_compose_hash: String::new(),
        }
    }
}

impl GroupPreferencesConfig {
    pub fn legacy_compose_hashes(&self) -> Vec<String> {
        self.legacy_compose_hash
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }
}

impl Default for EntitlementsConfig {
    fn default() -> Self {
        Self {
            persist: default_true(),
            storage_path: default_entitlements_path(),
            enforce: false,
            legacy_compose_hash: String::new(),
        }
    }
}

impl EntitlementsConfig {
    pub fn legacy_compose_hashes(&self) -> Vec<String> {
        self.legacy_compose_hash
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }
}

fn default_signal_service() -> String {
    "http://signal-api:8080".into()
}

fn default_poll_interval() -> Duration {
    Duration::from_millis(200)
}

fn default_near_ai_url() -> String {
    "https://cloud-api.near.ai/v1".into()
}

fn default_model() -> String {
    "deepseek-ai/DeepSeek-V4-Flash".into()
}

fn default_timeout() -> Duration {
    Duration::from_secs(10)
}

fn default_log_level() -> String {
    "info".into()
}

fn default_signal_username() -> String {
    "sigstack".into()
}

fn default_signal_username_opt() -> Option<String> {
    Some(default_signal_username())
}

fn default_dstack_socket() -> String {
    "/var/run/dstack.sock".into()
}

fn default_true() -> bool {
    true
}

fn default_whisper_service() -> String {
    "https://cloud-api.near.ai/v1".into()
}

fn default_whisper_model() -> String {
    "openai/whisper-large-v3".into()
}

fn default_whisper_timeout() -> Duration {
    Duration::from_secs(120)
}

fn default_whisper_max_attachment_bytes() -> usize {
    // ~5 min voice at typical Signal bitrates
    10 * 1024 * 1024
}

fn default_whisper_reply_prefix() -> String {
    "📝 Transcript:".into()
}

fn default_translate_all_max_per_minute() -> u32 {
    30
}

fn default_group_preferences_path() -> String {
    "/data/group_prefs.enc".into()
}

fn default_entitlements_path() -> String {
    "/data/entitlements.enc".into()
}

impl Config {
    /// Load configuration from environment variables.
    pub fn load() -> Result<Self> {
        // Load .env file if present
        dotenvy::dotenv().ok();

        let config = config::Config::builder()
            .add_source(
                config::Environment::default()
                    .separator("__")
                    // Note: try_parsing(true) would parse +15551234567 as a positive number
                    // stripping the + prefix. Keep strings as strings.
                    .try_parsing(false),
            )
            .build()
            .context("Failed to build configuration")?;

        let cfg: Self = config
            .try_deserialize()
            .context("Failed to deserialize configuration")?;

        cfg.validate()?;
        Ok(cfg)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if !self.whisper.enabled {
            bail!("WHISPER__ENABLED=true is required");
        }
        let Some(near) = &self.near_ai else {
            bail!("NEAR_AI__API_KEY (and related NEAR_AI__* settings) is required");
        };
        if near.api_key.trim().is_empty() {
            bail!("NEAR_AI__API_KEY must be non-empty");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot_config() -> Config {
        Config {
            signal: SignalConfig::default(),
            near_ai: None,
            bot: BotConfig::default(),
            dstack: DstackConfig::default(),
            whisper: WhisperConfig::default(),
            translate_all: TranslateAllConfig::default(),
            group_preferences: GroupPreferencesConfig::default(),
            entitlements: EntitlementsConfig::default(),
            agent: Default::default(),
        }
    }

    fn with_near(mut config: Config, api_key: Option<&str>) -> Config {
        config.near_ai = api_key.map(|k| NearAiConfig {
            api_key: k.into(),
            base_url: default_near_ai_url(),
            model: default_model(),
            timeout: default_timeout(),
        });
        config
    }

    #[test]
    fn requires_whisper_and_near_ai() {
        assert!(with_near(bot_config(), Some("sk-test")).validate().is_ok());

        let mut no_whisper = with_near(bot_config(), Some("sk-test"));
        no_whisper.whisper.enabled = false;
        let err = no_whisper.validate().unwrap_err();
        assert!(err.to_string().contains("WHISPER__ENABLED"));

        let err = bot_config().validate().unwrap_err();
        assert!(err.to_string().contains("NEAR_AI__API_KEY"));

        let err = with_near(bot_config(), Some("   ")).validate().unwrap_err();
        assert!(err.to_string().contains("non-empty"));
    }

    #[test]
    fn section_defaults() {
        assert_eq!(
            SignalConfig::default().service_url,
            "http://signal-api:8080"
        );
        assert!(SignalConfig::default().phone_number.is_none());
        assert_eq!(WhisperConfig::default().model, "openai/whisper-large-v3");
        assert!(TranslateAllConfig::default().enabled);
        assert!(GroupPreferencesConfig::default().persist);
        assert!(GroupPreferencesConfig::default()
            .legacy_compose_hash
            .is_empty());
        assert!(EntitlementsConfig::default().persist);
        assert!(!EntitlementsConfig::default().enforce);
        assert_eq!(
            EntitlementsConfig::default().storage_path,
            "/data/entitlements.enc"
        );
    }

    #[test]
    fn legacy_compose_hashes_split_and_trim() {
        let cfg = GroupPreferencesConfig {
            persist: true,
            storage_path: "/data/group_prefs.enc".into(),
            legacy_compose_hash: " abc ,def, ".into(),
        };
        assert_eq!(
            cfg.legacy_compose_hashes(),
            vec!["abc".to_string(), "def".to_string()]
        );
        assert!(GroupPreferencesConfig::default()
            .legacy_compose_hashes()
            .is_empty());
    }
}
