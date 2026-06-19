//! CCR — Compress-Cache-Retrieve store.
//!
//! When a compressor drops data (lossy paths), it stows the original here
//! keyed by a short content hash and emits a `retrieve_tool_output("<hash>")`
//! sentinel in the compacted text. The agent can call the
//! `retrieve_tool_output` tool to get the original back on demand — so even
//! aggressive compaction stays reversible and is safe under the always-on
//! default.
//!
//! Process-global and bounded: a fixed-capacity FIFO so a long session can't
//! grow it without bound. Keyed by content hash, so re-offloading identical
//! content is idempotent (the model sees a stable hash). Originals are not
//! persisted to disk — retrieval is best-effort within the session; an evicted
//! entry simply reports "no longer available", which is strictly better than
//! the pre-CCR behaviour (the data was gone the moment it was truncated).

use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

/// Max originals retained. ~256 large tool outputs is plenty for a session's
/// recent history while bounding worst-case memory.
const MAX_ENTRIES: usize = 256;
/// Bytes of the SHA-256 digest used for the key (→ 12 hex chars). Collision
/// risk at this length over a few hundred live entries is negligible.
const HASH_BYTES: usize = 6;

struct Inner {
    map: HashMap<String, String>,
    order: VecDeque<String>,
}

fn global() -> &'static Mutex<Inner> {
    static STORE: OnceLock<Mutex<Inner>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(Inner {
            map: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

/// Stash `content` and return its short hash. Idempotent for identical content.
pub fn offload(content: &str) -> String {
    let hash = short_hash(content);
    let mut inner = global().lock().unwrap_or_else(|p| p.into_inner());
    if !inner.map.contains_key(&hash) {
        inner.map.insert(hash.clone(), content.to_string());
        inner.order.push_back(hash.clone());
        while inner.order.len() > MAX_ENTRIES {
            if let Some(evicted) = inner.order.pop_front() {
                inner.map.remove(&evicted);
            }
        }
    }
    hash
}

/// Retrieve a previously-offloaded original by hash, if still cached.
pub fn retrieve(hash: &str) -> Option<String> {
    global()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .map
        .get(hash)
        .cloned()
}

/// Short hex content hash used as the CCR key.
pub fn short_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..HASH_BYTES])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let original = "the quick brown fox ".repeat(50);
        let hash = offload(&original);
        assert_eq!(hash.len(), HASH_BYTES * 2);
        assert_eq!(retrieve(&hash).as_deref(), Some(original.as_str()));
    }

    #[test]
    fn idempotent_hash() {
        let a = offload("identical payload content here");
        let b = offload("identical payload content here");
        assert_eq!(a, b);
    }

    #[test]
    fn missing_hash_is_none() {
        assert!(retrieve("ffffffffffff").is_none() || retrieve("000000000000").is_none());
    }

    #[test]
    fn eviction_bounds_size() {
        // Offload more than capacity; the earliest must eventually evict.
        let first = offload(&format!("entry-{}", 0));
        for i in 1..(MAX_ENTRIES + 50) {
            offload(&format!("entry-{i}"));
        }
        assert!(retrieve(&first).is_none(), "oldest entry should evict");
    }
}
