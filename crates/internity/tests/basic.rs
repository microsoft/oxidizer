// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the public `Lexicon` API.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
#[cfg(not(all(miri, windows)))]
use std::sync::Arc;
#[cfg(not(all(miri, windows)))]
use std::thread;

use internity::{Lexicon, Reader, Sym, SymBuildHasher, SymMap, SymSet, ThreadedLexicon};

#[test]
fn sym_option_niche_is_free() {
    assert_eq!(core::mem::size_of::<Sym>(), core::mem::size_of::<Option<Sym>>());
    assert_eq!(core::mem::size_of::<Sym>(), 4);
}

#[test]
fn sym_as_u32_roundtrips_and_debug() {
    let mut it = Lexicon::new();
    let a = it.intern("alpha");
    let raw = a.as_u32();
    assert_ne!(raw, 0);
    assert_eq!(Sym::from_u32(raw), Some(a));
    assert_eq!(Sym::from_u32(0), None);
    // `From<Sym> for u32` matches the inherent `as_u32`.
    assert_eq!(u32::from(a), raw);
    // `Debug` renders the raw handle value.
    assert!(format!("{a:?}").contains("Sym"));
}

#[test]
fn lexicon_and_threaded_debug() {
    let mut it = Lexicon::new();
    it.intern("a");
    it.intern("b");
    let s = format!("{it:?}");
    assert!(s.contains("Lexicon"), "{s}");
    assert!(s.contains("len"), "{s}");

    let t = ThreadedLexicon::new();
    t.intern("a");
    let s = format!("{t:?}");
    assert!(s.contains("ThreadedLexicon"), "{s}");
    assert!(s.contains("len"), "{s}");
}

#[test]
fn from_iter_and_extend() {
    // `FromIterator` builds an interner from strings.
    let mut it: Lexicon = ["a", "b", "a", "c"].into_iter().collect();
    assert_eq!(it.len(), 3);
    // `Extend` interns more.
    it.extend(["c", "d"]);
    assert_eq!(it.len(), 4);

    let t: ThreadedLexicon = vec!["x".to_string(), "y".to_string(), "x".to_string()].into_iter().collect();
    assert_eq!(t.len(), 2);
    let mut t = t;
    t.extend(["y", "z"]);
    assert_eq!(t.len(), 3);
}

#[test]
fn lexicon_default_and_is_empty() {
    let mut it = Lexicon::default();
    assert!(it.is_empty());
    assert_eq!(it.len(), 0);
    it.intern("x");
    assert!(!it.is_empty());
    assert_eq!(it.len(), 1);
}

#[test]
fn intern_accepts_owned_strings() {
    let mut lexicon = Lexicon::new();
    let lexicon_sym = lexicon.intern(String::from("owned"));
    assert_eq!(lexicon.resolve(lexicon_sym), "owned");

    let threaded = ThreadedLexicon::new();
    let threaded_sym = threaded.intern(String::from("owned"));
    assert_eq!(threaded.get("owned"), Some(threaded_sym));
}

#[test]
fn threaded_default_with_hasher_get_and_is_empty() {
    let it = ThreadedLexicon::default();
    assert!(it.is_empty());
    assert_eq!(it.get("nope"), None);
    let a = it.intern("hello");
    assert!(!it.is_empty());
    assert_eq!(it.get("hello"), Some(a));

    // A non-default `BuildHasher` via `with_hasher`; exercise the full API on this
    // second monomorphization so every instantiated method is covered.
    let custom = ThreadedLexicon::with_hasher(RandomState::new());
    assert!(custom.is_empty());
    let k = custom.intern("k");
    let custom_clone = custom.clone();
    assert_eq!(custom_clone.intern("k"), k);
    assert_eq!(custom.get("k"), Some(k));
    assert_eq!(custom.get("missing"), None);
    assert_eq!(custom.len(), 1);
    assert!(!custom.is_empty());
    let reader = custom.freeze();
    assert_eq!(reader.resolve(k), "k");
}

#[test]
fn reader_is_empty_and_len() {
    let empty = Lexicon::new().freeze();
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);

    let mut it = Lexicon::new();
    it.intern("a");
    let reader = it.freeze();
    assert!(!reader.is_empty());
    assert_eq!(reader.len(), 1);
}

