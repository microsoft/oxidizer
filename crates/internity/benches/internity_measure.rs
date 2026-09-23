// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Machine-readable workload measurements. Run with `cargo bench -p internity
//! --bench internity_measure --all-features` (not the compile-only Anvil recipe).
//! Each line is a JSON measurement; setup is outside the timed region.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    reason = "benchmark counts and percentiles"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::hash::{BuildHasher, Hasher};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

use foldhash::fast::FixedState;
use internity::de::DeserializeIn;
use internity::se::{SerializeIn, SerializeInWith, SerializeReader};
use internity::{Lexicon, LocalLexicon, Reader, Sym, ThreadedLexicon};
use rustc_hash::FxBuildHasher;
use serde::Deserializer;
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};
use serde_json::json;

#[global_allocator]
static ALLOC: Meter = Meter;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct Meter;

// SAFETY: all operations delegate to System unchanged; counters are observational.
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwards the allocator's caller-provided layout unchanged.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            record(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwards the allocator's caller-provided layout unchanged.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            record(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
        // SAFETY: forwards the caller-provided pointer and layout unchanged.
        unsafe { System.dealloc(ptr, layout) };
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwards pointer, layout, and new size unchanged.
        let result = unsafe { System.realloc(ptr, layout, new_size) };
        if !result.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
            record(new_size);
        }
        result
    }
}

fn record(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::SeqCst) + bytes;
    PEAK.fetch_max(live, Ordering::SeqCst);
    BYTES.fetch_add(bytes, Ordering::SeqCst);
    ALLOCS.fetch_add(1, Ordering::SeqCst);
}

fn measure<T>(id: &str, ops: usize, f: impl FnOnce() -> T) -> T {
    measure_items(id, ops, ops, f)
}

fn measure_items<T>(id: &str, ops: usize, items: usize, f: impl FnOnce() -> T) -> T {
    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let bytes = BYTES.load(Ordering::SeqCst);
    let allocs = ALLOCS.load(Ordering::SeqCst);
    let now = Instant::now();
    let result = f();
    let elapsed = now.elapsed();
    let elapsed_ns = elapsed.as_nanos() as f64;
    let allocated_bytes = BYTES.load(Ordering::SeqCst) - bytes;
    let allocations = ALLOCS.load(Ordering::SeqCst) - allocs;
    let peak = PEAK.load(Ordering::SeqCst);
    println!(
        "{}",
        json!({
            "id": id,
            "ops": ops,
            "ns_per_op": elapsed_ns / ops as f64,
            "ops_per_sec": ops as f64 * 1e9 / elapsed_ns,
            "items_per_sec": items as f64 * 1e9 / elapsed_ns,
            "allocated_bytes_per_op": allocated_bytes as f64 / ops as f64,
            "allocations_per_op": allocations as f64 / ops as f64,
            "peak_bytes": peak,
            "peak_added_bytes": peak.saturating_sub(before),
        })
    );
    black_box(result)
}

fn corpus(size: usize) -> Vec<String> {
    let mut state: u64 = 0x1234_5678_9abc_def0;
    let mut out = std::collections::BTreeSet::new();
    let alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    while out.len() < size {
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let len = 3 + (next() % 20) as usize;
        let mut s = String::with_capacity(len);
        for _ in 0..len {
            s.push(alphabet[(next() as usize) % alphabet.len()] as char);
        }
        out.insert(s);
    }
    out.into_iter().collect()
}

fn filled_local(words: &[String]) -> LocalLexicon {
    let mut lex = LocalLexicon::new();
    for word in words {
        lex.intern(word);
    }
    lex
}

fn filled_threaded(words: &[String]) -> ThreadedLexicon {
    let lex = ThreadedLexicon::new();
    for word in words {
        lex.intern(word);
    }
    lex
}

fn shard_for<S: BuildHasher>(key: &str, hasher: &S) -> u32 {
    // Mirror threaded_lexicon's byte-only hash and multiply-mix.
    // Checking against an actual handle below prevents a stale benchmark
    // selector from silently claiming a same-shard workload.
    let mut state = hasher.build_hasher();
    state.write(key.as_bytes());
    (state.finish().wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 58) as u32
}

