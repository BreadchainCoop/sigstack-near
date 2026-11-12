//! Help / info / thread-commands menus.

use crate::commands::menu_locale::{
    help_menu, info_menu, is_exact_command_any, thread_help_menu, thread_info_menu,
    COMMANDS_COMMANDS, HELP_COMMANDS, INFO_COMMANDS,
};
use crate::commands::CommandHandler;
use crate::error::AppResult;
use crate::group_preferences_store::GroupPreferencesStore;
use async_trait::async_trait;
use signal_client::BotMessage;
use std::sync::Arc;

const NOT_THREAD_MSG: &str = "!commands is only available in a Language Thread.";

pub struct HelpHandler;

impl HelpHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HelpHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandHandler for HelpHandler {
    fn matches(&self, message: &BotMessage) -> bool {
        // Exact match so !help-threads / !help-in-chat are not swallowed.
        is_exact_command_any(&message.text, HELP_COMMANDS)
    }

    fn label(&self) -> &'static str {
        "help"
    }

    async fn execute(&self, message: &BotMessage) -> AppResult<String> {
        let _ = message;
        Ok(help_menu().into())
    }
}

/// Sidecar-only compact Language Thread command list (`!commands`).
pub struct CommandsHandler {
    group_prefs: Arc<GroupPreferencesStore>,
}

impl CommandsHandler {
    pub fn new(group_prefs: Arc<GroupPreferencesStore>) -> Self {
        Self { group_prefs }
    }
}

#[async_trait]
impl CommandHandler for CommandsHandler {
    fn matches(&self, message: &BotMessage) -> bool {
        is_exact_command_any(&message.text, COMMANDS_COMMANDS)
    }

    fn label(&self) -> &'static str {
        "commands"
    }

    async fn execute(&self, message: &BotMessage) -> AppResult<String> {
        let Some(group_id) = message.group_id.as_deref() else {
            return Ok(NOT_THREAD_MSG.into());
        };
        if self.group_prefs.lookup_sidecar(group_id).is_none() {
            return Ok(NOT_THREAD_MSG.into());
        }
        Ok(thread_help_menu().into())
    }
}

/// Same menus as [`HelpHandler`], with per-command explanations and blank-line breaks.
pub struct InfoHandler {
    group_prefs: Arc<GroupPreferencesStore>,
}

impl InfoHandler {
    pub fn new(group_prefs: Arc<GroupPreferencesStore>) -> Self {
        Self { group_prefs }
    }
}

#[async_trait]
impl CommandHandler for InfoHandler {
    fn matches(&self, message: &BotMessage) -> bool {
        is_exact_command_any(&message.text, INFO_COMMANDS)
    }

