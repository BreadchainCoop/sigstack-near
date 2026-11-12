//! English `!help`, product, and `!privacy` menu text.
//!
//! Menus are English-only for now; multi-language UI is deferred.
//! Command-list layout follows the Signal mobile menu standard:
//! [`docs/solutions/signal-mobile-menus.md`](../../../../docs/solutions/signal-mobile-menus.md).

pub fn help_menu() -> &'static str {
    HELP_HUB
}

/// Voice product menu (`!transcription`).
pub fn transcription_menu() -> &'static str {
    HELP_TRANSCRIPTION
}

/// Hub descriptive menu (`!info`).
pub fn info_menu() -> &'static str {
    INFO_HUB
}

pub fn thread_help_menu() -> &'static str {
    HELP_THREAD
}

pub fn thread_info_menu() -> &'static str {
    INFO_THREAD
}

pub fn translation_threads_menu() -> &'static str {
    TRANSLATION_THREADS_MENU
}

pub fn translation_in_chat_menu(translate_all_enabled: bool) -> &'static str {
    if translate_all_enabled {
        TRANSLATION_IN_CHAT_MENU
    } else {
        TRANSLATION_IN_CHAT_MENU_AUTO_DISABLED
    }
}

/// How Language Threads works (use case + flow).
pub fn help_threads_guide() -> &'static str {
    HELP_THREADS_GUIDE
}

/// How in-chat translation works (use case + flow).
pub fn help_in_chat_guide() -> &'static str {
    HELP_IN_CHAT_GUIDE
}

/// How voice transcription works (use case + flow).
pub fn help_transcription_guide() -> &'static str {
    HELP_TRANSCRIPTION_GUIDE
}

/// Legacy `!translation` redirect naming the two product menus.
pub fn translation_split_redirect() -> &'static str {
    TRANSLATION_SPLIT_REDIRECT
}

pub fn privacy_menu() -> &'static str {
    PRIVACY_MENU
}

pub use signal_bot_core::is_exact_command_any;

pub(crate) const HELP_COMMANDS: &[&str] = &["!help"];
pub(crate) const INFO_COMMANDS: &[&str] = &["!info"];
pub(crate) const PRIVACY_COMMANDS: &[&str] = &["!privacy"];
pub(crate) const COMMANDS_COMMANDS: &[&str] = &["!commands", "!command"];
pub(crate) const TRANSLATION_REDIRECT_COMMANDS: &[&str] = &["!translation", "!translations"];

pub(crate) const TRANSLATION_THREADS_MENU_COMMANDS: &[&str] = &[
    "!translation-threads",
    "!translate-threads",
    "!translate-thread",
    "!translation-thread",
    "!translation threads",
    "!translate threads",
    "!translation thread",
    "!translate thread",
];

pub(crate) const TRANSLATION_IN_CHAT_MENU_COMMANDS: &[&str] = &[
    "!translation-in-chat",
    "!translate-in-chat",
    "!translation-inchat",
    "!translate-inchat",
    "!translation in-chat",
    "!translate in-chat",
];

pub(crate) const IN_CHAT_MENU_COMMANDS: &[&str] = &["!in-chat", "!inchat"];

pub(crate) const TRANSCRIPTION_MENU_COMMANDS: &[&str] = &["!transcription", "!transcriptions"];

pub(crate) const HELP_THREADS_COMMANDS: &[&str] = &[
    "!help-threads",
    "!help-thread",
    "!help thread",
    "!help threads",
];

pub(crate) const HELP_IN_CHAT_COMMANDS: &[&str] = &[
    "!help-in-chat",
    "!help-inchat",
    "!help in-chat",
    "!help in chat",
];

pub(crate) const HELP_TRANSCRIPTION_COMMANDS: &[&str] = &[
    "!help-transcription",
    "!help-transcribe",
    "!help-transcript",
    "!help transcription",
    "!help transcribe",
    "!help transcript",
];

/// Product hub menu for Language Threads (canonical + common typos).
pub fn is_translation_threads_menu_command(text: &str) -> bool {
    is_exact_command_any(text, TRANSLATION_THREADS_MENU_COMMANDS)
}