fn direct(words: &[String]) {
    measure("b1/local_insert", words.len(), || filled_local(words));
    measure("b1/threaded_insert", words.len(), || filled_threaded(words));
    let mut local = filled_local(words);
    let threaded = filled_threaded(words);
    measure("b1/local_reuse", words.len(), || {
        for word in words {
            black_box(local.intern(word));
        }
    });
    measure("b1/threaded_reuse", words.len(), || {
        for word in words {
            black_box(threaded.intern(word));
        }
    });
    let local_reader = local.freeze();
    let threaded_reader = threaded.freeze();
    let local_keys: Vec<_> = local_reader.iter().map(|(sym, _)| sym).collect();
    let threaded_keys: Vec<_> = threaded_reader.iter().map(|(sym, _)| sym).collect();
    measure("b1/local_lookup", words.len(), || {
        for sym in &local_keys {
            black_box(local_reader.resolve(*sym));
        }
    });
    measure("b1/threaded_lookup", words.len(), || {
        for sym in &threaded_keys {
            black_box(threaded_reader.resolve(*sym));
        }
    });
    // Live retained bytes, not peak bytes. The corpus is constructed beforehand.
    for (id, threaded) in [("b1/local_heap", false), ("b1/threaded_heap", true)] {
        let before = LIVE.load(Ordering::SeqCst);
        if threaded {
            let value = filled_threaded(words);
            let held = LIVE.load(Ordering::SeqCst) - before;
            println!("{}", json!({"id": id, "retained_bytes": held}));
            drop(value);
        } else {
            let value = filled_local(words);
            let held = LIVE.load(Ordering::SeqCst) - before;
            println!("{}", json!({"id": id, "retained_bytes": held}));
            drop(value);
        }
    }
}

fn scans(words: &[String]) {
    let local = filled_local(words).freeze();
    let threaded = filled_threaded(words).freeze();
    for (id, reader) in [
        ("b6/local_erased", &local as &dyn Reader),
        ("b6/threaded_erased", &threaded as &dyn Reader),
    ] {
        measure_items(&format!("{id}/{}", words.len()), 1, words.len(), || {
            for item in reader.iter() {
                black_box(item);
            }
        });
    }
    measure_items(&format!("b6/local_concrete/{}", words.len()), 1, words.len(), || {
        for item in local.iter() {
            black_box(item);
        }
    });
    measure_items(&format!("b6/threaded_concrete/{}", words.len()), 1, words.len(), || {
        for item in threaded.iter() {
            black_box(item);
        }
    });
    for (id, reader) in [
        ("b6/local_export", &local as &dyn Reader),
        ("b6/threaded_export", &threaded as &dyn Reader),
    ] {
        measure_items(&format!("{id}/{}", words.len()), 1, words.len(), || {
            serde_json::to_vec(&SerializeReader(reader)).expect("the frozen corpus contains only valid strings")
        });
    }
}

#[derive(SerializeIn, DeserializeIn)]
struct Record {
    #[serde(alias = "identifier")]
    name: Sym,
    #[serde(alias = "alternate")]
    alias: Sym,
    count: u32,
}

macro_rules! wide_record {
    ($($field:ident => $alias:literal),+ $(,)?) => {
        #[derive(SerializeIn, DeserializeIn)]
        struct WideRecord {
            $(#[serde(alias = $alias)] $field: Sym,)+
        }
    };
}

wide_record!(
    f00 => "alias_00", f01 => "alias_01", f02 => "alias_02", f03 => "alias_03",
    f04 => "alias_04", f05 => "alias_05", f06 => "alias_06", f07 => "alias_07",
    f08 => "alias_08", f09 => "alias_09", f10 => "alias_10", f11 => "alias_11",
    f12 => "alias_12", f13 => "alias_13", f14 => "alias_14", f15 => "alias_15",
    f16 => "alias_16", f17 => "alias_17", f18 => "alias_18", f19 => "alias_19",
    f20 => "alias_20", f21 => "alias_21", f22 => "alias_22", f23 => "alias_23",
    f24 => "alias_24", f25 => "alias_25", f26 => "alias_26", f27 => "alias_27",
    f28 => "alias_28", f29 => "alias_29", f30 => "alias_30", f31 => "alias_31",
    f32 => "alias_32", f33 => "alias_33", f34 => "alias_34", f35 => "alias_35",
    f36 => "alias_36", f37 => "alias_37", f38 => "alias_38", f39 => "alias_39",
    f40 => "alias_40", f41 => "alias_41", f42 => "alias_42", f43 => "alias_43",
    f44 => "alias_44", f45 => "alias_45", f46 => "alias_46", f47 => "alias_47",
    f48 => "alias_48", f49 => "alias_49", f50 => "alias_50", f51 => "alias_51",
    f52 => "alias_52", f53 => "alias_53", f54 => "alias_54", f55 => "alias_55",
    f56 => "alias_56", f57 => "alias_57", f58 => "alias_58", f59 => "alias_59",
    f60 => "alias_60", f61 => "alias_61", f62 => "alias_62", f63 => "alias_63",
);

struct ByteIdentifier<'a>(&'a [u8]);