#[test]
fn freeze_preserves_handles_and_strings() {
    let mut it = Lexicon::new();
    let syms: Vec<(Sym, String)> = (0..5000)
        .map(|i| {
            let s = format!("frozen-symbol-{i:07}");
            (it.intern(&s), s)
        })
        .collect();
    let n = it.len();
    let reader = it.freeze();
    assert_eq!(reader.len(), n);
    // Every handle from the live lexicon still resolves to the same string.
    for (sym, s) in &syms {
        assert_eq!(reader.resolve(*sym), s.as_str());
    }
    // Out-of-range handle is range-checked, not UB.
    assert_eq!(reader.try_resolve(Sym::from_u32(u32::MAX).unwrap()), None);
}

#[test]
fn dedup_returns_same_handle() {
    let mut it = Lexicon::new();
    let a = it.intern("hello");
    let b = it.intern("hello");
    assert_eq!(a, b);
    assert_eq!(it.len(), 1);
}

#[test]
fn distinct_strings_distinct_handles() {
    let mut it = Lexicon::new();
    let a = it.intern("hello");
    let b = it.intern("world");
    assert_ne!(a, b);
    assert_eq!(it.resolve(a), "hello");
    assert_eq!(it.resolve(b), "world");
    assert_eq!(it.len(), 2);
}

#[test]
fn empty_string_roundtrips() {
    let mut it = Lexicon::new();
    let e = it.intern("");
    assert_eq!(it.resolve(e), "");
    assert_eq!(it.intern(""), e);
}

#[test]
fn get_does_not_intern() {
    let mut it = Lexicon::new();
    assert_eq!(it.get("nope"), None);
    let s = it.intern("yep");
    assert_eq!(it.get("yep"), Some(s));
    assert_eq!(it.get("nope"), None);
}

#[test]
fn many_strings_across_chunks() {
    let mut it = Lexicon::new();
    #[cfg(miri)]
    let count = 2_000;
    #[cfg(not(miri))]
    let count = 50_000;

    let mut syms = Vec::new();
    // Enough long strings to force multiple byte chunks per shard.
    for i in 0..count {
        let s = format!("symbol-number-{i:08}-with-some-padding");
        syms.push((it.intern(&s), s));
    }
    // Re-interning yields identical handles; resolve returns the right bytes.
    for (sym, s) in &syms {
        assert_eq!(it.intern(s), *sym);
        assert_eq!(it.resolve(*sym), s.as_str());
    }
    assert_eq!(it.len(), count);
}

#[test]
fn foreign_handle_is_range_checked_not_ub() {
    let mut a = Lexicon::new();
    let _ = a.intern("only");
    // A handle with a valid shard but an out-of-range local index resolves to
    // None rather than causing UB.
    let bogus = Sym::from_u32(u32::MAX).unwrap();
    assert_eq!(a.try_resolve(bogus), None);
}

#[test]
fn freeze_while_shared_copies_and_preserves_handles() {
    // Two live handles to the same interner force `freeze` down the copying
    // (`build_reader`) path rather than the sole-owner move path.
    let it = ThreadedLexicon::new();
    let other = it.clone();
    let a = it.intern("alpha");
    let b = it.intern("beta");

    let reader = it.freeze(); // `other` still alive → copy path
    assert_eq!(reader.resolve(a), "alpha");
    assert_eq!(reader.resolve(b), "beta");
    assert_eq!(reader.len(), 2);
    // A foreign/out-of-range handle on the frozen sharded reader is range-checked.
    assert_eq!(reader.try_resolve(Sym::from_u32(u32::MAX).unwrap()), None);

    // The surviving handle is unaffected and still interns.
    assert_eq!(other.get("alpha"), Some(a));
    assert_ne!(other.intern("gamma"), a);
}

#[test]
#[cfg(not(all(miri, windows)))]
fn concurrent_intern_is_consistent() {
    let it = ThreadedLexicon::new();
    #[cfg(miri)]
    let (n_threads, n_strings, distinct) = (3, 300, 50usize);
    #[cfg(not(miri))]
    let (n_threads, n_strings, distinct) = (8, 5_000, 1_000usize);

    // All threads intern the same set of strings; every thread must agree on the
    // handle for each string, and each string must have exactly one handle.
    #[expect(clippy::needless_collect, reason = "all workers must be spawned before any are joined")]
    let handles: Vec<_> = (0..n_threads)
        .map(|_| {
            let it = it.clone();
            thread::spawn(move || {
                let mut local = HashMap::new();
                for i in 0..n_strings {
                    let s = format!("shared-{}", i % distinct);
                    let sym = it.intern(&s);
                    local.insert(s, sym);
                }
                local
            })
        })
        .collect();

    let maps: Vec<HashMap<String, Sym>> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    // Cross-check: every thread saw the same handle for a given string.
    let first = &maps[0];
    for m in &maps[1..] {
        for (k, v) in m {
            assert_eq!(first.get(k), Some(v), "handle mismatch for {k:?}");
        }
    }

    // Exactly `distinct` distinct strings were interned.
    assert_eq!(it.len(), distinct);

    // Freeze (sole handle after joins), then resolve every handle to its string.
    let reader = it.freeze();
    for (k, v) in first {
        assert_eq!(reader.resolve(*v), k.as_str());
    }
}

