// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the public `LocalLexicon` API.

#[cfg(not(all(miri, windows)))]
use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(all(miri, windows)))]
use std::thread;

use internity::{Lexicon, LocalLexicon, Reader, Sym, SymBuildHasher, SymMap, SymSet, ThreadedLexicon};

#[derive(Clone)]
struct PanicOnMarkerBuildHasher {
    armed: Arc<AtomicBool>,
}

struct PanicOnMarkerHasher {
    armed: Arc<AtomicBool>,
}

impl BuildHasher for PanicOnMarkerBuildHasher {
    type Hasher = PanicOnMarkerHasher;

    fn build_hasher(&self) -> Self::Hasher {
        PanicOnMarkerHasher {
            armed: Arc::clone(&self.armed),
        }
    }
}

impl Hasher for PanicOnMarkerHasher {
    fn finish(&self) -> u64 {
        0
    }

    fn write(&mut self, bytes: &[u8]) {
        assert!(
            !(self.armed.load(Ordering::Relaxed) && bytes == b"marker"),
            "deliberate rehash panic"
        );
    }
}

fn panic_on_marker_hasher() -> (PanicOnMarkerBuildHasher, Arc<AtomicBool>) {
    let armed = Arc::new(AtomicBool::new(false));
    (PanicOnMarkerBuildHasher { armed: Arc::clone(&armed) }, armed)
}

#[test]
fn sym_option_niche_is_free() {
    assert_eq!(core::mem::size_of::<Sym>(), core::mem::size_of::<Option<Sym>>());
    assert_eq!(core::mem::size_of::<Sym>(), 4);
}

#[test]
fn sym_as_u32_roundtrips_and_debug() {
    let mut it = LocalLexicon::new();
    let a = it.intern("alpha");
    let raw = a.as_u32();
    assert_ne!(raw, 0);
    assert_eq!(Sym::from_u32(raw), Some(a));
    assert_eq!(Sym::from_u32(0), None);
    assert_eq!(u32::from(a), raw);
    assert!(format!("{a:?}").contains("Sym"));
}

#[test]
fn lexicon_and_threaded_debug() {
    let mut it = LocalLexicon::new();
    it.intern("a");
    it.intern("b");
    let s = format!("{it:?}");
    assert!(s.contains("LocalLexicon"), "{s}");
    assert!(s.contains("len"), "{s}");

    let t = ThreadedLexicon::new();
    t.intern("a");
    let s = format!("{t:?}");
    assert!(s.contains("ThreadedLexicon"), "{s}");
    assert!(s.contains("len"), "{s}");
}

#[test]
fn from_iter_and_extend() {
    let mut it: LocalLexicon = ["a", "b", "a", "c"].into_iter().collect();
    assert_eq!(it.len(), 3);
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
    let mut it = LocalLexicon::default();
    assert!(it.is_empty());
    assert_eq!(it.len(), 0);
    it.intern("x");
    assert!(!it.is_empty());
    assert_eq!(it.len(), 1);
}

#[test]
fn intern_accepts_owned_strings() {
    let mut lexicon = LocalLexicon::new();
    let lexicon_sym = lexicon.intern(String::from("owned"));
    assert_eq!(lexicon.resolve(lexicon_sym), "owned");
    assert_eq!(lexicon.get(String::from("owned")), Some(lexicon_sym));

    let threaded = ThreadedLexicon::new();
    let threaded_sym = threaded.intern(String::from("owned"));
    assert_eq!(threaded.get(String::from("owned")), Some(threaded_sym));
}

#[test]
fn lexicon_trait_abstracts_over_engines() {
    fn intern_name(lexicon: &mut impl Lexicon) -> Sym {
        lexicon.intern("generic")
    }

    let mut local = LocalLexicon::new();
    let local_sym = intern_name(&mut local);
    assert_eq!(local.resolve(local_sym), "generic");

    let mut threaded = ThreadedLexicon::new();
    let threaded_sym = intern_name(&mut threaded);
    assert_eq!(threaded.get("generic"), Some(threaded_sym));
}