impl<'de> Deserializer<'de> for ByteIdentifier<'de> {
    type Error = de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_bytes(self.0)
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_bytes(self.0)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
        map struct enum ignored_any
    }
}

struct ByteRecord<'a> {
    keys: &'a [String],
    words: &'a [String],
    row: usize,
}

struct ByteFields<'a> {
    record: ByteRecord<'a>,
    index: usize,
}

impl<'de> Deserializer<'de> for ByteRecord<'de> {
    type Error = de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_map(ByteFields { record: self, index: 0 })
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_any(visitor)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
        map enum identifier ignored_any
    }
}

impl<'de> MapAccess<'de> for ByteFields<'de> {
    type Error = de::value::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error> {
        self.record
            .keys
            .get(self.index)
            .map(|key| seed.deserialize(ByteIdentifier(key.as_bytes())))
            .transpose()
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Self::Error> {
        let word = &self.record.words[(self.record.row + self.index) % self.record.words.len()];
        self.index += 1;
        seed.deserialize(de::value::BorrowedStrDeserializer::new(word.as_str()))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.record.keys.len() - self.index)
    }
}

fn wide_records<I: Lexicon>(lex: &mut I, words: &[String], keys: &[String], json_input: &str, byte_keys: bool) -> Vec<WideRecord> {
    if byte_keys {
        (0..6000)
            .map(|row| {
                WideRecord::deserialize_in(lex, ByteRecord { keys, words, row })
                    .expect("64 byte aliases map to 64 valid UTF-8 symbol fields")
            })
            .collect()
    } else {
        Vec::<WideRecord>::deserialize_in(lex, &mut serde_json::Deserializer::from_str(json_input))
            .expect("the fixed 64-field JSON fixture has all declared aliases")
    }
}

