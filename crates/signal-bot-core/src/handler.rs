//! Shared command handler trait.

use crate::error::AppResult;
use async_trait::async_trait;
use signal_client::BotMessage;

/// Separates messages in a handler's reply: dispatch sends each part as its own Signal
/// message, in order. For example, `!verify` sends its quote alone so it copies in one piece.
pub const REPLY_BREAK: char = '\u{1e}';

/// Command handler trait.
#[async_trait]
pub trait CommandHandler: Send + Sync {
    /// Command trigger (e.g., "!help").
    fn trigger(&self) -> Option<&str> {
        None
    }

    /// Whether this is the default handler for non-command messages.
    fn is_default(&self) -> bool {
        false
    }

    /// Check if this handler matches the message.
    fn matches(&self, message: &BotMessage) -> bool {
        if let Some(trigger) = self.trigger() {
            message.text.starts_with(trigger)
        } else {
            self.is_default() && !message.text.starts_with('!') && !message.is_voice_note()
        }
    }

    /// Sees every incoming message before dispatch picks a handler, whichever handler (if
    /// any) then runs it. For bookkeeping a later command needs, such as remembering voice
    /// notes so `!transcribe` can find the one it replies to. Must be cheap and not block.
    fn observe(&self, _message: &BotMessage) {}

    /// Execute the command.
    async fn execute(&self, message: &BotMessage) -> AppResult<String>;

    /// When true, bot replies with a Signal quote-reply to the source message.
    fn reply_with_quote(&self) -> bool {
        false
    }

    /// When true, the handler sends its own Signal reply in `execute` (main loop skips send).
    fn handles_own_reply(&self) -> bool {
        false
    }

    /// When true, the reply is sent with Signal text styles: `**bold**`, `*italic*` and
    /// `` `monospace` ``. Only for replies the bot writes itself; user or model text could
    /// contain the markers.
    fn styled_reply(&self) -> bool {
        false
    }

    /// Short name for dispatch / debug logs.
    fn label(&self) -> &'static str {
        if self.is_default() {
            "default"
        } else {
            "command"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stub {
        trigger: Option<&'static str>,
        default: bool,
    }

    #[async_trait]
    impl CommandHandler for Stub {
        fn trigger(&self) -> Option<&str> {
            self.trigger
        }

        fn is_default(&self) -> bool {
            self.default
        }

        async fn execute(&self, _message: &BotMessage) -> AppResult<String> {
            Ok("ok".into())
        }
    }

    fn msg(text: &str, voice: bool) -> BotMessage {
        use signal_client::Attachment;
        BotMessage {
            source: "+1".into(),
            source_number: None,
            source_name: None,
            text: text.into(),
            timestamp: 0,
            message_timestamp: 0,
            is_group: false,
            group_id: None,
            group_name: None,
            receiving_account: "+2".into(),
            attachments: if voice {
                vec![Attachment {
                    content_type: "audio/aac".into(),
                    filename: None,
                    id: "a1".into(),
                    size: Some(10),
                    upload_timestamp: None,
                }]
            } else {
                vec![]
            },
            quote: None,
        }
    }

    #[test]
    fn default_matches_uses_trigger_prefix() {
        let h = Stub {
            trigger: Some("!help"),
            default: false,
        };
        assert!(h.matches(&msg("!help please", false)));
        assert!(!h.matches(&msg("help", false)));
        assert_eq!(h.label(), "command");
        assert!(!h.reply_with_quote());
        assert!(!h.handles_own_reply());
        assert!(!h.styled_reply());
        h.observe(&msg("!help", false));
    }

    #[test]
    fn default_handler_skips_commands_and_voice() {
        let h = Stub {
            trigger: None,
            default: true,
        };
        assert!(h.matches(&msg("hello", false)));
        assert!(!h.matches(&msg("!help", false)));
        assert!(!h.matches(&msg("", true)));
        assert_eq!(h.label(), "default");
    }
}