#[test]
fn lexicon_trait_supports_dynamic_dispatch() {
    let lexicons: [Box<dyn Lexicon>; 2] = [Box::new(LocalLexicon::new()), Box::new(ThreadedLexicon::new())];

    for mut lexicon in lexicons {
        assert!(lexicon.is_empty());

        let sym = lexicon.intern("dynamic");
        assert_eq!(lexicon.get("dynamic"), Some(sym));
        assert_eq!(lexicon.len(), 1);
        assert!(!lexicon.is_empty());

        let reader = lexicon.freeze();
        assert_eq!(reader.resolve(sym), "dynamic");
    }
}

#[test]
fn local_lexicon_implements_reader() {
    fn resolve_generic(reader: &impl Reader, sym: Sym) -> &str {
        reader.resolve(sym)
    }

    let mut lexicon = LocalLexicon::new();
    let sym = lexicon.intern("readable");
    let other = lexicon.intern("other");
    assert_eq!(resolve_generic(&lexicon, sym), "readable");
    assert_eq!(Reader::len(&lexicon), 2);
    assert_eq!(Reader::iter(&lexicon).collect::<Vec<_>>(), [(sym, "readable"), (other, "other")]);
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
    let empty = LocalLexicon::new().freeze();
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);

    let mut it = LocalLexicon::new();
    it.intern("a");
    let reader = it.freeze();
    assert!(!reader.is_empty());
    assert_eq!(reader.len(), 1);
}

#[test]
fn freeze_preserves_handles_and_strings() {
    let mut it = LocalLexicon::new();
    let syms: Vec<(Sym, String)> = (0..5000)
        .map(|i| {
            let s = format!("frozen-symbol-{i:07}");
            (it.intern(&s), s)
        })
        .collect();
    let n = it.len();
    let reader = it.freeze();
    assert_eq!(reader.len(), n);
    for (sym, s) in &syms {
        assert_eq!(reader.resolve(*sym), s.as_str());
    }
    // Out-of-range handle is range-checked, not UB.
    assert_eq!(reader.try_resolve(Sym::from_u32(u32::MAX).unwrap()), None);
}

#[test]
fn dedup_returns_same_handle() {
    let mut it = LocalLexicon::new();
    let a = it.intern("hello");
    let b = it.intern("hello");
    assert_eq!(a, b);
    assert_eq!(it.len(), 1);
}

#[test]
fn local_rehash_panic_leaves_lexicon_consistent() {
    let (hasher, armed) = panic_on_marker_hasher();
    let mut lexicon = LocalLexicon::with_hasher(hasher);
    let marker = lexicon.intern("marker");

    armed.store(true, Ordering::Relaxed);
    let mut observed_panic = false;
    for index in 0..64 {
        let before = lexicon.len();
        let candidate = format!("candidate-{index}");
        if catch_unwind(AssertUnwindSafe(|| lexicon.intern(&candidate))).is_err() {
            assert_eq!(lexicon.len(), before);
            observed_panic = true;
            break;
        }
    }
    assert!(observed_panic, "the table must grow within the bounded insertion loop");

    armed.store(false, Ordering::Relaxed);
    assert_eq!(lexicon.get("marker"), Some(marker));
    assert_eq!(lexicon.resolve(marker), "marker");
    let recovered = lexicon.intern("recovered");
    assert_eq!(lexicon.resolve(recovered), "recovered");
}

#[test]
fn threaded_rehash_panic_leaves_lexicon_consistent() {
    let (hasher, armed) = panic_on_marker_hasher();
    let lexicon = ThreadedLexicon::with_hasher(hasher);
    let marker = lexicon.intern("marker");

    armed.store(true, Ordering::Relaxed);
    let mut observed_panic = false;
    for index in 0..64 {
        let before = lexicon.len();
        let candidate = format!("candidate-{index}");
        if catch_unwind(AssertUnwindSafe(|| lexicon.intern(&candidate))).is_err() {
            assert_eq!(lexicon.len(), before);
            observed_panic = true;
            break;
        }
    }
    assert!(observed_panic, "the table must grow within the bounded insertion loop");

    armed.store(false, Ordering::Relaxed);
    assert_eq!(lexicon.get("marker"), Some(marker));
    let reader = lexicon.freeze();
    assert_eq!(reader.resolve(marker), "marker");
}

