//! Recent voice note attachments for `!transcribe` quote-reply fallback.

use signal_client::Attachment;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

const DEFAULT_CAPACITY: usize = 1_000;

type Key = (String, i64);

#[derive(Default)]
struct Entries {
    by_key: HashMap<Key, Attachment>,
    /// Keys in the order they were first remembered, oldest at the front.
    order: VecDeque<Key>,
}

/// Maps `(chat_id, message_timestamp)` → voice attachment.
///
/// Signal quote metadata often omits downloadable attachment IDs for voice notes;
/// we populate this cache whenever a voice note arrives on a chat. Only attachment
/// metadata is kept (no audio), in memory. When full, the oldest entry is dropped.
pub struct VoiceAttachmentCache {
    entries: RwLock<Entries>,
    capacity: usize,
}

impl VoiceAttachmentCache {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            entries: RwLock::new(Entries::default()),
            capacity: capacity.max(1),
        })
    }

    pub fn with_default_capacity() -> Arc<Self> {
        Self::new(DEFAULT_CAPACITY)
    }

    pub fn remember(&self, chat_id: &str, message_timestamp: i64, attachment: Attachment) {
        let key = (chat_id.to_string(), message_timestamp);
        let mut entries = self.write();
        if entries.by_key.insert(key.clone(), attachment).is_some() {
            // Already known (e.g. a redelivered envelope): keep its place in line.
            return;
        }
        entries.order.push_back(key);
        while entries.order.len() > self.capacity {
            if let Some(oldest) = entries.order.pop_front() {
                entries.by_key.remove(&oldest);
            }
        }
    }

    pub fn lookup(&self, chat_id: &str, message_timestamp: i64) -> Option<Attachment> {
        self.read()
            .by_key
            .get(&(chat_id.to_string(), message_timestamp))
            .cloned()
    }

    /// Each update finishes its few writes before releasing the lock, so a poisoned
    /// lock still holds a consistent cache.
    fn read(&self) -> RwLockReadGuard<'_, Entries> {
        self.entries.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Entries> {
        self.entries.write().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_audio(id: &str) -> Attachment {
        Attachment {
            content_type: "audio/ogg".into(),
            filename: None,
            id: id.into(),
            size: Some(1024),
            upload_timestamp: Some(1_719_000_000_000),
        }
    }

    #[test]
    fn remembers_and_lookup_by_chat_and_timestamp() {
        let cache = VoiceAttachmentCache::new(10);
        let audio = sample_audio("voice-1");
        cache.remember("group.abc", 1_719_000_000_000, audio.clone());

        let found = cache.lookup("group.abc", 1_719_000_000_000).unwrap();
        assert_eq!(found.id, "voice-1");
        assert!(cache.lookup("group.abc", 999).is_none());
        assert!(cache.lookup("other", 1_719_000_000_000).is_none());
    }

    #[test]
    fn full_cache_drops_the_oldest_entry_first() {
        let cache = VoiceAttachmentCache::new(3);
        for t in 1..=100 {
            cache.remember("chat", t, sample_audio(&format!("v{t}")));
        }
        // Only the three most recent survive, whatever the hash order.
        for t in 1..=97 {
            assert!(cache.lookup("chat", t).is_none(), "{t} should be evicted");
        }
        for t in 98..=100 {
            assert_eq!(cache.lookup("chat", t).unwrap().id, format!("v{t}"));
        }
        assert_eq!(cache.read().order.len(), 3);
        assert_eq!(cache.read().by_key.len(), 3);
    }

    #[test]
    fn remembering_a_known_note_updates_it_without_evicting() {
        let cache = VoiceAttachmentCache::new(2);
        cache.remember("chat", 1, sample_audio("old"));
        cache.remember("chat", 2, sample_audio("two"));
        cache.remember("chat", 1, sample_audio("new"));

        assert_eq!(cache.lookup("chat", 1).unwrap().id, "new");
        assert_eq!(cache.lookup("chat", 2).unwrap().id, "two");
        // Note 1 kept its original place, so it's the next to go.
        cache.remember("chat", 3, sample_audio("three"));
        assert!(cache.lookup("chat", 1).is_none());
        assert!(cache.lookup("chat", 2).is_some());
        assert!(cache.lookup("chat", 3).is_some());
    }

    #[test]
    fn capacity_is_at_least_one() {
        let cache = VoiceAttachmentCache::new(0);
        cache.remember("chat", 1, sample_audio("a"));
        cache.remember("chat", 2, sample_audio("b"));
        assert!(cache.lookup("chat", 1).is_none());
        assert_eq!(cache.lookup("chat", 2).unwrap().id, "b");
    }

    #[test]
    fn poisoned_lock_still_serves_the_cache() {
        let cache = VoiceAttachmentCache::new(2);
        cache.remember("chat", 1, sample_audio("a"));
        let poisoner = cache.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.entries.write().unwrap();
            panic!("poison the lock");
        })
        .join();
        assert!(cache.entries.is_poisoned());
        assert_eq!(cache.lookup("chat", 1).unwrap().id, "a");
        cache.remember("chat", 2, sample_audio("b"));
        assert_eq!(cache.lookup("chat", 2).unwrap().id, "b");
    }
}
