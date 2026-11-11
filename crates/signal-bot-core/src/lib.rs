//! Shared types for signal-bot product crates (handlers trait + errors).

pub mod command_match;
pub mod error;
pub mod handler;

pub use command_match::{
    command_head, is_exact_command, is_exact_command_any, normalize_command_head, normalize_exact,
    normalize_token, starts_with_word, starts_with_word_any, strip_prefix_list, strip_word_prefix,
};
pub use error::{AppError, AppResult};
pub use handler::{CommandHandler, REPLY_BREAK};