#[test]
fn distinct_strings_distinct_handles() {
    let mut it = LocalLexicon::new();
    let a = it.intern("hello");
    let b = it.intern("world");
    assert_ne!(a, b);
    assert_eq!(it.resolve(a), "hello");
    assert_eq!(it.resolve(b), "world");
    assert_eq!(it.len(), 2);
}

#[test]
fn empty_string_roundtrips() {
    let mut it = LocalLexicon::new();
    let e = it.intern("");
    assert_eq!(it.resolve(e), "");
    assert_eq!(it.intern(""), e);
}

#[test]
fn get_does_not_intern() {
    let mut it = LocalLexicon::new();
    assert_eq!(it.get("nope"), None);
    let s = it.intern("yep");
    assert_eq!(it.get("yep"), Some(s));
    assert_eq!(it.get("nope"), None);
}

#[test]
fn many_strings_across_chunks() {
    let mut it = LocalLexicon::new();
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
    for (sym, s) in &syms {
        assert_eq!(it.intern(s), *sym);
        assert_eq!(it.resolve(*sym), s.as_str());
    }
    assert_eq!(it.len(), count);
}

#[test]
fn foreign_handle_is_range_checked_not_ub() {
    let mut a = LocalLexicon::new();
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
    assert_eq!(reader.try_resolve(Sym::from_u32(u32::MAX).unwrap()), None);

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

    let first = &maps[0];
    for m in &maps[1..] {
        for (k, v) in m {
            assert_eq!(first.get(k), Some(v), "handle mismatch for {k:?}");
        }
    }

    assert_eq!(it.len(), distinct);

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
    let words = Arc::new((0..distinct).map(|i| format!("word-{i}")).collect::<Vec<_>>());

    let writers: Vec<_> = (0..n_threads)
        .map(|_| {
            let it = it.clone();
            let words = Arc::clone(&words);
            thread::spawn(move || {
                for word in words.iter() {
                    it.intern(word);
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    assert_eq!(it.len(), distinct);

    let syms: Vec<Sym> = words
        .iter()
        .map(|word| it.get(word).expect("every word was interned before all writer threads joined"))
        .collect();
    let reader = Arc::new(it.freeze());

    let readers: Vec<_> = (0..n_threads)
        .map(|_| {
            let reader = Arc::clone(&reader);
            let syms = syms.clone();
            let words = Arc::clone(&words);
            thread::spawn(move || {
                for _ in 0..read_iters {
                    for (sym, word) in syms.iter().zip(words.iter()) {
                        assert_eq!(reader.resolve(*sym), word.as_str());
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
    let mut it = LocalLexicon::new();
    let a = it.intern("a");
    let b = it.intern("bb");
    let c = it.intern("ccc");
    let pairs: Vec<_> = it.iter().collect();
    assert_eq!(pairs, vec![(a, "a"), (b, "bb"), (c, "ccc")]);

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
    for (sym, s) in reader.iter() {
        assert_eq!(reader.resolve(sym), s);
    }
}

#[test]
fn sym_map_and_set() {
    let mut it = LocalLexicon::new();
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
fn serde_lexicon_roundtrips_handles() {
    let mut it = LocalLexicon::new();
    let syms: Vec<(Sym, String)> = ["a", "bb", "ccc", "a"].iter().map(|s| (it.intern(s), s.to_string())).collect();
    let json = serde_json::to_string(&it).unwrap();
    let it2: LocalLexicon = serde_json::from_str(&json).unwrap();
    assert_eq!(it2.len(), 3);
    for (sym, s) in &syms {
        assert_eq!(it2.resolve(*sym), s.as_str());
    }
}

#[cfg(feature = "serde")]
#[test]
fn serde_threaded_roundtrips_handles() {
    use internity::se::SerializeReader;

    let it = ThreadedLexicon::new();
    let words = ["one", "two", "three", "four", "five"];
    let syms: Vec<(Sym, &str)> = words.iter().map(|w| (it.intern(w), *w)).collect();
    // A live `ThreadedLexicon` is not serialized directly: freeze it to take a
    // point-in-time snapshot, then serialize the resulting `Reader`.
    let reader = it.clone().freeze();
    let json = serde_json::to_string(&SerializeReader(&reader)).unwrap();
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
    let error = serde_json::from_str::<LocalLexicon>("42").unwrap_err();
    assert!(error.to_string().contains("a sequence of interned strings"));
    serde_json::from_str::<LocalLexicon>("[\"a\", 42]").unwrap_err();
}

#[test]
fn foreign_sym_resolves_to_none_without_panicking() {
    // Crafted handles with a zero local index must be rejected without
    // underflowing.
    let it = ThreadedLexicon::new();
    let real = it.intern("hello");
    let reader = it.freeze();

    assert_eq!(reader.resolve(real), "hello");

    // Shard 1, all local bits zero (`1 << LOCAL_BITS`): a non-zero handle whose
    // low 26 bits are 0.
    let zero_local = Sym::from_u32(1u32 << 26).unwrap();
    assert_eq!(reader.try_resolve(zero_local), None);

    let past_end = Sym::from_u32(u32::MAX).unwrap();
    assert_eq!(reader.try_resolve(past_end), None);
}

/// A `freeze` that races a concurrent writer must observe a single point-in-time
/// snapshot, not a per-shard-torn one. A single writer interns `s0, s1, …` in
/// order, so at any instant the committed set is exactly a contiguous prefix
/// `{s0, …, s{k-1}}`. A torn snapshot (some shards read before a write, others
/// after) could surface a *non-prefix* set — e.g. `s9` present while `s3` is
/// missing — so asserting the prefix property on every mid-flight snapshot
/// guards the cross-shard consistency of `build_reader`.
#[cfg(not(all(miri, windows)))]
#[cfg_attr(
    miri_strict_provenance,
    ignore = "parking_lot_core uses integer-to-pointer casts on Unix, which strict-provenance Miri rejects"
)]
#[test]
fn freeze_races_writer_and_stays_prefix_consistent() {
    use std::collections::BTreeSet;
    use std::sync::mpsc;

    // The properties under test are structural -- a freeze snapshot must be a
    // prefix, never a cross-shard tear -- so they hold at any size, provided the
    // count still spans several shards. The polling loop below re-freezes and
    // walks the whole lexicon on every iteration, so the cost grows faster than
    // linearly under Miri; 256 keeps multi-shard coverage at a fraction of it.
    #[cfg(miri)]
    const COUNT: usize = 256;
    #[cfg(not(miri))]
    const COUNT: usize = 4096;
    const MID: usize = COUNT / 2;

    let writer_lex = ThreadedLexicon::new();
    let freezer_lex = writer_lex.clone();

    // Rendezvous over channels rather than barriers/flags so a writer panic can
    // never hang the main thread: a dropped sender surfaces as `recv` returning
    // `Err`, and loop termination is driven off `JoinHandle::is_finished` (true on
    // both normal return and panic). Every exit path joins the worker so a panic
    // is reported, never swallowed or turned into a stall.
    let (reached_tx, reached_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();

    let writer = thread::spawn(move || {
        for i in 0..MID {
            writer_lex.intern(format!("s{i}"));
        }
        // Publish exactly `MID` handles, then block until released. If the main
        // thread has gone away, `send`/`recv` fail and the worker simply exits.
        if reached_tx.send(()).is_err() {
            return;
        }
        if release_rx.recv().is_err() {
            return;
        }
        for i in MID..COUNT {
            writer_lex.intern(format!("s{i}"));
        }
    });

    // Deterministic mid-flight snapshot: the writer is paused at exactly `MID`.
    // A `recv` error means the writer terminated early (e.g. panicked) before the
    // rendezvous — join to surface the underlying failure instead of hanging.
    if reached_rx.recv().is_err() {
        writer.join().expect("writer terminated before the mid rendezvous");
        panic!("writer dropped the rendezvous channel without panicking");
    }
    {
        let reader = freezer_lex.clone().freeze();
        let present: BTreeSet<String> = reader.iter().map(|(_, s)| s.to_owned()).collect();
        assert_eq!(present.len(), reader.len(), "torn length vs content");
        assert_eq!(reader.len(), MID, "writer is pinned at exactly MID committed handles");
        assert!(!present.is_empty() && present.len() < COUNT, "snapshot must be partial");
        let expected: BTreeSet<String> = (0..MID).map(|i| format!("s{i}")).collect();
        assert_eq!(present, expected, "mid-flight snapshot is not a prefix — cross-shard tear");
    }
    // Release the writer to finish the remaining `COUNT - MID` interns. If the
    // writer already exited, the receiver is gone and `send` fails harmlessly.
    let _ = release_tx.send(());

    // `is_finished` also terminates the loop if the writer panics.
    loop {
        let done = writer.is_finished();
        let reader = freezer_lex.clone().freeze();

        let present: BTreeSet<String> = reader.iter().map(|(_, s)| s.to_owned()).collect();
        assert_eq!(present.len(), reader.len(), "torn length vs content");

        for (sym, s) in reader.iter() {
            assert_eq!(reader.resolve(sym), s);
        }

        let expected: BTreeSet<String> = (0..present.len()).map(|i| format!("s{i}")).collect();
        assert_eq!(present, expected, "snapshot is not a prefix — cross-shard tear");

        if done {
            break;
        }
    }

    writer.join().expect("writer panicked");

    let reader = freezer_lex.freeze();
    assert_eq!(reader.len(), COUNT);
}

#[test]
fn with_capacity_preallocates_and_interns() {
    let mut lexicon = LocalLexicon::with_capacity(128, 128 * 8);
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
    let mut lexicon = LocalLexicon::with_capacity_and_hasher(16, 256, RandomState::new());
    let a = lexicon.intern("alpha");
    assert_eq!(lexicon.intern("alpha"), a);
    assert_eq!(lexicon.resolve(a), "alpha");
}

#[test]
fn with_capacity_zero_is_valid() {
    let mut lexicon = LocalLexicon::with_capacity(0, 0);
    let a = lexicon.intern("x");
    assert_eq!(lexicon.resolve(a), "x");
}

#[test]
fn intern_bytes_interns_valid_utf8_and_resolves_to_str() {
    let mut lexicon = LocalLexicon::new();
    let a = lexicon.intern_bytes("café".as_bytes()).expect("valid UTF-8");
    assert_eq!(lexicon.resolve(a), "café");
    assert_eq!(lexicon.get("café"), Some(a));
}

#[test]
fn intern_bytes_rejects_invalid_utf8_on_first_insert() {
    let mut lexicon = LocalLexicon::new();
    let err = lexicon.intern_bytes(&[0xff, 0xfe]).unwrap_err();
    assert_eq!(err.valid_up_to(), 0);
    assert_eq!(lexicon.len(), 0); // nothing was stored
}

#[test]
fn intern_bytes_hit_skips_revalidation_and_dedups() {
    let mut lexicon = LocalLexicon::new();
    // Seed via the str path, then hit via the byte path: the stored entry is
    // already valid, so the hit returns the same handle without re-validating.
    let a = lexicon.intern("hello");
    let b = lexicon.intern_bytes(b"hello").expect("already interned");
    assert_eq!(a, b);
    assert_eq!(lexicon.len(), 1);

    let c = lexicon.intern_bytes(b"world").expect("valid UTF-8");
    assert_eq!(lexicon.intern_bytes(b"world").expect("hit"), c);
    assert_ne!(a, c);
    assert_eq!(lexicon.len(), 2);
}

#[test]
fn intern_bytes_through_lexicon_trait_object() {
    let mut lexicon: Box<dyn Lexicon> = Box::new(LocalLexicon::new());
    let a = lexicon.intern_bytes(b"erased").expect("valid UTF-8");
    assert_eq!(lexicon.intern("erased"), a); // dedups with the str path
    lexicon.intern_bytes(&[0x80]).unwrap_err();
    assert_eq!(lexicon.freeze().resolve(a), "erased");
}

#[test]
fn threaded_intern_bytes_interns_validates_and_dedups() {
    let lexicon = ThreadedLexicon::new();
    let a = lexicon.intern_bytes("café".as_bytes()).expect("valid UTF-8");
    assert_eq!(lexicon.intern_bytes(b"caf\xc3\xa9").expect("hit"), a); // same bytes → same handle
    assert_eq!(lexicon.intern("café"), a); // dedups with the str path
    lexicon.intern_bytes(&[0xff]).unwrap_err();
    assert_eq!(lexicon.freeze().resolve(a), "café");
}

#[test]
fn threaded_intern_bytes_through_lexicon_trait() {
    let mut lexicon: Box<dyn Lexicon> = Box::new(ThreadedLexicon::new());
    let a = lexicon.intern_bytes(b"shared").expect("valid UTF-8");
    assert_eq!(lexicon.intern("shared"), a);
    lexicon.intern_bytes(&[0xc0]).unwrap_err();
    assert_eq!(lexicon.freeze().resolve(a), "shared");
}

#[cfg(not(all(miri, windows)))]
#[test]
fn threaded_intern_bytes_is_consistent_across_threads() {
    let lexicon = ThreadedLexicon::new();
    #[expect(clippy::needless_collect, reason = "all threads must be spawned before any are joined")]
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let lexicon = lexicon.clone();
            thread::spawn(move || lexicon.intern_bytes(b"concurrent").expect("valid UTF-8"))
        })
        .collect();
    let syms: Vec<Sym> = handles.into_iter().map(|h| h.join().expect("thread panicked")).collect();
    let first = syms[0];
    assert!(syms.iter().all(|&s| s == first)); // every thread agrees on one handle
    assert_eq!(lexicon.len(), 1);
    assert_eq!(lexicon.freeze().resolve(first), "concurrent");
}

#[test]
fn intern_bytes_walks_collision_chain_by_byte_comparison() {
    // `PanicOnMarkerHasher::finish` returns 0, so every key collides into one
    // bucket: the byte-probe must walk the chain and compare bytes to dedup.
    let (hasher, _armed) = panic_on_marker_hasher();
    let mut lexicon = LocalLexicon::with_hasher(hasher);

    let alpha = lexicon.intern_bytes(b"alpha").unwrap();
    let beta = lexicon.intern("beta");
    let gamma = lexicon.intern_bytes(b"gamma").unwrap();
    let delta = lexicon.intern_bytes("δ".as_bytes()).unwrap(); // multibyte, same bucket
    assert_eq!(lexicon.len(), 4);
    assert_ne!(alpha, beta);
    assert_ne!(beta, gamma);
    assert_ne!(gamma, delta);
    assert_ne!(alpha, delta);

    assert_eq!(lexicon.intern_bytes(b"alpha").unwrap(), alpha);
    assert_eq!(lexicon.intern_bytes(b"beta").unwrap(), beta);
    assert_eq!(lexicon.intern("gamma"), gamma);
    assert_eq!(lexicon.intern_bytes("δ".as_bytes()).unwrap(), delta);
    assert_eq!(lexicon.len(), 4); // no duplicates created

    let epsilon = lexicon.intern_bytes(b"epsilon").unwrap();
    assert_ne!(epsilon, alpha);
    assert_eq!(lexicon.len(), 5);

    let reader = lexicon.freeze();
    assert_eq!(reader.resolve(alpha), "alpha");
    assert_eq!(reader.resolve(beta), "beta");
    assert_eq!(reader.resolve(gamma), "gamma");
    assert_eq!(reader.resolve(delta), "δ");
    assert_eq!(reader.resolve(epsilon), "epsilon");
}

#[test]
fn threaded_intern_bytes_walks_collision_chain_by_byte_comparison() {
    let (hasher, _armed) = panic_on_marker_hasher();
    let lexicon = ThreadedLexicon::with_hasher(hasher);

    let alpha = lexicon.intern_bytes(b"alpha").unwrap();
    let beta = lexicon.intern("beta");
    let gamma = lexicon.intern_bytes(b"gamma").unwrap();
    let delta = lexicon.intern_bytes("δ".as_bytes()).unwrap();
    assert_eq!(lexicon.len(), 4);
    assert_ne!(alpha, beta);
    assert_ne!(beta, gamma);
    assert_ne!(gamma, delta);
    assert_ne!(alpha, delta);

    assert_eq!(lexicon.intern_bytes(b"alpha").unwrap(), alpha);
    assert_eq!(lexicon.intern("gamma"), gamma);
    assert_eq!(lexicon.intern_bytes("δ".as_bytes()).unwrap(), delta);
    assert_eq!(lexicon.len(), 4);

    let reader = lexicon.freeze();
    assert_eq!(reader.resolve(alpha), "alpha");
    assert_eq!(reader.resolve(beta), "beta");
    assert_eq!(reader.resolve(gamma), "gamma");
    assert_eq!(reader.resolve(delta), "δ");
}

#[test]
fn dense_index_round_trips_through_lexicon_and_reader() {
    let mut lexicon = LocalLexicon::new();
    let syms: Vec<Sym> = (0..64).map(|i| lexicon.intern(format!("name{i}"))).collect();

    for (expected, &sym) in syms.iter().enumerate() {
        assert_eq!(lexicon.index_of(sym), Some(expected));
        assert_eq!(lexicon.sym_at(expected), Some(sym));
    }

    // Freezing preserves the numbering.
    let reader = lexicon.freeze();
    for (expected, &sym) in syms.iter().enumerate() {
        assert_eq!(reader.index_of(sym), Some(expected));
        assert_eq!(reader.sym_at(expected), Some(sym));
        assert_eq!(reader.resolve(sym), format!("name{expected}"));
    }
}

#[test]
fn dense_index_is_zero_based_while_raw_handle_is_one_based() {
    // The raw handle is deliberately non-zero, so it is offset by one from the
    // position. Callers must use `index_of`, never `as_u32`, to index a side
    // table; this pins the distinction the docs promise.
    let mut lexicon = LocalLexicon::new();
    let a = lexicon.intern("a");
    let b = lexicon.intern("b");

    assert_eq!(lexicon.index_of(a), Some(0));
    assert_eq!(lexicon.index_of(b), Some(1));
    assert_eq!(a.as_u32(), 1);
    assert_eq!(b.as_u32(), 2);

    // The last handle's raw value is out of bounds for a `len`-sized side table.
    assert_eq!(usize::try_from(b.as_u32()).unwrap(), lexicon.len());
}

#[test]
fn dense_index_is_none_out_of_range() {
    let mut lexicon = LocalLexicon::new();
    let only = lexicon.intern("only");

    assert_eq!(lexicon.index_of(only), Some(0));
    assert_eq!(lexicon.sym_at(1), None);
    assert_eq!(lexicon.sym_at(usize::MAX), None);

    // The first handle past the end: its dense position is exactly `len`, the
    // boundary where an off-by-one would hand back a bogus in-range index.
    let first_past_end = Sym::from_u32(2).unwrap();
    assert_eq!(lexicon.index_of(first_past_end), None);

    // A handle past the end is rejected rather than yielding a bogus index.
    let past_end = Sym::from_u32(u32::MAX).unwrap();
    assert_eq!(lexicon.index_of(past_end), None);

    let reader = lexicon.freeze();
    assert_eq!(reader.index_of(first_past_end), None);
    assert_eq!(reader.index_of(past_end), None);
    assert_eq!(reader.sym_at(1), None);
    assert_eq!(reader.sym_at(usize::MAX), None);
}

#[test]
fn dense_index_supports_a_side_table() {
    let mut lexicon = LocalLexicon::new();
    for name in ["alpha", "beta", "gamma"] {
        let _ = lexicon.intern(name);
    }

    // One slot per symbol, indexed directly — no hashing.
    let mut lengths = vec![0usize; lexicon.len()];
    for (sym, s) in lexicon.iter() {
        let i = lexicon.index_of(sym).unwrap();
        lengths[i] = s.len();
    }
    assert_eq!(lengths, vec![5, 4, 5]);
}

#[test]
fn frozen_readers_can_be_stored_by_value() {
    // The point of exporting the concrete types: a struct can name the field.
    struct LocalStore {
        names: internity::LocalReader,
    }
    struct ThreadedStore {
        names: internity::ThreadedReader,
    }

    let mut lexicon = LocalLexicon::new();
    let a = lexicon.intern("a");
    let local = LocalStore { names: lexicon.freeze() };
    assert_eq!(local.names.resolve(a), "a");

    let lexicon = ThreadedLexicon::new();
    let b = lexicon.intern("b");
    let threaded = ThreadedStore { names: lexicon.freeze() };
    assert_eq!(threaded.names.resolve(b), "b");
}

#[test]
fn frozen_readers_are_cloneable() {
    // Readers are immutable, so a clone is an independent owner of the same
    // corpus. Handles stay valid against either copy.
    let mut lexicon = LocalLexicon::new();
    let a = lexicon.intern("a");
    let b = lexicon.intern("b");
    let local = lexicon.freeze();
    let local_clone = local.clone();

    drop(local);
    assert_eq!(local_clone.resolve(a), "a");
    assert_eq!(local_clone.index_of(b), Some(1));
    assert_eq!(local_clone.len(), 2);

    let lexicon = ThreadedLexicon::new();
    let c = lexicon.intern("c");
    let threaded = lexicon.freeze();
    let threaded_clone = threaded.clone();

    drop(threaded);
    assert_eq!(threaded_clone.resolve(c), "c");
    assert_eq!(threaded_clone.len(), 1);
}

#[test]
fn frozen_reader_supports_an_external_string_lookup() {
    // The pattern documented on `LocalLexicon::freeze`, exercised end to end.
    let mut lexicon = LocalLexicon::new();
    let names: Vec<String> = (0..256).map(|i| format!("name{i}")).collect();
    let syms: Vec<Sym> = names.iter().map(|n| lexicon.intern(n)).collect();

    let reader = lexicon.freeze();
    let mut by_string: Vec<Sym> = reader.iter().map(|(sym, _)| sym).collect();
    by_string.sort_unstable_by(|&a, &b| reader.resolve(a).cmp(reader.resolve(b)));

    let get = |needle: &str| {
        by_string
            .binary_search_by(|&sym| reader.resolve(sym).cmp(needle))
            .ok()
            .map(|i| by_string[i])
    };

    for (name, &want) in names.iter().zip(&syms) {
        assert_eq!(get(name), Some(want));
    }
    assert_eq!(get("absent"), None);
}

#[test]
fn reader_debug_reports_length() {
    let mut lexicon = LocalLexicon::new();
    let _ = lexicon.intern("a");
    let reader = lexicon.freeze();
    let text = format!("{reader:?}");
    assert!(text.contains("LocalReader"), "{text}");
    assert!(text.contains("len"), "{text}");

    let lexicon = ThreadedLexicon::new();
    let _ = lexicon.intern("a");
    let text = format!("{:?}", lexicon.freeze());
    assert!(text.contains("ThreadedReader"), "{text}");
}