/// Product hub menu for in-chat translation (canonical + common typos).
pub fn is_translation_in_chat_menu_command(text: &str) -> bool {
    is_exact_command_any(text, TRANSLATION_IN_CHAT_MENU_COMMANDS)
}

pub fn is_in_chat_menu_command(text: &str) -> bool {
    is_exact_command_any(text, IN_CHAT_MENU_COMMANDS)
}

pub fn is_transcription_menu_command(text: &str) -> bool {
    is_exact_command_any(text, TRANSCRIPTION_MENU_COMMANDS)
}

pub fn is_translation_redirect_command(text: &str) -> bool {
    is_exact_command_any(text, TRANSLATION_REDIRECT_COMMANDS)
}

pub fn is_help_threads_command(text: &str) -> bool {
    is_exact_command_any(text, HELP_THREADS_COMMANDS)
}

pub fn is_help_in_chat_command(text: &str) -> bool {
    is_exact_command_any(text, HELP_IN_CHAT_COMMANDS)
}

pub fn is_help_transcription_command(text: &str) -> bool {
    is_exact_command_any(text, HELP_TRANSCRIPTION_COMMANDS)
}

const HELP_TRANSCRIPTION: &str = r#"Voice Transcription

AUTO:
!transcribe-on
!transcribe-off

PER MSG QUOTE REPLY:
!transcribe

!help-transcription"#;

const HELP_HUB: &str = r#"--Sigstack--

MENUS:
!translation-threads
!translation-in-chat
!transcription

OTHER:
!verify <challenge>
!info
!privacy
!help"#;

const HELP_THREAD: &str = r#"Language Thread

!rename <name>
  Change this group's name
!leave
  Leave this Language Thread
!info
!commands"#;

const INFO_HUB: &str = r#"--Sigstack--

!translation-threads
  Language Threads — multilingual main chat + language sidecars

!translation-in-chat
  In-chat translation — auto or quote-translate in this group

!transcription
  Voice transcription — quote !transcribe or !transcribe-on

!verify <challenge>
  Prove this bot runs the approved code in a TEE

!privacy
  Privacy, TEE, and !verify attestation

!help-transcription
  How voice transcription works

!info
  This menu (commands with descriptions)

!help
  Compact command list"#;

const INFO_THREAD: &str = r#"Language Thread

!rename <name>
  Change this Language Thread's group name

!leave
  Leave this Language Thread

!info
  This menu (commands with descriptions)

!commands
  Compact command list"#;

const TRANSLATION_THREADS_MENU: &str = r#"Join/Create Language Thread

!list-langs
!translate-me-thread <lang>
!translate-me-thread <main> <thread>
!help-threads

examples:
   !translate-me-thread es
   !translate-me-thread es en

!enable-in-chat (disable threads)
!help"#;

const TRANSLATION_IN_CHAT_MENU: &str = r#"In-chat Translation

!list-langs-in-chat
!translate-all-on <lang1> <lang2>
!translate-all-off
!translate-me-on <lang1> <lang2>
!translate-me-off
!translate <lang> (as reply)
!help-in-chat

examples:
   !translate-all-on fr zh
   !translate-me-on ru ar
   !translate es

Quote !translate accepts any Language Threads language (!list-langs).
!enable-threads (disable in-chat)
!help"#;

const TRANSLATION_IN_CHAT_MENU_AUTO_DISABLED: &str = r#"In-chat translation

Auto-translate is disabled on this bot (!translate-all-on).

!translate <lang>
  Reply to a message

!help-in-chat
!help"#;

const HELP_THREADS_GUIDE: &str = r#"Language Threads and Bilingual Threads — how they work

Language Threads: people need monolingual lanes, but organizers still want one shared multilingual main chat.
Bilingual Threads: the group is exactly two languages and both rooms should receive translated text.

How Language Threads works:
- Main group stays multilingual (everyone can post in any language).
- Each language gets a sidecar Signal group ("Language Thread").
- Sidecar → main is relay-only (original language). Main → sidecars translate.

