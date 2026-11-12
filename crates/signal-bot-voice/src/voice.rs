//! Implicit voice note handler — transcribe via Whisper and quote-reply.

use crate::fanout::SharedTranscriptFanout;
use crate::transcribe_store::TranscribeStore;
use crate::voice_attachment_cache::VoiceAttachmentCache;
use async_trait::async_trait;
use signal_bot_core::{AppResult, CommandHandler};
use signal_client::{Attachment, BotMessage, SignalClient};
use std::sync::Arc;
use tracing::{info, instrument, warn};
use whisper_client::{WhisperClient, WhisperError};

#[cfg_attr(not(test), allow(dead_code))]
const DEFAULT_REPLY_PREFIX: &str = "📝 Transcript:";

pub struct VoiceHandler {
    whisper: Arc<WhisperClient>,
    signal: Arc<SignalClient>,
    reply_prefix: String,
    max_attachment_bytes: usize,
    transcribe_store: Option<Arc<TranscribeStore>>,
    voice_cache: Option<Arc<VoiceAttachmentCache>>,
    fanout: Option<SharedTranscriptFanout>,
}

impl VoiceHandler {
    pub fn new(
        whisper: Arc<WhisperClient>,
        signal: Arc<SignalClient>,
        reply_prefix: impl Into<String>,
        max_attachment_bytes: usize,
    ) -> Self {
        Self {
            whisper,
            signal,
            reply_prefix: reply_prefix.into(),
            max_attachment_bytes,
            transcribe_store: None,
            voice_cache: None,
            fanout: None,
        }
    }

    pub fn with_transcribe_store(mut self, store: Arc<TranscribeStore>) -> Self {
        self.transcribe_store = Some(store);
        self
    }

    pub fn with_voice_cache(mut self, cache: Arc<VoiceAttachmentCache>) -> Self {
        self.voice_cache = Some(cache);
        self
    }

    pub fn with_fanout(mut self, fanout: Option<SharedTranscriptFanout>) -> Self {
        self.fanout = fanout;
        self
    }

    fn spawn_fanout(&self, original: &signal_client::BotMessage, spoken: &str) {
        crate::fanout::spawn_fanout(self.fanout.clone(), original, spoken);
    }

    async fn send_quote_reply(&self, message: &BotMessage, body: &str) -> AppResult<()> {
        self.signal.reply_quoted(message, body, None).await?;
        Ok(())
    }

    pub fn format_transcript(text: &str, prefix: &str) -> String {
        format!("{prefix}\n{text}")
    }

    pub fn attachment_filename(audio: &Attachment) -> String {
        // Generic names only — do not forward Signal filenames to the STT vendor.
        if audio.content_type.contains("aac") || audio.content_type.contains("mp4") {
            "voice.m4a".into()
        } else if audio.content_type.contains("ogg") {
            "voice.ogg".into()
        } else {
            "voice.bin".into()
        }
    }

    fn user_message_for_whisper_error(err: &WhisperError) -> &'static str {
        match err {
            WhisperError::EmptyTranscription => {
                "Could not transcribe voice note (no speech detected). Try a clearer recording."
            }
            WhisperError::Http(_) | WhisperError::Api(_) => {
                "Could not transcribe voice note. Try again later."
            }
        }
    }
}

#[async_trait]
impl CommandHandler for VoiceHandler {
    /// Remembers every voice note, including in chats without auto-transcribe, which is where
    /// `!transcribe` is used: Signal's quote of a voice note doesn't carry its attachment.
    /// Keyed by the sent timestamp (`dataMessage.timestamp`), which is what a quote's `id`
    /// holds; the envelope timestamp can differ for a redelivered message.
    fn observe(&self, message: &BotMessage) {
        if let (Some(cache), Some(audio)) = (&self.voice_cache, message.primary_audio_attachment())
        {
            cache.remember(message.reply_target(), message.message_timestamp, audio.clone());
        }
    }

    fn matches(&self, message: &BotMessage) -> bool {
        if !message.is_voice_note() {
            return false;
        }
        self.transcribe_store
            .as_ref()
            .is_some_and(|store| store.is_enabled(message.reply_target(), message.is_group))
    }

    fn reply_with_quote(&self) -> bool {
        true
    }

    fn handles_own_reply(&self) -> bool {
        true
    }