    fn label(&self) -> &'static str {
        "info"
    }

    async fn execute(&self, message: &BotMessage) -> AppResult<String> {
        if let Some(group_id) = message.group_id.as_deref() {
            if self.group_prefs.lookup_sidecar(group_id).is_some() {
                return Ok(thread_info_menu().into());
            }
        }
        Ok(info_menu().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dm(text: &str) -> BotMessage {
        BotMessage {
            source: "+15550002222".into(),
            source_number: Some("+15550002222".into()),
            source_name: None,
            text: text.into(),
            timestamp: 1,
            message_timestamp: 1,
            is_group: false,
            group_id: None,
            group_name: None,
            receiving_account: "+15550001111".into(),
            attachments: vec![],
            quote: None,
        }
    }

    fn group(text: &str, group_id: &str) -> BotMessage {
        let mut m = dm(text);
        m.is_group = true;
        m.group_id = Some(group_id.into());
        m
    }

    #[tokio::test]
    async fn help_returns_hub_menu() {
        let handler = HelpHandler::new();

        assert!(handler.matches(&dm("!help")));
        assert!(handler.matches(&dm("!HELP")));
        assert!(!handler.matches(&dm("!help-threads")));
        assert!(!handler.matches(&dm("!help-thread")));
        assert!(!handler.matches(&dm("!help-in-chat")));
        assert!(!handler.matches(&dm("!help thread")));
        let t = handler.execute(&dm("!help")).await.unwrap();
        assert!(t.contains("!translation-threads"));
        assert!(t.contains("!translation-in-chat"));
        assert!(t.contains("!transcription"));
        assert!(t.contains("!privacy"));
        assert!(t.contains("!info"));
        assert!(!t.contains("GUIDES:"));
        assert!(!t.contains("!help-threads"));
        assert!(!t.contains("!help-in-chat"));
        assert!(!t.contains("!help-transcription"));
        assert!(!t.contains("!transcribe-on"));
        assert!(!t.contains("!translate-me-on"));
        assert!(!t.contains("Voice notes in this chat"));
        assert!(!t.contains("!set-en"));
    }

    #[tokio::test]
    async fn info_returns_described_hub() {
        let store = GroupPreferencesStore::new_in_memory(0);
        let handler = InfoHandler::new(store);
        assert!(handler.matches(&dm("!info")));
        let out = handler.execute(&dm("!info")).await.unwrap();
        assert!(out.contains("!translation-threads\n  "));
        assert!(out.contains("\n\n!privacy"));
        assert!(out.contains("Compact command list"));
        assert!(out.contains("!privacy\n  "));
        assert!(out.contains("TEE"));
        assert!(out.contains("!verify <challenge>\n  Prove this bot"));
    }

    #[tokio::test]
    async fn help_in_sidecar_returns_hub_menu() {
        let store = GroupPreferencesStore::new_in_memory(0);
        store.set_sidecar("main-1", "it", "group.it".into(), "it-internal".into());
        let handler = HelpHandler::new();
        let out = handler
            .execute(&group("!help", "it-internal"))
            .await
            .unwrap();
        assert!(out.contains("!translation-threads"));
        assert!(!out.contains("!rename"));
    }

    #[tokio::test]
    async fn commands_in_sidecar_returns_thread_menu() {
        let store = GroupPreferencesStore::new_in_memory(0);
        store.set_sidecar("main-1", "it", "group.it".into(), "it-internal".into());
        let handler = CommandsHandler::new(store);
        assert!(handler.matches(&group("!commands", "it-internal")));
        assert!(handler.matches(&group("!command", "it-internal")));
        assert!(!handler.matches(&group("!help", "it-internal")));
        let out = handler
            .execute(&group("!commands", "it-internal"))
            .await
            .unwrap();
        assert!(out.contains("!rename <name>"));
        assert!(out.contains("!leave"));
        assert!(out.contains("!commands"));
        assert!(!out.contains("!translation-threads"));
    }

    #[tokio::test]
    async fn commands_outside_sidecar_refuses() {
        let store = GroupPreferencesStore::new_in_memory(0);
        store.set_sidecar("main-1", "it", "group.it".into(), "it-internal".into());
        let handler = CommandsHandler::new(store);
        let out = handler
            .execute(&group("!commands", "main-1"))
            .await
            .unwrap();
        assert_eq!(out, NOT_THREAD_MSG);
        let out = handler.execute(&dm("!commands")).await.unwrap();
        assert_eq!(out, NOT_THREAD_MSG);
    }

    #[tokio::test]
    async fn info_in_sidecar_returns_thread_info() {
        let store = GroupPreferencesStore::new_in_memory(0);
        store.set_sidecar("main-1", "it", "group.it".into(), "it-internal".into());
        let handler = InfoHandler::new(store);
        let out = handler
            .execute(&group("!info", "it-internal"))
            .await
            .unwrap();
        assert!(out.contains("!rename <name>\n  "));
        assert!(out.contains("!leave\n  "));
        assert!(!out.contains("!translation-threads"));
    }

    #[tokio::test]
    async fn help_in_main_stays_hub() {
        let handler = HelpHandler::new();
        let out = handler.execute(&group("!help", "main-1")).await.unwrap();
        assert!(out.contains("!translation-threads"));
        assert!(!out.contains("!rename"));
    }
}
