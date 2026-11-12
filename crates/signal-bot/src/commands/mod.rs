//! Bot command handlers.

mod bot_username;
#[cfg(test)]
mod command_aliases;
mod dm_fallback;
mod help;
mod link;
mod menu_locale;
mod privacy;
mod product_menus;
mod rename;
mod translate;
mod translate_all;
pub mod translate_lang;
mod translate_langs;
mod translate_me;
mod translate_service;
mod verify;

pub use bot_username::BotUsernameHandler;
pub use dm_fallback::DmFallbackHandler;
pub use help::{CommandsHandler, HelpHandler, InfoHandler};
pub use link::LinkHandler;
pub use privacy::PrivacyHandler;
pub use product_menus::{
    HelpInChatHandler, HelpThreadsHandler, HelpTranscriptionHandler, InChatMenuHandler,
    TranscriptionMenuHandler, TranslationInChatMenuHandler, TranslationMenuHandler,
    TranslationThreadsMenuHandler,
};
pub use rename::RenameHandler;
pub use signal_bot_core::CommandHandler;
pub use translate::TranslateHandler;
pub use translate_all::TranslateAllHandler;
pub use translate_langs::{TranslateLangsHandler, TranslateLangsInChatHandler};
pub use translate_me::TranslateMeHandler;
pub use translate_service::DEFAULT_TRANSCRIPT_PREFIX;
pub use verify::VerifyHandler;
