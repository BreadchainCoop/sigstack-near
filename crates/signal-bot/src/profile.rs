//! Set the bot's Signal profile (display name, about line, picture) at startup.
//!
//! Without a profile name, people who haven't saved the number see only a number or
//! username. The picture is compiled into the binary, so the attested image digest
//! covers it like the rest of the bot.

use crate::config::BotConfig;
use signal_client::SignalClient;
use std::time::Duration;
use tracing::{info, warn};

/// The bot's profile picture: a 512×512 JPEG, metadata stripped.
pub const AVATAR: &[u8] = include_bytes!("../assets/avatar.jpg");

/// How many times to try setting the profile at startup, and how long to wait between tries.
pub const PROFILE_ATTEMPTS: u32 = 5;
pub const PROFILE_RETRY_DELAY: Duration = Duration::from_secs(30);

/// A profile to set, from `BOT__PROFILE_*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub about: Option<String>,
    pub avatar: Option<&'static [u8]>,
}

impl Profile {
    /// The configured profile, or `None` when `BOT__PROFILE_NAME` is unset or blank, in
    /// which case the bot leaves its Signal profile as it is.
    pub fn from_config(bot: &BotConfig) -> Option<Self> {
        let name = non_blank(bot.profile_name.as_deref())?;
        Some(Self {
            name,
            about: non_blank(bot.profile_about.as_deref()),
            avatar: bot.profile_avatar.then_some(AVATAR),
        })
    }
}

fn non_blank(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// Sets the profile, retrying while signal-api starts up. Non-fatal: returns whether it
/// succeeded, and the bot keeps running either way. Signal keeps the profile, so an
/// earlier boot's profile stays in place if this fails.
pub async fn ensure_profile(
    signal: &SignalClient,
    phone_number: &str,
    profile: &Profile,
    attempts: u32,
    retry_delay: Duration,
) -> bool {
    for attempt in 1..=attempts {
        match signal
            .set_profile(
                phone_number,
                &profile.name,
                profile.about.as_deref(),
                profile.avatar,
            )
            .await
        {
            Ok(()) => {
                info!(
                    name = %profile.name,
                    avatar = profile.avatar.is_some(),
                    "Signal profile set"
                );
                return true;
            }
            Err(e) => {
                warn!(attempt, attempts, "Setting the Signal profile failed: {e}");
                if attempt < attempts {
                    tokio::time::sleep(retry_delay).await;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn bot(name: Option<&str>, about: Option<&str>, avatar: bool) -> BotConfig {
        BotConfig {
            profile_name: name.map(str::to_owned),
            profile_about: about.map(str::to_owned),
            profile_avatar: avatar,
            ..BotConfig::default()
        }
    }

    #[test]
    fn avatar_is_a_jpeg() {
        assert_eq!(&AVATAR[..3], &[0xFF, 0xD8, 0xFF]);
        assert!(AVATAR.len() < 200_000);
    }

    #[test]
    fn profile_from_config() {
        assert_eq!(Profile::from_config(&bot(None, Some("x"), true)), None);
        assert_eq!(Profile::from_config(&bot(Some("  "), None, true)), None);

        let p = Profile::from_config(&bot(Some(" Sigstack "), Some(" "), false)).unwrap();
        assert_eq!(p.name, "Sigstack");
        assert_eq!(p.about, None);
        assert_eq!(p.avatar, None);

        let p = Profile::from_config(&bot(Some("Sigstack"), Some("About"), true)).unwrap();
        assert_eq!(p.about.as_deref(), Some("About"));
        assert_eq!(p.avatar, Some(AVATAR));
    }

    fn profile() -> Profile {
        Profile {
            name: "Sigstack".into(),
            about: Some("About".into()),
            avatar: Some(b"abc"),
        }
    }

    #[tokio::test]
    async fn ensure_profile_retries_until_signal_api_accepts() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/v1/profiles/%2B15550001111"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/v1/profiles/%2B15550001111"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let signal = SignalClient::new(server.uri()).unwrap();
        let ok = ensure_profile(
            &signal,
            "+15550001111",
            &profile(),
            3,
            Duration::from_millis(1),
        )
        .await;
        assert!(ok);
    }

    #[tokio::test]
    async fn ensure_profile_gives_up_after_its_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/v1/profiles/%2B15550001111"))
            .respond_with(ResponseTemplate::new(500))
            .expect(2)
            .mount(&server)
            .await;

        let signal = SignalClient::new(server.uri()).unwrap();
        let ok = ensure_profile(
            &signal,
            "+15550001111",
            &profile(),
            2,
            Duration::from_millis(1),
        )
        .await;
        assert!(!ok);
    }
}
