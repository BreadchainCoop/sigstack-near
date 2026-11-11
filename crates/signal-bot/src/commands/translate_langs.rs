//! `!list-langs` and `!list-langs-in-chat` — supported translation languages.

use crate::commands::translate_lang::{
    format_language_list, in_chat_auto_languages, ALL_LANGUAGES,
};
use crate::commands::CommandHandler;
use crate::error::AppResult;
use async_trait::async_trait;
use signal_bot_core::starts_with_word_any;
use signal_client::BotMessage;

pub(crate) const LIST_LANGS_COMMANDS: &[&str] = &[
    "!list-langs",
    "!list-lang",
    "!list-languages",
    "!list-language",
];

pub(crate) const LIST_LANGS_IN_CHAT_COMMANDS: &[&str] = &[
    "!list-langs-in-chat",
    "!list-langs-auto",
    "!list-lang-in-chat",
    "!list-languages-in-chat",
];

pub(crate) fn is_list_langs_command(text: &str) -> bool {
    starts_with_word_any(text, LIST_LANGS_COMMANDS)
}

pub(crate) fn is_list_langs_in_chat_command(text: &str) -> bool {
    starts_with_word_any(text, LIST_LANGS_IN_CHAT_COMMANDS)
}

pub struct TranslateLangsHandler;

impl TranslateLangsHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TranslateLangsHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandHandler for TranslateLangsHandler {
    fn trigger(&self) -> Option<&str> {
        Some("!list-langs")
    }

    fn matches(&self, message: &BotMessage) -> bool {
        is_list_langs_command(&message.text)
    }

    fn label(&self) -> &'static str {
        "translate_langs"
    }

    fn styled_reply(&self) -> bool {
        true
    }

    async fn execute(&self, _message: &BotMessage) -> AppResult<String> {
        Ok(format!(
            "**Language Threads & manual translate** (use code with !translate-me-thread or quote !translate):\n\n{}\n\n\
             In-chat auto-translate uses a smaller list: !list-langs-in-chat",
            format_language_list(ALL_LANGUAGES)
        ))
    }
}

pub struct TranslateLangsInChatHandler;

impl TranslateLangsInChatHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TranslateLangsInChatHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandHandler for TranslateLangsInChatHandler {
    fn trigger(&self) -> Option<&str> {
        Some("!list-langs-in-chat")
    }

    fn matches(&self, message: &BotMessage) -> bool {
        is_list_langs_in_chat_command(&message.text)
    }

    fn label(&self) -> &'static str {
        "translate_langs_in_chat"
    }

    fn styled_reply(&self) -> bool {
        true
    }

    async fn execute(&self, _message: &BotMessage) -> AppResult<String> {
        let langs: Vec<_> = in_chat_auto_languages().copied().collect();
        Ok(format!(
            "**In-chat auto-translate** (use code with !translate-all-on or !translate-me-on):\n\n{}\n\n\
             Language Threads and quote !translate support more languages: !list-langs",
            format_language_list(&langs)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_langs_exact_not_common_suffix() {
        let h = TranslateLangsHandler::new();
        let mut msg = BotMessage {
            source: "+1".into(),
            source_number: None,
            source_name: None,
            text: "!list-langs".into(),
            timestamp: 0,
            message_timestamp: 0,
            is_group: true,
            group_id: Some("g".into()),
            group_name: None,
            receiving_account: "+2".into(),
            attachments: vec![],
            quote: None,
        };
        assert!(h.matches(&msg));
        msg.text = "!list-lang".into();
        assert!(h.matches(&msg));
        msg.text = "!list-languages".into();
        assert!(h.matches(&msg));
        msg.text = "!list-language".into();
        assert!(h.matches(&msg));
        msg.text = "!List-Langs".into();
        assert!(h.matches(&msg));
        msg.text = "!list-langs-common".into();
        assert!(!h.matches(&msg));
    }

    #[test]
    fn list_langs_in_chat_matches_aliases() {
        let h = TranslateLangsInChatHandler::new();
        let mut msg = BotMessage {
            source: "+1".into(),
            source_number: None,
            source_name: None,
            text: "!list-langs-in-chat".into(),
            timestamp: 0,
            message_timestamp: 0,
            is_group: true,
            group_id: Some("g".into()),
            group_name: None,
            receiving_account: "+2".into(),
            attachments: vec![],
            quote: None,
        };
        assert!(h.matches(&msg));
        msg.text = "!list-langs-auto".into();
        assert!(h.matches(&msg));
        msg.text = "!list-lang-in-chat".into();
        assert!(h.matches(&msg));
    }

    #[tokio::test]
    async fn execute_lists_supported_languages() {
        let h = TranslateLangsHandler::new();
        let msg = BotMessage {
            source: "+1".into(),
            source_number: None,
            source_name: None,
            text: "!list-langs".into(),
            timestamp: 0,
            message_timestamp: 0,
            is_group: true,
            group_id: Some("g".into()),
            group_name: None,
            receiving_account: "+2".into(),
            attachments: vec![],
            quote: None,
        };
        let out = h.execute(&msg).await.unwrap();
        assert!(out.contains("Language Threads"));
        assert!(out.contains("!translate-me-thread"));
        assert!(out.contains("!list-langs-in-chat"));
        assert!(out.contains("es"));
        assert!(out.contains("eu"));
    }

    #[tokio::test]
    async fn execute_in_chat_list_excludes_basque() {
        let h = TranslateLangsInChatHandler::new();
        let msg = BotMessage {
            source: "+1".into(),
            source_number: None,
            source_name: None,
            text: "!list-langs-in-chat".into(),
            timestamp: 0,
            message_timestamp: 0,
            is_group: true,
            group_id: Some("g".into()),
            group_name: None,
            receiving_account: "+2".into(),
            attachments: vec![],
            quote: None,
        };
        let out = h.execute(&msg).await.unwrap();
        assert!(out.contains("In-chat auto-translate"));
        assert!(out.contains("!translate-all-on"));
        assert!(out.contains("!list-langs"));
        assert!(out.contains("es"));
        assert!(!out.contains(" eu "));
        assert!(!out.contains("Basque"));
    }

    #[test]
    fn language_lists_are_styled() {
        assert!(TranslateLangsHandler::new().styled_reply());
        assert!(TranslateLangsInChatHandler::new().styled_reply());
    }
}
