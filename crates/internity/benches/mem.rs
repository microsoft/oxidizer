// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Memory-footprint measurement for each interner.
//!
//! Uses a tracking global allocator to report the **live heap bytes** each interner
//! holds after two phases, over the same ≈6000-identifier corpus as `compare.rs`:
//!
//! * **insert** — the filled interner (ready to intern more), including the dedup
//!   table and any spare `Vec`/`HashMap` capacity.
//! * **lookup** — the structure the lookup benchmark resolves against. For internity
//!   and lasso this is the *frozen* read form, which sheds the string→handle map;
//!   for the interners without a frozen form it is the same filled interner.
//!
//! Run with `cargo bench --bench mem`. The tracking allocator makes this binary
//! slow — it is a memory measurement, not a timing one.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::used_underscore_binding,
    reason = "benchmark harness code: index/stat casts and gungraun bindings are benign"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use internity::{Lexicon, ThreadedLexicon};

// ---------------------------------------------------------------------------
// Tracking allocator: counts net live (allocated − freed) bytes.
// ---------------------------------------------------------------------------

#[global_allocator]
static ALLOC: Tracking = Tracking;
static LIVE: AtomicUsize = AtomicUsize::new(0);

struct Tracking;

// SAFETY: every method forwards to `System` (a valid allocator) and only adjusts
// an atomic counter, preserving all `GlobalAlloc` invariants.
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` is forwarded unchanged from a valid caller.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` is forwarded unchanged from a valid caller.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr`/`layout` are forwarded unchanged from a valid caller.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: `ptr`/`layout`/`new_size` are forwarded unchanged from a valid caller.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            LIVE.fetch_add(new_size, Ordering::Relaxed);
        }
        p
    }
}

#[inline]
fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

/// Builds a value with `build`, returns the net live bytes it holds (intermediate
/// allocations freed inside `build` don't count), then drops it.
fn footprint<T>(build: impl FnOnce() -> T) -> usize {
    let before = live();
    let value = build();
    let bytes = live().saturating_sub(before);
    drop(value);
    bytes
}

// ---------------------------------------------------------------------------
// Corpus (identical to `compare.rs`).
// ---------------------------------------------------------------------------

fn corpus() -> Vec<String> {
    let mut state: u64 = 0x1234_5678_9abc_def0;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let mut out = Vec::new();
    while out.len() < 6000 {
        let len = 3 + (next() % 20) as usize;
        let mut s = String::with_capacity(len);
        for _ in 0..len {
            s.push(alphabet[(next() as usize) % alphabet.len()] as char);
        }
        out.push(s);
    }
    out.sort();
    out.dedup();
    out
}

fn kib(bytes: usize) -> String {
    format!("{:>8.1} KiB", bytes as f64 / 1024.0)
}

fn main() {
    let corpus = corpus();
    let corpus_bytes: usize = corpus.iter().map(String::len).sum();
    let n = corpus.len();
    println!(
        "Corpus: {n} distinct strings, {} of UTF-8 (avg {:.1} B/string)\n",
        kib(corpus_bytes),
        corpus_bytes as f64 / n as f64,
    );

    // Owned interners: `insert` is the filled interner; `lookup` is the read
    // structure the lookup benchmark resolves against (frozen where available).
    println!("== Owned interners — live heap held ==");
    println!("  {:<30} {:>12} {:>12}", "Interner", "insert", "lookup");
    two("internity  Lexicon", || fill_lexicon(&corpus), || fill_lexicon(&corpus).freeze());
    two(
        "internity  ThreadedLexicon",
        || fill_threaded(&corpus),
        || fill_threaded(&corpus).freeze(),
    );
    two("lasso", || fill_lasso(&corpus), || fill_lasso(&corpus).into_resolver());
    // No frozen read form: the lookup benchmark resolves the live interner.
    two("string-interner", || fill_si(&corpus), || fill_si(&corpus));
    two("symbol_table", || fill_symbol_table(&corpus), || fill_symbol_table(&corpus));

    // Process-global caches: one persistent table backs both phases, so `insert`
    // and `lookup` are the same. Measured once.
    println!("\n== Global caches — live heap held (persistent; insert == lookup) ==");
    row("ustr", footprint(|| fill_ustr(&corpus)));
    row("string_cache", footprint(|| fill_string_cache(&corpus)));
    println!(
        "\nNote: internity/lasso `lookup` is the frozen read form, which drops the \
         string→handle map;\n      `string-interner`/`symbol_table` have no frozen \
         form so `lookup` == `insert`."
    );
}

/// Prints one owned-interner row: the filled footprint and the read-form footprint.
fn two<A, B>(label: &str, insert: impl FnOnce() -> A, lookup: impl FnOnce() -> B) {
    let i = footprint(insert);
    let l = footprint(lookup);
    println!("  {label:<30} {} {}", kib(i), kib(l));
}

fn row(label: &str, bytes: usize) {
    println!("  {label:<30} {}", kib(bytes));
}

// ---------------------------------------------------------------------------
// Fillers — each returns the owned interner (or handles) so `footprint` can
// measure the live bytes it holds.
// ---------------------------------------------------------------------------

fn fill_lexicon(corpus: &[String]) -> Lexicon {
    let mut it = Lexicon::new();
    for s in corpus {
        it.intern(s);
    }
    it
}

fn fill_threaded(corpus: &[String]) -> ThreadedLexicon {
    let it = ThreadedLexicon::new();
    for s in corpus {
        it.intern(s);
    }
    it
}

fn fill_lasso(corpus: &[String]) -> lasso::Rodeo {
    let mut r = lasso::Rodeo::default();
    for s in corpus {
        r.get_or_intern(s);
    }
    r
}

fn fill_si(corpus: &[String]) -> string_interner::StringInterner<string_interner::DefaultBackend> {
    let mut si = string_interner::StringInterner::new();
    for s in corpus {
        si.get_or_intern(s);
    }
    si
}

fn fill_symbol_table(corpus: &[String]) -> symbol_table::SymbolTable {
    let st = symbol_table::SymbolTable::new();
    for s in corpus {
        st.intern(s);
    }
    st
}

/// `ustr` is a process-global cache: interning leaks into it forever. We return
/// nothing to hold, so the measured bytes are the global table's growth.
fn fill_ustr(corpus: &[String]) {
    for s in corpus {
        ustr::ustr(s);
    }
}

/// `string_cache` dynamic atoms are refcounted, so we keep the handles alive to
/// keep the atoms allocated; the measured bytes include those handles.
fn fill_string_cache(corpus: &[String]) -> Vec<string_cache::DefaultAtom> {
    corpus.iter().map(|s| string_cache::DefaultAtom::from(s.as_str())).collect()
}