fn wide_json(words: &[String], keys: &[String]) -> String {
    format!(
        "[{}]",
        (0..6000)
            .map(|row| {
                format!(
                    "{{{}}}",
                    keys.iter()
                        .enumerate()
                        .map(|(field, alias)| format!(
                            "{}:{}",
                            serde_json::to_string(alias).expect("serializing a String to JSON cannot fail"),
                            serde_json::to_string(&words[(row + field) % words.len()]).expect("serializing a String to JSON cannot fail")
                        ))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn wide_serde_workload(words: &[String]) {
    let aliases: Vec<_> = (0..64).map(|i| format!("alias_{i:02}")).collect();
    let fields: Vec<_> = (0..64).map(|i| format!("f{i:02}")).collect();
    let json_input = wide_json(words, &aliases);
    let expected_export = wide_json(words, &fields);
    for bytes in [false, true] {
        for threaded in [false, true] {
            let id = format!(
                "b4/{}/wide64_{}",
                if threaded { "threaded" } else { "local" },
                if bytes { "bytes" } else { "json" }
            );
            if threaded {
                let output = measure_items(&id, 1, 6000, || {
                    let mut lex = ThreadedLexicon::new();
                    let records = wide_records(&mut lex, words, &aliases, &json_input, bytes);
                    let reader = lex.freeze();
                    let corpus = serde_json::to_vec(&SerializeReader(&reader)).expect("frozen strings are valid UTF-8");
                    let exported = serde_json::to_vec(&SerializeInWith::new(&records, &reader))
                        .expect("every record handle was interned into this reader");
                    (records, reader, corpus, exported)
                });
                assert_eq!(
                    output.3,
                    expected_export.as_bytes(),
                    "wide byte/string field routing must preserve values"
                );
            } else {
                let output = measure_items(&id, 1, 6000, || {
                    let mut lex = LocalLexicon::new();
                    let records = wide_records(&mut lex, words, &aliases, &json_input, bytes);
                    let reader = lex.freeze();
                    let corpus = serde_json::to_vec(&SerializeReader(&reader)).expect("frozen strings are valid UTF-8");
                    let exported = serde_json::to_vec(&SerializeInWith::new(&records, &reader))
                        .expect("every record handle was interned into this reader");
                    (records, reader, corpus, exported)
                });
                assert_eq!(
                    output.3,
                    expected_export.as_bytes(),
                    "wide byte/string field routing must preserve values"
                );
            }
        }
    }
}

fn serde_workload(words: &[String]) {
    for shape in ["distinct", "duplicate90", "escaped"] {
        let json_input = format!(
            "[{}]",
            (0..6000)
                .map(|i| {
                    let name = if shape == "duplicate90" && i % 10 != 0 {
                        words[0].clone()
                    } else if shape == "escaped" {
                        format!("é\\\"{}\n", words[i])
                    } else {
                        words[i].clone()
                    };
                    format!(
                        r#"{{"identifier":{},"alternate":{},"count":{}}}"#,
                        serde_json::to_string(&name).expect("serializing a String to JSON cannot fail"),
                        serde_json::to_string(&words[(i / 10) % words.len()]).expect("serializing a String to JSON cannot fail"),
                        i
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        );
        for threaded in [false, true] {
            let id = format!("b4/{}/{}", if threaded { "threaded" } else { "local" }, shape);
            if threaded {
                measure_items(&id, 1, 6000, || {
                    let lex = ThreadedLexicon::new();
                    let records: Vec<Record> = lex
                        .deserialize_in(&mut serde_json::Deserializer::from_str(&json_input))
                        .expect("generated JSON contains all Record fields with valid values");
                    let reader = lex.freeze();
                    let corpus_json = serde_json::to_vec(&SerializeReader(&reader)).expect("frozen corpus contains only validated strings");
                    let records_json = serde_json::to_vec(&SerializeInWith::new(&records, &reader))
                        .expect("all Record handles came from the same lexicon as the reader");
                    (records, reader, corpus_json, records_json)
                });
            } else {
                measure_items(&id, 1, 6000, || {
                    let mut lex = LocalLexicon::new();
                    let records: Vec<Record> = lex
                        .deserialize_in(&mut serde_json::Deserializer::from_str(&json_input))
                        .expect("generated JSON contains all Record fields with valid values");
                    let reader = lex.freeze();
                    let corpus_json = serde_json::to_vec(&SerializeReader(&reader)).expect("frozen corpus contains only validated strings");
                    let records_json = serde_json::to_vec(&SerializeInWith::new(&records, &reader))
                        .expect("all Record handles came from the same lexicon as the reader");
                    (records, reader, corpus_json, records_json)
                });
            }
        }
    }
}

fn contention<S: BuildHasher + Clone + Send + Sync>(words: &[String], hasher: &S, choice: &str) {
    for threads in [1, 2, 4, 8, 16] {
        for hit_pct in [90, 99] {
            for bytes in [false, true] {
                for skew in [false, true] {
                    let lex = ThreadedLexicon::with_hasher((*hasher).clone());
                    for word in words {
                        lex.intern(word);
                    }
                    let target_shard = lex
                        .get(&words[0])
                        .expect("corpus is nonempty and its first word was interned during setup")
                        .as_u32()
                        >> 26;
                    let start = Arc::new(Barrier::new(threads + 1));
                    let end = Arc::new(Barrier::new(threads + 1));
                    let id = format!(
                        "b2/{choice}/{}/{}/{hit_pct}/{threads}/{}",
                        if skew { "same_shard" } else { "uniform" },
                        if bytes { "bytes" } else { "str" },
                        words.len()
                    );
                    // Build *absent* strings outside the timed region and select
                    // the same physical shard using public packed handles.
                    let mut miss = Vec::new();
                    for i in 0.. {
                        let key = format!("miss-key-{i}");
                        if !skew || shard_for(&key, hasher) == target_shard {
                            miss.push(key);
                        }
                        if miss.len() >= threads * (words.len() / 10 + 1) {
                            break;
                        }
                    }
                    if skew {
                        let probe = ThreadedLexicon::with_hasher((*hasher).clone());
                        assert_eq!(probe.intern(&miss[0]).as_u32() >> 26, target_shard);
                    }
                    let mut tails = Vec::new();
                    thread::scope(|scope| {
                        let mut workers = Vec::new();
                        for worker in 0..threads {
                            let lex = &lex;
                            let start = Arc::clone(&start);
                            let end = Arc::clone(&end);
                            let miss = &miss;
                            workers.push(scope.spawn(move || {
                                let mut latencies = Vec::new();
                                start.wait();
                                for i in 0..words.len() {
                                    let is_miss = i % 100 >= hit_pct;
                                    let key = if is_miss {
                                        miss[worker * (words.len() / 10 + 1) + i / 100 * (100 - hit_pct) + (i % 100 - hit_pct)].as_str()
                                    } else if skew {
                                        &words[0]
                                    } else {
                                        &words[(i + worker) % words.len()]
                                    };
                                    let now = Instant::now();
                                    if bytes {
                                        if i % 1000 == 0 {
                                            black_box(lex.intern_bytes(b"\xff\xfe").is_err());
                                        } else {
                                            black_box(lex.intern_bytes(key.as_bytes()).expect("key is a valid UTF-8 string"));
                                        }
                                    } else {
                                        black_box(lex.intern(key));
                                    }
                                    if is_miss {
                                        latencies.push(now.elapsed().as_nanos() as u64);
                                    }
                                }
                                end.wait();
                                latencies
                            }));
                        }
                        start.wait();
                        let now = Instant::now();
                        end.wait();
                        let elapsed = now.elapsed().as_secs_f64();
                        for worker in workers {
                            tails.extend(worker.join().expect("benchmark worker must not panic on prevalidated keys"));
                        }
                        tails.sort_unstable();
                        println!(
                            "{}",
                            json!({
                                "id": id,
                                "ops_per_sec": threads as f64 * words.len() as f64 / elapsed,
                                "p95_miss_ns": tails[tails.len() * 95 / 100],
                                "miss_samples": tails.len(),
                                "invalid_inputs": if bytes { words.len().div_ceil(1000) * threads } else { 0 },
                                "lock_wait_share": null,
                            })
                        );
                    });
                }
            }
        }
    }
}

fn assert_snapshot(snapshot: &internity::ThreadedReader, words: &[String], writers: usize) {
    let mut counts = vec![0usize; writers];
    let mut endpoints = vec![0usize; writers];
    let mut corpus_count = 0;
    for (sym, value) in snapshot.iter() {
        assert_eq!(snapshot.resolve(sym), value);
        if let Some(suffix) = value.strip_prefix("writer-") {
            let (worker, index) = suffix
                .split_once('-')
                .expect("writer keys contain a worker index and sequence number");
            let worker: usize = worker.parse().expect("writer keys use a numeric worker index");
            let index: usize = index.parse().expect("writer keys use a numeric sequence number");
            assert!(worker < writers, "snapshot contains an unexpected writer");
            counts[worker] += 1;
            endpoints[worker] = endpoints[worker].max(index + 1);
        } else {
            corpus_count += 1;
        }
    }
    assert_eq!(corpus_count, words.len(), "snapshot must retain the entire prefilled corpus");
    assert_eq!(counts, endpoints, "each writer's committed keys must form a contiguous prefix");
    assert_eq!(snapshot.len(), corpus_count + counts.iter().sum::<usize>());
}

const LONG_SUFFIX: &str = "-long-identifier-component-0123456789-abcdef-long-identifier-component-0123456789-abcdef";

fn long_corpus(words: &[String]) -> Vec<String> {
    words.iter().map(|word| format!("{word}{LONG_SUFFIX}")).collect()
}

fn mixed_corpus(words: &[String]) -> Vec<String> {
    words
        .iter()
        .enumerate()
        .map(|(index, word)| {
            if index % 2 == 0 {
                word.clone()
            } else {
                format!("{word}{LONG_SUFFIX}")
            }
        })
        .collect()
}

fn freeze_scenarios(words: &[String]) {
    freeze(words, "short");
    freeze(&long_corpus(words), "long");
    freeze(&mixed_corpus(words), "mixed");
}

fn freeze(words: &[String], shape: &str) {
    let lex = filled_threaded(words);
    let shared = lex.clone();
    let shared = measure(&format!("b3/shared_freeze/{shape}/{}", words.len()), 1, || shared.freeze());
    assert_snapshot(&shared, words, 0);
    drop(shared);
    let owned = measure(&format!("b3/owned_freeze/{shape}/{}", words.len()), 1, || lex.freeze());
    assert_snapshot(&owned, words, 0);
    drop(owned);
    for writers in [0, 1, 8] {
        for rounds in [1, 3] {
            let lex = filled_threaded(words);
            let barrier = Barrier::new(writers + 1);
            let measuring = AtomicBool::new(false);
            let stop = AtomicBool::new(false);
            let snapshots = thread::scope(|scope| {
                let mut workers = Vec::new();
                for worker in 0..writers {
                    let lex = &lex;
                    let barrier = &barrier;
                    let measuring = &measuring;
                    let stop = &stop;
                    workers.push(scope.spawn(move || {
                        let mut durations = Vec::new();
                        barrier.wait();
                        for i in 0.. {
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            let key = format!("writer-{worker}-{i}");
                            // Keep writes that start during freeze even if they unblock afterward.
                            let during_freeze = measuring.load(Ordering::Acquire);
                            let start = Instant::now();
                            black_box(lex.intern(&key));
                            if during_freeze {
                                durations.push(start.elapsed().as_nanos() as u64);
                            }
                        }
                        durations
                    }));
                }
                barrier.wait();
                let mut retained = Vec::with_capacity(rounds);
                for round in 0..rounds {
                    let snapshot = lex.clone();
                    let id = format!("b3/shared/{shape}/{}/{writers}/{rounds}/{round}", words.len());
                    let frozen = measure(&id, 1, || {
                        measuring.store(true, Ordering::Release);
                        let frozen = snapshot.freeze();
                        measuring.store(false, Ordering::Release);
                        frozen
                    });
                    retained.push(frozen);
                }
                stop.store(true, Ordering::Release);
                let mut durations = Vec::new();
                for worker in workers {
                    durations.extend(worker.join().expect("writer only interns valid generated strings"));
                }
                for snapshot in &retained {
                    assert_snapshot(snapshot, words, writers);
                }
                durations.sort_unstable();
                if writers > 0 {
                    println!(
                        "{}",
                        json!({
                            "id": format!("b3/writer_stall/{shape}/{}/{writers}/{rounds}", words.len()),
                            "p95_writer_ns": (!durations.is_empty()).then(|| durations[durations.len() * 95 / 100]),
                            "p99_writer_ns": (!durations.is_empty()).then(|| durations[durations.len() * 99 / 100]),
                            "writer_samples": durations.len(),
                        })
                    );
                }
                retained
            });
            let post = measure(&format!("b3/post_insert/{shape}/{}/{writers}/{rounds}", words.len()), 1, || {
                lex.intern("post-snapshot-control")
            });
            let reader = lex.clone().freeze();
            measure(&format!("b3/post_resolve/{shape}/{}/{writers}/{rounds}", words.len()), 1, || {
                assert_eq!(reader.resolve(post), "post-snapshot-control");
            });
            black_box(snapshots);
        }
    }
}

fn main() {
    let quick = std::env::args().any(|arg| arg == "--quick");
    let freeze_smoke = std::env::args().any(|arg| arg == "--freeze-smoke");
    let words = corpus(6000);
    if freeze_smoke {
        freeze_scenarios(&words);
        return;
    }
    direct(&words);
    scans(&words);
    serde_workload(&words);
    wide_serde_workload(&words);
    if !quick {
        contention(&words, &FxBuildHasher, "fx");
        contention(&words, &FixedState::default(), "foldhash");
        freeze_scenarios(&words);
        let words = corpus(60_000);
        scans(&words);
        contention(&words, &FxBuildHasher, "fx");
        contention(&words, &FixedState::default(), "foldhash");
        freeze_scenarios(&words);
        let words = corpus(600_000);
        freeze_scenarios(&words);
    }
}