#[test]
#[cfg(not(all(miri, windows)))]
fn concurrent_intern_then_concurrent_resolve() {
    let it = ThreadedLexicon::new();
    #[cfg(miri)]
    let (n_threads, distinct, read_iters) = (3, 100usize, 20);
    #[cfg(not(miri))]
    let (n_threads, distinct, read_iters) = (8, 5_000usize, 50);

    // Fill phase: every thread interns the same set concurrently.
    let writers: Vec<_> = (0..n_threads)
        .map(|_| {
            let it = it.clone();
            thread::spawn(move || {
                for i in 0..distinct {
                    it.intern(format!("word-{i}"));
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    assert_eq!(it.len(), distinct);

    // Capture each string's handle, then freeze for the read phase.
    let syms: Vec<Sym> = (0..distinct)
        .map(|i| {
            it.get(&format!("word-{i}"))
                .expect("every word was interned before all writer threads joined")
        })
        .collect();
    let reader = Arc::new(it.freeze());

    // Read phase: many threads resolve the frozen reader concurrently.
    let readers: Vec<_> = (0..n_threads)
        .map(|_| {
            let reader = Arc::clone(&reader);
            let syms = syms.clone();
            thread::spawn(move || {
                for _ in 0..read_iters {
                    for (i, sym) in syms.iter().enumerate() {
                        assert_eq!(reader.resolve(*sym), format!("word-{i}"));
                    }
                }
            })
        })
        .collect();
    for r in readers {
        r.join().unwrap();
    }
}

#[test]
fn lexicon_iter_yields_pairs_in_order() {
    let mut it = Lexicon::new();
    let a = it.intern("a");
    let b = it.intern("bb");
    let c = it.intern("ccc");
    let pairs: Vec<_> = it.iter().collect();
    assert_eq!(pairs, vec![(a, "a"), (b, "bb"), (c, "ccc")]);

    // The frozen reader iterates the same pairs.
    let reader = it.freeze();
    let mut got: Vec<_> = reader.iter().collect();
    got.sort_by_key(|&(s, _)| s.as_u32());
    assert_eq!(got, vec![(a, "a"), (b, "bb"), (c, "ccc")]);
}

#[test]
fn threaded_reader_iter_roundtrips() {
    let it = ThreadedLexicon::new();
    let words = ["alpha", "beta", "gamma", "delta"];
    for w in words {
        it.intern(w);
    }
    let reader = it.freeze();
    let mut got: Vec<String> = reader.iter().map(|(_, s)| s.to_string()).collect();
    got.sort();
    let mut expect: Vec<String> = words.iter().map(|s| (*s).to_string()).collect();
    expect.sort();
    assert_eq!(got, expect);
    // Every yielded handle resolves back to its string.
    for (sym, s) in reader.iter() {
        assert_eq!(reader.resolve(sym), s);
    }
}

#[test]
fn sym_map_and_set() {
    let mut it = Lexicon::new();
    let a = it.intern("a");
    let b = it.intern("b");

    let mut map: SymMap<i32> = SymMap::default();
    map.insert(a, 1);
    map.insert(b, 2);
    assert_eq!(map.get(&a), Some(&1));
    assert_eq!(map.get(&b), Some(&2));

    let mut set: SymSet = SymSet::default();
    assert!(set.insert(a));
    assert!(!set.insert(a));
    assert!(set.contains(&a));
    assert!(!set.contains(&b));
}

#[cfg(feature = "serde")]
#[test]
fn serde_sym_roundtrips() {
    let mut it = Lexicon::new();
    let a = it.intern("hello");
    let json = serde_json::to_string(&a).unwrap();
    assert_eq!(json, a.as_u32().to_string());
    let back: Sym = serde_json::from_str(&json).unwrap();
    assert_eq!(back, a);
    // Zero is not a valid Sym.
    serde_json::from_str::<Sym>("0").unwrap_err();
    // A non-integer is a deserialize error.
    serde_json::from_str::<Sym>("\"x\"").unwrap_err();
}

#[cfg(feature = "serde")]
#[test]
fn serde_lexicon_roundtrips_handles() {
    let mut it = Lexicon::new();
    let syms: Vec<(Sym, String)> = ["a", "bb", "ccc", "a"].iter().map(|s| (it.intern(s), s.to_string())).collect();
    let json = serde_json::to_string(&it).unwrap();
    let it2: Lexicon = serde_json::from_str(&json).unwrap();
    assert_eq!(it2.len(), 3);
    // Handles are reproduced identically, resolving to the same strings.
    for (sym, s) in &syms {
        assert_eq!(it2.resolve(*sym), s.as_str());
    }
}

#[cfg(feature = "serde")]
#[test]
fn serde_threaded_roundtrips_handles() {
    let it = ThreadedLexicon::new();
    let words = ["one", "two", "three", "four", "five"];
    let syms: Vec<(Sym, &str)> = words.iter().map(|w| (it.intern(w), *w)).collect();
    let json = serde_json::to_string(&it).unwrap();
    let it2: ThreadedLexicon = serde_json::from_str(&json).unwrap();
    assert_eq!(it2.len(), words.len());
    for (sym, s) in &syms {
        assert_eq!(it2.get(s), Some(*sym), "handle mismatch for {s}");
    }
}

#[test]
fn sym_hasher_write_fallback_is_deterministic() {
    use core::hash::{BuildHasher, Hasher};

    let bh = SymBuildHasher::default();
    let hash = |bytes: &[u8]| {
        let mut h = bh.build_hasher();
        h.write(bytes);
        h.finish()
    };
    assert_eq!(hash(b"arbitrary"), hash(b"arbitrary"));
    assert_ne!(hash(b"arbitrary"), hash(b"different"));
}

#[cfg(feature = "serde")]
#[test]
fn serde_lexicon_rejects_non_sequence() {
    let error = serde_json::from_str::<Lexicon>("42").unwrap_err();
    assert!(error.to_string().contains("a sequence of interned strings"));
    // A non-string element mid-sequence is an error.
    serde_json::from_str::<Lexicon>("[\"a\", 42]").unwrap_err();
}

#[test]
fn foreign_sym_resolves_to_none_without_panicking() {
    // A frozen `ThreadedLexicon` reader must treat handles it never issued as
    // out of range (return `None`) rather than panicking — even for crafted
    // handles whose per-shard local bits are zero, which previously underflowed
    // the local-index decode in debug builds.
    let it = ThreadedLexicon::new();
    let real = it.intern("hello");
    let reader = it.freeze();

    assert_eq!(reader.resolve(real), "hello");

    // Shard 1, all local bits zero (`1 << LOCAL_BITS`): a non-zero handle whose
    // low 26 bits are 0.
    let zero_local = Sym::from_u32(1u32 << 26).unwrap();
    assert_eq!(reader.try_resolve(zero_local), None);

    // All bits set: valid shard, but a local index far past the shard's length.
    let past_end = Sym::from_u32(u32::MAX).unwrap();
    assert_eq!(reader.try_resolve(past_end), None);
}

#[test]
fn with_capacity_preallocates_and_interns() {
    let mut lexicon = Lexicon::with_capacity(128, 128 * 8);
    assert!(lexicon.is_empty());
    assert_eq!(lexicon.len(), 0);

    let a = lexicon.intern("hello");
    let b = lexicon.intern("world");
    assert_eq!(lexicon.intern("hello"), a); // dedup still works
    assert_ne!(a, b);
    assert_eq!(lexicon.resolve(a), "hello");
    assert_eq!(lexicon.resolve(b), "world");
    assert_eq!(lexicon.len(), 2);
}

#[test]
fn with_capacity_and_hasher_uses_given_hasher() {
    let mut lexicon = Lexicon::with_capacity_and_hasher(16, 256, RandomState::new());
    let a = lexicon.intern("alpha");
    assert_eq!(lexicon.intern("alpha"), a);
    assert_eq!(lexicon.resolve(a), "alpha");
}

#[test]
fn with_capacity_zero_is_valid() {
    let mut lexicon = Lexicon::with_capacity(0, 0);
    let a = lexicon.intern("x");
    assert_eq!(lexicon.resolve(a), "x");
}