    fn label(&self) -> &'static str {
        "voice"
    }

    #[instrument(skip(self, message), fields(source = %message.source, is_group = message.is_group))]
    async fn execute(&self, message: &BotMessage) -> AppResult<String> {
        let audio = match message.primary_audio_attachment() {
            Some(a) => a,
            None => {
                self.send_quote_reply(message, "Could not read voice attachment.")
                    .await?;
                return Ok(String::new());
            }
        };

        if let Some(expected) = audio.size {
            if expected > self.max_attachment_bytes as i64 {
                warn!(
                    expected_bytes = expected,
                    max = self.max_attachment_bytes,
                    "Voice attachment exceeds size limit"
                );
                self.send_quote_reply(
                    message,
                    "Voice note too long (max 5 min). Send a shorter clip.",
                )
                .await?;
                return Ok(String::new());
            }
        }

        let bytes = match self.signal.download_attachment(&audio.id).await {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!("Failed to download voice attachment {}: {}", audio.id, e);
                self.send_quote_reply(message, "Could not download voice note. Try again later.")
                    .await?;
                return Ok(String::new());
            }
        };

        if bytes.len() > self.max_attachment_bytes {
            warn!(
                bytes = bytes.len(),
                max = self.max_attachment_bytes,
                "Downloaded voice attachment exceeds size limit"
            );
            self.send_quote_reply(
                message,
                "Voice note too long (max 5 min). Send a shorter clip.",
            )
            .await?;
            return Ok(String::new());
        }

        let filename = Self::attachment_filename(audio);
        let result = self
            .whisper
            .transcribe(&bytes, &filename, &audio.content_type)
            .await;

        match result {
            Ok(transcription) => {
                info!(
                    source = %message.source,
                    chars = transcription.text.len(),
                    "Voice note transcribed"
                );
                let spoken = transcription.trimmed_text().to_string();
                let body = Self::format_transcript(&spoken, &self.reply_prefix);
                self.send_quote_reply(message, &body).await?;
                self.spawn_fanout(message, &spoken);
                Ok(String::new())
            }
            Err(e) => {
                warn!("Whisper transcription failed: {}", e);
                self.send_quote_reply(message, Self::user_message_for_whisper_error(&e))
                    .await?;
                Ok(String::new())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fanout::{wait_for_fanout, RecordSend, RecordingFanout, SharedTranscriptFanout};
    use crate::transcribe_store::TranscribeStore;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn format_transcript_includes_prefix() {
        let out = VoiceHandler::format_transcript("Hola mundo", DEFAULT_REPLY_PREFIX);
        assert_eq!(out, "📝 Transcript:\nHola mundo");
    }

    #[test]
    fn attachment_filename_ignores_signal_name() {
        let audio = Attachment {
            content_type: "audio/ogg".into(),
            filename: Some("from-alice-group.ogg".into()),
            id: "x".into(),
            size: None,
            upload_timestamp: None,
        };
        assert_eq!(VoiceHandler::attachment_filename(&audio), "voice.ogg");
    }

    fn dm_voice(size: Option<i64>) -> BotMessage {
        BotMessage {
            source: "+15550002222".into(),
            source_number: Some("+15550002222".into()),
            source_name: None,
            text: String::new(),
            timestamp: 99,
            message_timestamp: 99,
            is_group: false,
            group_id: None,
            group_name: None,
            receiving_account: "+15550001111".into(),
            attachments: vec![Attachment {
                content_type: "audio/aac".into(),
                filename: Some("note.m4a".into()),
                id: "att-1".into(),
                size,
                upload_timestamp: None,
            }],
            quote: None,
        }
    }

    fn test_whisper(url: &str) -> Arc<WhisperClient> {
        Arc::new(
            WhisperClient::new(
                url,
                std::time::Duration::from_secs(5),
                "test-key",
                "openai/whisper-large-v3",
            )
            .unwrap(),
        )
    }

    async fn mount_send_ok(signal_mock: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(signal_mock)
            .await;
    }

    #[tokio::test]
    async fn execute_without_audio_attachment() {
        let signal_mock = MockServer::start().await;
        mount_send_ok(&signal_mock).await;
        let rec = RecordingFanout::new();
        let fanout: SharedTranscriptFanout = rec.clone();
        let handler = VoiceHandler::new(
            test_whisper("http://127.0.0.1:9"),
            Arc::new(SignalClient::new(signal_mock.uri()).unwrap()),
            DEFAULT_REPLY_PREFIX,
            1024,
        )
        .with_fanout(Some(fanout));
        let mut msg = dm_voice(None);
        msg.attachments.clear();
        let out = handler.execute(&msg).await.unwrap();
        assert!(out.is_empty());
        assert!(rec.spoken.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn execute_rejects_oversized_declared_size() {
        let signal_mock = MockServer::start().await;
        mount_send_ok(&signal_mock).await;
        let rec = RecordingFanout::new();
        let fanout: SharedTranscriptFanout = rec.clone();
        let handler = VoiceHandler::new(
            test_whisper("http://127.0.0.1:9"),
            Arc::new(SignalClient::new(signal_mock.uri()).unwrap()),
            DEFAULT_REPLY_PREFIX,
            100,
        )
        .with_fanout(Some(fanout));
        let out = handler.execute(&dm_voice(Some(500))).await.unwrap();
        assert!(out.is_empty());
        assert!(rec.spoken.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn execute_transcribes_via_whisper() {
        let signal_mock = MockServer::start().await;
        let whisper_mock = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v1/attachments/att-1"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"fake-audio-bytes"))
            .mount(&signal_mock)
            .await;
        mount_send_ok(&signal_mock).await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "text": " Hola mundo\n",
                "language": "spanish"
            })))
            .mount(&whisper_mock)
            .await;

        let cache = VoiceAttachmentCache::with_default_capacity();
        let store = Arc::new(TranscribeStore::new(None));
        let msg = dm_voice(Some(16));
        store.set_enabled(msg.reply_target(), true, false);
        let handler = VoiceHandler::new(
            test_whisper(&whisper_mock.uri()),
            Arc::new(SignalClient::new(signal_mock.uri()).unwrap()),
            DEFAULT_REPLY_PREFIX,
            10_000,
        )
        .with_voice_cache(cache.clone())
        .with_transcribe_store(store);

        assert!(handler.matches(&msg));
        assert!(handler.handles_own_reply());
        let out = handler.execute(&msg).await.unwrap();
        assert!(out.is_empty());
        // Only `observe` caches, so each note is remembered once.
        let cached = cache.lookup(msg.reply_target(), msg.message_timestamp);
        assert!(cached.is_none());
    }

    #[test]
    fn observe_remembers_voice_notes_even_with_auto_transcribe_off() {
        let cache = VoiceAttachmentCache::with_default_capacity();
        let handler = VoiceHandler::new(
            test_whisper("http://127.0.0.1:9"),
            Arc::new(SignalClient::new("http://127.0.0.1:9").unwrap()),
            DEFAULT_REPLY_PREFIX,
            10_000,
        )
        .with_voice_cache(cache.clone())
        .with_transcribe_store(Arc::new(TranscribeStore::new(None)));

        // A redelivered envelope: its timestamp differs from the sent timestamp, which is
        // what a quote of this note carries as its id.
        let mut msg = dm_voice(Some(16));
        msg.timestamp = 1_000;
        msg.message_timestamp = 77;
        assert!(!handler.matches(&msg), "auto-transcribe is off");
        handler.observe(&msg);
        let cached = cache.lookup(msg.reply_target(), 77).unwrap();
        assert_eq!(cached.id, "att-1");
        assert!(cache.lookup(msg.reply_target(), 1_000).is_none());
    }

    #[test]
    fn observe_without_cache_or_audio_is_a_no_op() {
        let handler = VoiceHandler::new(
            test_whisper("http://127.0.0.1:9"),
            Arc::new(SignalClient::new("http://127.0.0.1:9").unwrap()),
            DEFAULT_REPLY_PREFIX,
            10_000,
        );
        handler.observe(&dm_voice(Some(16)));

        let cache = VoiceAttachmentCache::with_default_capacity();
        let handler = handler.with_voice_cache(cache.clone());
        let mut text = dm_voice(Some(16));
        text.attachments.clear();
        text.text = "hello".into();
        handler.observe(&text);
        assert!(cache.lookup(text.reply_target(), text.timestamp).is_none());
    }

    #[tokio::test]
    async fn execute_handles_download_failure() {
        let signal_mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/attachments/att-1"))
            .respond_with(ResponseTemplate::new(500).set_body_string("fail"))
            .mount(&signal_mock)
            .await;
        mount_send_ok(&signal_mock).await;

        let rec = RecordingFanout::new();
        let fanout: SharedTranscriptFanout = rec.clone();
        let handler = VoiceHandler::new(
            test_whisper("http://127.0.0.1:9"),
            Arc::new(SignalClient::new(signal_mock.uri()).unwrap()),
            DEFAULT_REPLY_PREFIX,
            10_000,
        )
        .with_fanout(Some(fanout));
        let out = handler.execute(&dm_voice(Some(10))).await.unwrap();
        assert!(out.is_empty());
        assert!(rec.spoken.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn execute_sends_transcript_before_fanout() {
        let signal_mock = MockServer::start().await;
        let whisper_mock = MockServer::start().await;
        let rec = RecordingFanout::new();

        Mock::given(method("GET"))
            .and(path("/v1/attachments/att-1"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"fake-audio-bytes"))
            .mount(&signal_mock)
            .await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .respond_with(RecordSend(rec.clone()))
            .expect(1)
            .mount(&signal_mock)
            .await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "text": "hola",
                "language": "spanish"
            })))
            .mount(&whisper_mock)
            .await;

        let fanout: SharedTranscriptFanout = rec.clone();
        let handler = VoiceHandler::new(
            test_whisper(&whisper_mock.uri()),
            Arc::new(SignalClient::new(signal_mock.uri()).unwrap()),
            DEFAULT_REPLY_PREFIX,
            10_000,
        )
        .with_fanout(Some(fanout));
        let msg = dm_voice(Some(16));
        handler.execute(&msg).await.unwrap();
        wait_for_fanout(&rec).await;
        assert_eq!(*rec.events.lock().unwrap(), vec!["send", "fanout"]);
        assert_eq!(*rec.spoken.lock().unwrap(), vec!["hola".to_string()]);
        assert_eq!(
            *rec.sources.lock().unwrap(),
            vec!["+15550002222".to_string()]
        );
    }

    #[tokio::test]
    async fn execute_skips_fanout_when_send_fails() {
        let signal_mock = MockServer::start().await;
        let whisper_mock = MockServer::start().await;
        let rec = RecordingFanout::new();

        Mock::given(method("GET"))
            .and(path("/v1/attachments/att-1"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"fake-audio-bytes"))
            .mount(&signal_mock)
            .await;
        Mock::given(method("POST"))
            .and(path("/v2/send"))
            .respond_with(ResponseTemplate::new(500).set_body_string("fail"))
            .mount(&signal_mock)
            .await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "text": "hola",
                "language": "spanish"
            })))
            .mount(&whisper_mock)
            .await;

        let fanout: SharedTranscriptFanout = rec.clone();
        let handler = VoiceHandler::new(
            test_whisper(&whisper_mock.uri()),
            Arc::new(SignalClient::new(signal_mock.uri()).unwrap()),
            DEFAULT_REPLY_PREFIX,
            10_000,
        )
        .with_fanout(Some(fanout));
        assert!(handler.execute(&dm_voice(Some(16))).await.is_err());
        assert!(rec.spoken.lock().unwrap().is_empty());
        assert!(rec.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn execute_skips_fanout_on_whisper_error() {
        let signal_mock = MockServer::start().await;
        let whisper_mock = MockServer::start().await;
        let rec = RecordingFanout::new();

        Mock::given(method("GET"))
            .and(path("/v1/attachments/att-1"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"fake-audio-bytes"))
            .mount(&signal_mock)
            .await;
        mount_send_ok(&signal_mock).await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("fail"))
            .mount(&whisper_mock)
            .await;

        let fanout: SharedTranscriptFanout = rec.clone();
        let handler = VoiceHandler::new(
            test_whisper(&whisper_mock.uri()),
            Arc::new(SignalClient::new(signal_mock.uri()).unwrap()),
            DEFAULT_REPLY_PREFIX,
            10_000,
        )
        .with_fanout(Some(fanout));
        let out = handler.execute(&dm_voice(Some(16))).await.unwrap();
        assert!(out.is_empty());
        assert!(rec.spoken.lock().unwrap().is_empty());
    }
}