How Bilingual Threads works:
- !translate-me-thread es en assigns Spanish to main and English to the one sidecar.
- Both directions translate into the destination room's language (relay if already that language).
- No third sidecar. Not the same as one Language Thread.

Typical use:
1. In main, send !list-langs then !translate-me-thread es (Language Threads) or !translate-me-thread es en (Bilingual Threads)
2. Accept the sidecar invite
3. Read/write in that thread; the bot bridges with main
4. !leave from a thread to leave it
5. From main, !enable-in-chat tears down threads if you want in-chat auto instead

In-chat auto, Language Threads, and Bilingual Threads cannot run at the same time.

Commands: !translation-threads
!help"#;

const HELP_IN_CHAT_GUIDE: &str = r#"In-chat translation — how it works

Use when everyone stays in one Signal group and wants bilingual (or quote) translation there.

How it works:
- No sidecar groups — replies stay in this chat as quote-replies.
- Group-wide: !translate-all-on es en auto-translates messages between that pair (everyone uses this pair while it is on).
- Personal: !translate-me-on es en auto-translates only your messages when group-wide is off.
- One-off: reply to a message with !translate <lang>

Typical use:
1. Pick two in-chat auto languages (!list-langs-in-chat)
2. !translate-all-on es en (or !translate-me-on for just you)
3. Chat normally; the bot quote-replies translations
4. !translate-all-off / !translate-me-off to stop
5. From this group, !enable-threads clears in-chat auto if you want Language Threads or Bilingual Threads instead

In-chat auto, Language Threads, and Bilingual Threads cannot run at the same time.

Commands: !translation-in-chat
!help"#;

const HELP_TRANSCRIPTION_GUIDE: &str = r#"Voice transcription — how it works

Use when people send voice notes and you want text in the same Signal chat.

Voice is decrypted in this TEE, then sent (audio only, no Signal metadata) to NEAR AI Whisper Large V3 in their GPU TEE. Use !privacy for suite privacy and !verify.

How it works:
- Auto mode (default off): send !transcribe-on so inbound voice notes become quote-reply transcripts.
- Manual: quote a voice note and send !transcribe
- Toggle with !transcribe-on / !transcribe-off

Typical use:
1. Add this bot to the group (it auto-accepts invites)
2. Quote a voice note and send !transcribe, or !transcribe-on for auto
3. With in-chat auto or Language Threads on, transcripts are translated in this same bot

Commands: !transcription
Privacy / TEE: !privacy"#;

const TRANSLATION_SPLIT_REDIRECT: &str = r#"Translation has two menus:

!translation-threads
!translation-in-chat

!help"#;

const PRIVACY_MENU: &str = r#"Privacy & TEE

!verify <challenge>

example:
   !verify "write something unique here"

Sigstack is one Signal number in one Phala TEE/CVM.

Translation: Signal text is processed in this TEE and translated via NEAR AI private inference.

Transcription: Voice notes are decrypted in this TEE. Audio bytes (no phone, group id, or filename) are sent to NEAR AI Whisper Large V3 (GPU TEE). Transcripts come back here and are posted in Signal.

