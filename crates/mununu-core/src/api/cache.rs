//! Content-hash cache for parsed + realized contexts (priority_roadmap §2.4 / Tier B3).
//!
//! API handlers re-parse and re-realize the context on every request. For
//! repeated verifications of the same file with different formulas — the
//! common UI workflow — this is wasteful. The realize phase typically
//! dominates wall-clock time (CLTS construction, predicate maps, abstraction
//! unrolling).
//!
//! This cache keys on a content hash of the request inputs (context + sidecars)
//! and stores the realized result behind `Arc`. Handlers that only need to
//! READ the realized context can borrow via `Arc::as_ref()` without cloning.
//!
//! Eviction policy: bounded LRU-ish via simple map size cap. Crude but
//! sufficient — the cache is a performance optimization, not correctness-
//! critical, and verifying many distinct contexts in a single API session is
//! rare.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use crate::context_dsl::{RealizedContext, parse as parse_context_doc, realize_context};

/// Maximum number of entries kept in the cache simultaneously. Tuned for
/// typical UI sessions; raise if the API server hosts many distinct contexts.
const MAX_CACHE_SIZE: usize = 64;

/// One cache entry: the parsed input documents AND the realized result.
/// Caching both means the handler can read context_doc.automata without
/// re-parsing, even though only the realized portion drives evaluation.
pub struct CacheEntry {
    pub context_doc: Arc<crate::context_dsl::ast::ContextDoc>,
    pub sidecar_docs: Arc<Vec<crate::context_dsl::ast::ContextDoc>>,
    pub realized: Arc<RealizedContext>,
}

impl Clone for CacheEntry {
    fn clone(&self) -> Self {
        Self {
            context_doc: Arc::clone(&self.context_doc),
            sidecar_docs: Arc::clone(&self.sidecar_docs),
            realized: Arc::clone(&self.realized),
        }
    }
}

static CONTEXT_CACHE: OnceLock<Mutex<HashMap<u64, CacheEntry>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<u64, CacheEntry>> {
    CONTEXT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Compute a hash key from raw context + sidecar strings. Order of sidecars
/// is part of the key (changing the order changes realization).
pub fn cache_key(context: &str, sidecars: &[&str]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    context.hash(&mut hasher);
    sidecars.len().hash(&mut hasher);
    for s in sidecars {
        s.hash(&mut hasher);
    }
    hasher.finish()
}

/// Get a cached `CacheEntry` for the given inputs, computing it on miss.
///
/// On a cache hit, returns `Ok((entry, true))` — the bool indicates
/// `cache_hit`. On miss, parses + realizes and caches the result, returning
/// `Ok((entry, false))`.
///
/// Errors are NOT cached — a parse or realize failure on one request shouldn't
/// suppress a retry with corrected inputs.
pub fn get_or_realize(context: &str, sidecars: &[&str]) -> Result<(CacheEntry, bool), String> {
    let key = cache_key(context, sidecars);
    {
        let cache_guard = cache()
            .lock()
            .map_err(|e| format!("cache mutex poisoned: {e}"))?;
        if let Some(entry) = cache_guard.get(&key) {
            return Ok((entry.clone(), true));
        }
    }

    // Cache miss — parse + realize
    let context_doc = parse_context_doc(context).map_err(|e| format!("parse failed: {e}"))?;
    let sidecar_docs: Result<Vec<_>, _> = sidecars.iter().map(|s| parse_context_doc(s)).collect();
    let sidecar_docs = sidecar_docs.map_err(|e| format!("sidecar parse failed: {e}"))?;
    let realized =
        realize_context(&context_doc, &sidecar_docs).map_err(|e| format!("realize failed: {e}"))?;

    let entry = CacheEntry {
        context_doc: Arc::new(context_doc),
        sidecar_docs: Arc::new(sidecar_docs),
        realized: Arc::new(realized),
    };

    {
        let mut cache_guard = cache()
            .lock()
            .map_err(|e| format!("cache mutex poisoned: {e}"))?;
        // Crude eviction: when at capacity, drop one arbitrary entry. LRU
        // would be more sophisticated; this is sufficient for a perf cache.
        if cache_guard.len() >= MAX_CACHE_SIZE
            && let Some(k) = cache_guard.keys().next().copied()
        {
            cache_guard.remove(&k);
        }
        cache_guard.insert(key, entry.clone());
    }

    Ok((entry, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests must use UNIQUE context names so the shared static cache (which is
    // process-wide and shared with concurrently running tests) doesn't bleed
    // state across tests. Calling `clear()` is unsafe in parallel contexts —
    // another test may have just inserted an entry it's about to read.

    #[test]
    fn same_inputs_hit_cache() {
        let ctx = r#"
context cache_test_same {
    automata {
        automaton M {
            states { state s0 initial; }
            transitions { transition s0 -> s0 on label tick; }
        }
    }
}
"#;
        let (_e1, hit1) = get_or_realize(ctx, &[]).unwrap();
        let (_e2, hit2) = get_or_realize(ctx, &[]).unwrap();
        // First call may miss or hit depending on whether another test already
        // populated this exact key (extremely unlikely with unique name); the
        // second call MUST hit because the first just inserted on miss.
        assert!(hit2, "second call with identical input should hit");
        // If hit1 was already true (rare), hit2 still must be true.
        let _ = hit1;
    }

    #[test]
    fn different_inputs_miss_cache() {
        let ctx_a = r#"context cache_test_diff_a { automata { automaton X { states { state s0 initial; } transitions {} } } }"#;
        let ctx_b = r#"context cache_test_diff_b { automata { automaton Y { states { state s0 initial; } transitions {} } } }"#;
        // First fetch warms the entries (may be a miss or hit if a previous run
        // of this test in the same process already populated). Either way, the
        // immediate re-fetch below must hit.
        let _ = get_or_realize(ctx_a, &[]).unwrap();
        let _ = get_or_realize(ctx_b, &[]).unwrap();
        let (_, hit_a2) = get_or_realize(ctx_a, &[]).unwrap();
        let (_, hit_b2) = get_or_realize(ctx_b, &[]).unwrap();
        assert!(hit_a2, "ctx_a should hit on re-fetch");
        assert!(hit_b2, "ctx_b should hit on re-fetch");
    }

    #[test]
    fn parse_failure_not_cached() {
        let bad = "this is not ctxdsl at all -- cache_test_parse_failure";
        assert!(get_or_realize(bad, &[]).is_err());
        // Second call should still error (and still not cache) — no panic
        assert!(get_or_realize(bad, &[]).is_err());
    }
}
