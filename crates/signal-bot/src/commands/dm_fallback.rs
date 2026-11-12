//! Direct-message fallback: answers DMs no other handler took, so a first-time user
//! (or a mistyped command) gets a pointer instead of silence. Groups stay silent.
//!
//! Register it last: dispatch runs the first handler that matches.

use crate::bot_identity::BotIdentity;
use crate::commands::CommandHandler;
use crate::error::AppResult;
use async_trait::async_trait;
use signal_client::BotMessage;
use std::sync::Arc;

/// Reply to plain text in a DM.
pub const DM_INTRO: &str = "Hi, I'm Sigstack. I translate and transcribe inside a TEE.

Send !help for commands, or !verify <text> to check that I'm running the approved code.";

/// Reply to a voice note in a DM without auto-transcribe.
pub const DM_VOICE_HINT: &str = "To transcribe this voice note, reply to it with !transcribe. \
Send !transcribe-on to transcribe every voice note in this chat.";

/// Longest command name echoed back in the unknown-command reply.
const MAX_ECHOED_COMMAND_CHARS: usize = 32;

pub struct DmFallbackHandler {
    bot_identity: Arc<BotIdentity>,
}

impl DmFallbackHandler {
    pub fn new(bot_identity: Arc<BotIdentity>) -> Self {
        Self { bot_identity }
    }

    /// The reply for `message`, which `matches` already accepted.
    pub fn reply_for(message: &BotMessage) -> String {
        if message.is_voice_note() && message.text.trim().is_empty() {
            return DM_VOICE_HINT.into();
        }
        let text = message.text.trim();
        if let Some(rest) = text.strip_prefix('!') {
            let name: String = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .chars()
                .take(MAX_ECHOED_COMMAND_CHARS)
                .collect();
            if !name.is_empty() {
                return format!(
                    "I don't know !{name}. Send !help for commands, or !verify <text> to check \
                     that I'm running the approved code."
                );
            }
        }
        DM_INTRO.into()
    }
}

#[async_trait]
impl CommandHandler for DmFallbackHandler {
    fn matches(&self, message: &BotMessage) -> bool {
        !message.is_group
            && !self.bot_identity.is_bot_message(message)
            && (message.is_voice_note() || !message.text.trim().is_empty())
    }

    fn label(&self) -> &'static str {
        "dm_fallback"
    }

    async fn execute(&self, message: &BotMessage) -> AppResult<String> {
        Ok(Self::reply_for(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signal_client::Attachment;

    fn msg(text: &str, is_group: bool, voice: bool) -> BotMessage {
        BotMessage {
            source: "+15550002222".into(),
            source_number: Some("+15550002222".into()),
            source_name: None,
            text: text.into(),
            timestamp: 1,
            message_timestamp: 1,
            is_group,
            group_id: is_group.then(|| "group.abc".into()),
            group_name: None,
            receiving_account: "+15550001111".into(),
            attachments: if voice {
                vec![Attachment {
                    content_type: "audio/aac".into(),
                    filename: None,
                    id: "att-1".into(),
                    size: Some(10),
                    upload_timestamp: None,
                }]
            } else {
                vec![]
            },
            quote: None,
        }
    }

    fn handler() -> DmFallbackHandler {
        DmFallbackHandler::new(BotIdentity::new())
    }

    #[test]
    fn matches_dms_only() {
        let h = handler();
        assert!(h.matches(&msg("hi", false, false)));
        assert!(h.matches(&msg("!verfy", false, false)));
        assert!(h.matches(&msg("", false, true)));
        assert!(!h.matches(&msg("hi", true, false)));
        assert!(!h.matches(&msg("", true, true)));
        assert!(!h.matches(&msg("   ", false, false)));
        assert_eq!(h.label(), "dm_fallback");
    }

    #[test]
    fn ignores_the_bots_own_messages() {
        let identity = BotIdentity::new();
        identity.remember_phone("+15550002222");
        let h = DmFallbackHandler::new(identity);
        assert!(!h.matches(&msg("hi", false, false)));
    }

    #[tokio::test]
    async fn plain_text_gets_the_intro() {
        let out = handler().execute(&msg("hello there", false, false)).await;
        let out = out.unwrap();
        assert_eq!(out, DM_INTRO);
        assert!(out.contains("!help"));
        assert!(out.contains("!verify <text>"));
    }

    #[test]
    fn unknown_command_is_named_and_bounded() {
        let out = DmFallbackHandler::reply_for(&msg("!verfy hello", false, false));
        assert!(out.starts_with("I don't know !verfy. Send !help"));

        let long = format!("!{}", "x".repeat(200));
        let out = DmFallbackHandler::reply_for(&msg(&long, false, false));
        assert!(out.contains(&format!("!{}.", "x".repeat(MAX_ECHOED_COMMAND_CHARS))));
        assert!(!out.contains(&"x".repeat(MAX_ECHOED_COMMAND_CHARS + 1)));

        // A lone "!" isn't a command name.
        assert_eq!(
            DmFallbackHandler::reply_for(&msg("!", false, false)),
            DM_INTRO
        );
    }

    #[test]
    fn voice_note_gets_the_transcribe_hint() {
        let out = DmFallbackHandler::reply_for(&msg("", false, true));
        assert_eq!(out, DM_VOICE_HINT);
        assert!(out.contains("!transcribe"));
        assert!(out.contains("!transcribe-on"));
    }
}