Attestation: !verify <your text> returns a quote from this CVM that signs your text, and the agent registration on NEAR that approved its code. It also reports the NEAR AI checks: every translation must be signed by an attested model GPU TEE, and the Whisper model is attested too (NEAR AI doesn't sign individual transcripts yet).

!help"#;

#[cfg(test)]
mod tests {
    use super::*;
    use signal_bot_core::is_exact_command;

    #[test]
    fn help_translation_is_hub() {
        let h = help_menu();
        assert!(h.contains("!translation-threads"));
        assert!(h.contains("!translation-in-chat"));
        assert!(h.contains("!transcription"));
        assert!(h.contains("!privacy"));
        assert!(h.contains("!info"));
        assert!(!h.contains("GUIDES:"));
        assert!(!h.contains("!help-threads"));
        assert!(!h.contains("!help-in-chat"));
        assert!(!h.contains("!help-transcription"));
        assert!(!h.contains("Language Threads\n"));
        assert!(!h.contains("In-chat translation\n"));
        assert!(!h.contains("  "));
        assert!(!h.contains("!ask"));
        assert!(!h.contains("!models"));
    }

    #[test]
    fn info_hub_has_breaks_and_descriptions() {
        let h = info_menu();
        assert!(h.contains("!translation-threads\n  "));
        assert!(h.contains("!translation-in-chat\n  "));
        assert!(h.contains("!transcription\n  "));
        assert!(h.contains("!privacy\n  "));
        assert!(h.contains("!info\n  "));
        assert!(h.contains("!help\n  "));
        assert!(h.contains("\n\n!translation-in-chat"));
        assert!(h.contains("Privacy, TEE, and !verify attestation"));
        assert!(h.contains("!verify <challenge>\n  "));
    }

    #[test]
    fn thread_menu_has_leave_not_subscribe() {
        let h = thread_help_menu();
        assert!(h.contains("!leave"));
        assert!(h.contains("!rename"));
        assert!(h.contains("!commands"));
        assert!(!h.contains("!translate-me-thread"));
        assert!(!h.contains("!translate-me-on"));
        // Hub !help must not appear as the thread menu trigger.
        assert!(!h.trim_end().ends_with("!help"));
    }

    #[test]
    fn threads_menu_lists_thread_commands() {
        let h = translation_threads_menu();
        assert!(h.contains("!translate-me-thread <lang>"));
        assert!(h.contains("!translate-me-thread <main> <thread>"));
        assert!(h.contains("!translate-me-thread es en"));
        assert!(h.contains("!enable-in-chat"));
        assert!(h.contains("!help-threads"));
        assert!(h.contains("!list-langs"));
        assert!(!h.contains("!leave"));
        assert!(!h.contains("!translate-all-on"));
    }

    #[test]
    fn in_chat_menu_lists_auto_commands() {
        let h = translation_in_chat_menu(true);
        assert!(h.contains("!translate-all-on <lang1> <lang2>"));
        assert!(h.contains("!translate-me-on <lang1> <lang2>"));
        assert!(h.contains("!enable-threads"));
        assert!(h.contains("!help-in-chat"));
        assert!(h.contains("!translate <lang>"));
        assert!(!h.contains("!translate-me-thread"));
        assert!(!h.contains("!models"));
    }

    #[test]
    fn feature_guides_cover_use_cases() {
        let threads = help_threads_guide();
        assert!(threads.contains("Language Threads"));
        assert!(threads.contains("Bilingual Threads"));
        assert!(threads.contains("!translate-me-thread es"));
        assert!(threads.contains("!translate-me-thread es en"));
        assert!(threads.contains("sidecar"));
        let in_chat = help_in_chat_guide();
        assert!(in_chat.contains("In-chat translation"));
        assert!(in_chat.contains("!translate-all-on"));
        assert!(in_chat.contains("quote"));
        let transcription = help_transcription_guide();
        assert!(transcription.contains("Voice transcription"));
        assert!(transcription.contains("NEAR AI"));
        assert!(transcription.contains("Whisper"));
        assert!(transcription.contains("!transcribe"));
        assert!(transcription.contains("default off"));
        assert!(transcription.contains("Add this bot"));
        assert!(transcription.contains("!privacy"));
        assert!(!transcription.trim_end().ends_with("!help"));
    }

    #[test]
    fn in_chat_menu_auto_disabled_hides_on_commands() {
        let h = translation_in_chat_menu(false);
        assert!(h.contains("Auto-translate is disabled"));
        assert!(!h.contains("!translate-all-on <lang1>"));
        assert!(h.contains("!translate <lang>"));
    }

    #[test]
    fn exact_command_does_not_match_prefixed() {
        assert!(is_exact_command("!translation", "!translation"));
        assert!(!is_exact_command("!translation-on es en", "!translation"));
        assert!(is_exact_command("!in-chat", "!in-chat"));
        assert!(!is_exact_command("!in-chat-extra", "!in-chat"));
        assert!(is_exact_command(
            "!translation-threads",
            "!translation-threads"
        ));
    }

    #[test]
    fn translation_threads_menu_command_aliases() {
        assert!(is_translation_threads_menu_command("!translation-threads"));
        assert!(is_translation_threads_menu_command("!translate-threads"));
        assert!(is_translation_threads_menu_command("!translate-thread"));
        assert!(is_translation_threads_menu_command("!translation-thread"));
        assert!(is_translation_threads_menu_command("!translate thread"));
        assert!(is_translation_threads_menu_command("!translation threads"));
        assert!(is_translation_threads_menu_command(
            "  !translation-threads  "
        ));
        assert!(!is_translation_threads_menu_command(
            "!translation-on es en"
        ));
        assert!(!is_translation_threads_menu_command(
            "!translation-threads-extra"
        ));
    }

    #[test]
    fn translation_in_chat_menu_command_aliases() {
        assert!(is_translation_in_chat_menu_command("!translation-in-chat"));
        assert!(is_translation_in_chat_menu_command("!translate-in-chat"));
        assert!(is_translation_in_chat_menu_command("!translate-inchat"));
        assert!(is_translation_in_chat_menu_command("!translate in-chat"));
        assert!(is_translation_in_chat_menu_command(
            "  !translate-in-chat  "
        ));
        assert!(!is_translation_in_chat_menu_command(
            "!translation-on es en"
        ));
        assert!(!is_translation_in_chat_menu_command(
            "!translation-in-chat-extra"
        ));
    }

    #[test]
    fn help_guide_aliases() {
        assert!(is_help_threads_command("!help-thread"));
        assert!(is_help_threads_command("!help thread"));
        assert!(is_help_threads_command("!Help-Threads"));
        assert!(is_help_in_chat_command("!help-inchat"));
        assert!(is_help_in_chat_command("!help in chat"));
        assert!(is_help_transcription_command("!help-transcribe"));
        assert!(is_help_transcription_command("!help transcript"));
        assert!(!is_help_threads_command("!help"));
        assert!(!is_help_threads_command("!help extra"));
        assert!(!is_help_transcription_command("!transcription"));
        assert!(!is_transcription_menu_command("!transcript"));
        assert!(is_transcription_menu_command("!transcriptions"));
        assert!(is_in_chat_menu_command("!inchat"));
        assert!(is_translation_redirect_command("!translations"));
    }

    #[test]
    fn help_transcription_covers_voice() {
        let h = transcription_menu();
        assert!(h.contains("!transcribe"));
        assert!(h.contains("!transcribe-on"));
        assert!(h.contains("!transcribe-off"));
        assert!(h.contains("!help-transcription"));
        assert!(!h.contains("!privacy-transcription"));
        assert!(!h.contains("!privacy-translation"));
        assert!(!h.contains("!ask"));
        assert!(!h.contains("!translate-me-on"));
        assert!(!h.contains("!verify"));
        assert!(!h.contains("!info"));
        assert!(!h.trim_end().ends_with("!help"));
    }

    #[test]
    fn privacy_menu_covers_near_stt() {
        let m = privacy_menu();
        assert!(m.contains("one Phala TEE/CVM"));
        assert!(m.contains("one Signal number"));
        assert!(m.contains("Translation:"));
        assert!(m.contains("Transcription:"));
        assert!(m.contains("NEAR AI Whisper"));
        assert!(m.contains("!verify <challenge>"));
        assert!(!m.contains("two Signal"));
        assert!(!m.contains("both bots"));
        assert!(!m.contains("**"));
    }

    #[test]
    fn product_menus_omit_verify() {
        assert!(!translation_threads_menu().contains("!verify"));
        assert!(!translation_in_chat_menu(true).contains("!verify"));
        assert!(!translation_in_chat_menu(false).contains("!verify"));
        assert!(!thread_help_menu().contains("!verify"));
    }

    #[test]
    fn help_lists_verify() {
        assert!(help_menu().contains("OTHER:\n!verify <challenge>\n"));
    }

    #[test]
    fn help_transcription_guide_is_one_bot() {
        let g = help_transcription_guide();
        assert!(g.contains("Add this bot"));
        assert!(g.contains("!transcribe"));
        assert!(!g.contains("PEER_PHONE"));
        assert!(!g.contains("invites the transcription"));
    }
}
