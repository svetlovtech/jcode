//! Persistent LRU cache for query embedding vectors.
//!
//! Dense memory retrieval embeds the retrieval query on every search. Unlike
//! stored memories (embedded once at write time and persisted on the entry),
//! the query embedding was recomputed on every call - a network round-trip on
//! every `find_similar` when the active backend is remote. The same question
//! recurs across sessions ("what does X do?"), so the vector is cached on
//! disk keyed by `(model_id, hashed query text)` and reused until evicted.
//!
//! Design:
//! - **Key = hash, not text.** Queries are user content; only a SHA-256 of
//!   `(model_id, formatted query text)` is persisted, so the cache file never
//!   contains the queries themselves.
//! - **LRU cap.** The cache holds at most [`QUERY_CACHE_MAX_ENTRIES`] vectors
//!   (~16 KB each at 4096 dims), bounding the file at ~30 MB in the worst
//!   case. Inserts evict the least-recently-used entry.
//! - **Atomic writes.** The cache is rewritten with
//!   `storage::write_json` (temp + rename) on every insert; a crash leaves
//!   the previous complete file, never a torn one. Saves are best-effort:
//!   a failed write only loses caching, retrieval still works (the caller
//!   falls back to computing the embedding).
//! - **Model-scoped.** The cache key includes the embedding model id, so
//!   switching backends (MiniLM ↔ OpenAI) never serves vectors from the
//!   wrong space. Old entries become dead weight and are naturally evicted
//!   by LRU.
//! - **test_mode isolation.** `MemoryManager` test mode keeps the cache in a
//!   `memory/test` subdirectory, mirroring how memory files themselves are
//!   isolated.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Maximum number of cached query vectors. At 4096-dim f32 vectors
/// serialized compactly this bounds the cache file well under 30 MB.
const QUERY_CACHE_MAX_ENTRIES: usize = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct CachedVector {
    vector: Vec<f32>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct QueryCacheFile {
    /// Keyed by hex(sha256(model_id \n formatted_query)). Insertion-order
    /// recency is tracked by `order`.
    vectors: HashMap<String, CachedVector>,
    /// LRU order, most recent last. Only keys present in `vectors`.
    order: Vec<String>,
}

struct QueryCacheState {
    file: QueryCacheFile,
    /// Set once a write fails, so a broken location does not retry on every
    /// query (retrieval must never stall on cache I/O).
    write_failed: bool,
}

/// Cache states keyed by resolved cache-file path. Per-path (rather than one
/// global) so the normal and `memory/test` scopes stay independent even
/// within one process, and a `JCODE_HOME` switch gets its own cache instead
/// of mixing files resolved under different homes.
static QUERY_CACHE: OnceLock<Mutex<HashMap<PathBuf, QueryCacheState>>> = OnceLock::new();

fn query_cache() -> &'static Mutex<HashMap<PathBuf, QueryCacheState>> {
    QUERY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Hex-encoded SHA-256 of `(model_id, formatted_query)`.
fn cache_key(model_id: &str, formatted_query: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(model_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(formatted_query.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Resolve the cache file path, honoring `JCODE_HOME` and the memory test
/// mode. Resolved on every call so env switches pick up the right file; the
/// loaded state itself is memoized per path in [`QUERY_CACHE`].
fn cache_path(test_mode: bool) -> Result<PathBuf> {
    let mut dir = jcode_storage::jcode_dir()?.join("memory");
    if test_mode {
        dir = dir.join("test");
    }
    Ok(dir.join("query_embeddings.json"))
}

/// Touch the LRU order for an existing key (no-op when absent).
fn touch_lru(order: &mut Vec<String>, key: &str) {
    if let Some(pos) = order.iter().position(|k| k == key) {
        let key = order.remove(pos);
        order.push(key);
    }
}

/// Evict least-recently-used entries until the map fits the cap.
fn evict_lru(file: &mut QueryCacheFile) {
    while file.vectors.len() > QUERY_CACHE_MAX_ENTRIES {
        let Some(oldest) = file.order.first().cloned() else {
            file.order.clear();
            break;
        };
        file.order.remove(0);
        file.vectors.remove(&oldest);
    }
    // Drop dangling order entries (defensive; should not happen).
    file.order.retain(|k| file.vectors.contains_key(k));
}

/// Look up a cached query vector. `None` = miss.
pub fn lookup(model_id: &str, formatted_query: &str, test_mode: bool) -> Option<Vec<f32>> {
    let key = cache_key(model_id, formatted_query);
    let path = cache_path(test_mode).ok()?;
    let Ok(mut states) = query_cache().lock() else {
        return None;
    };
    let state = states
        .entry(path.clone())
        .or_insert_with(|| QueryCacheState {
            file: load_file_for(&path),
            write_failed: false,
        });
    let hit = state.file.vectors.get(&key).map(|c| c.vector.clone());
    if hit.is_some() {
        touch_lru(&mut state.file.order, &key);
    }
    hit
}

/// Store a query vector. Never fails the caller: I/O errors are logged once
/// and caching is disabled for that path for the process lifetime.
pub fn store(model_id: &str, formatted_query: &str, vector: &[f32], test_mode: bool) {
    let key = cache_key(model_id, formatted_query);
    let Ok(path) = cache_path(test_mode) else {
        return;
    };
    let Ok(mut states) = query_cache().lock() else {
        return;
    };
    let state = states
        .entry(path.clone())
        .or_insert_with(|| QueryCacheState {
            file: load_file_for(&path),
            write_failed: false,
        });
    if state.write_failed {
        return;
    }
    state
        .file
        .vectors
        .insert(key.clone(), CachedVector { vector: vector.to_vec() });
    touch_lru(&mut state.file.order, &key);
    if !state.file.order.contains(&key) {
        state.file.order.push(key);
    }
    evict_lru(&mut state.file);
    if let Err(err) = jcode_storage::write_json(&path, &state.file) {
        state.write_failed = true;
        crate::logging::warn(&format!(
            "query embedding cache disabled after write failure: {err}"
        ));
    }
}

fn load_file_for(path: &std::path::Path) -> QueryCacheFile {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key(text: &str) -> String {
        cache_key("test-model", text)
    }

    #[test]
    fn cache_key_is_deterministic_and_model_scoped() {
        assert_eq!(test_key("hello"), test_key("hello"));
        assert_ne!(
            cache_key("model-a", "hello"),
            cache_key("model-b", "hello"),
            "different models must never share cache keys"
        );
    }

    #[test]
    fn evict_lru_respects_the_cap_and_order() {
        let mut file = QueryCacheFile::default();
        for i in 0..(QUERY_CACHE_MAX_ENTRIES + 10) {
            let key = format!("k{i:05}");
            file.vectors.insert(key.clone(), CachedVector { vector: vec![] });
            file.order.push(key);
        }
        evict_lru(&mut file);
        assert_eq!(file.vectors.len(), QUERY_CACHE_MAX_ENTRIES);
        // Oldest 10 were evicted, newest survive, order stays aligned.
        assert!(!file.vectors.contains_key("k00000"));
        assert!(file.vectors.contains_key(&format!("k{:05}", QUERY_CACHE_MAX_ENTRIES + 9)));
        assert_eq!(file.order.len(), file.vectors.len());
    }

    #[test]
    fn touch_lru_moves_key_to_back() {
        let mut order = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        touch_lru(&mut order, "a");
        assert_eq!(order, vec!["b".to_string(), "c".to_string(), "a".to_string()]);
        // Touching an absent key is a no-op.
        touch_lru(&mut order, "zzz");
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn store_then_lookup_roundtrips_and_persists_to_disk() {
        // Isolated JCODE_HOME: both the process-global cache state and the
        // cache file live under this temp dir for the test's lifetime.
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                if let Some(prev) = &self.0 {
                    crate::env::set_var("JCODE_HOME", prev);
                } else {
                    crate::env::remove_var("JCODE_HOME");
                }
                // Reset process-global state so later tests re-resolve the path.
                query_cache().lock().expect("cache lock").clear();
            }
        }
        let _restore = Restore(prev_home);

        let model = "test-model";
        let query = "Как называется сервис отвечающий за Telegram?";
        assert!(lookup(model, query, false).is_none(), "cold cache misses");

        store(model, query, &[0.5, -0.25, 1.0], false);

        // In-process hit without any disk reread.
        assert_eq!(lookup(model, query, false), Some(vec![0.5, -0.25, 1.0]));

        // Fresh process (reset state): the vector survives via the file.
        query_cache().lock().expect("cache lock").clear();
        assert_eq!(lookup(model, query, false), Some(vec![0.5, -0.25, 1.0]));
        let cache_file = temp.path().join("memory").join("query_embeddings.json");
        assert!(cache_file.exists(), "cache file must exist on disk");

        // The file must not contain the query text, only its hash.
        let raw = std::fs::read_to_string(&cache_file).expect("read cache file");
        assert!(
            !raw.contains("Telegram"),
            "cache file must not store query text: {raw}"
        );

        // A different model id is a different vector space: miss.
        assert!(lookup("other-model", query, false).is_none());
    }

    #[test]
    fn test_mode_uses_isolated_cache_file() {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                if let Some(prev) = &self.0 {
                    crate::env::set_var("JCODE_HOME", prev);
                } else {
                    crate::env::remove_var("JCODE_HOME");
                }
                query_cache().lock().expect("cache lock").clear();
            }
        }
        let _restore = Restore(prev_home);

        store("m", "query", &[1.0], true);
        let normal = temp.path().join("memory").join("query_embeddings.json");
        let test = temp
            .path()
            .join("memory")
            .join("test")
            .join("query_embeddings.json");
        assert!(!normal.exists(), "test-mode writes must not touch the real cache");
        assert!(test.exists(), "test-mode cache lives under memory/test");
        // And the scopes do not see each other.
        assert_eq!(lookup("m", "query", true), Some(vec![1.0]));
        assert_eq!(lookup("m", "query", false), None);
    }
}
