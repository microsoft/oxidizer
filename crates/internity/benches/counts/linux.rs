// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic instruction/cache-count benchmarks (Valgrind/Callgrind via
//! gungraun). One-shot and noise-free — the deterministic counterpart to the
//! wall-clock `compare` suite.
//!
//! There is one gungraun function per single-threaded criterion variant:
//! `<op>_<crate>` where `<op>` is `insert`, `reuse`, or `lookup`, matching the
//! criterion group names.
//! The `concurrent` criterion group has no gungraun counterpart — Callgrind runs
//! single-threaded and one-shot, so concurrent throughput has no instruction-count
//! analog.
//!
//! # Determinism
//!
//! Callgrind counts are only useful for regression gating if they are stable
//! run-to-run. internity's default hasher (FxHash) is fixed-seed, so it is already
//! deterministic. The competitors whose default hasher is *randomly seeded*
//! (lasso, string-interner) are constructed here with a **fixed-seed** foldhash so
//! their shard/bucket distribution — and thus any resize/allocation that lands on
//! the single measured op — is reproducible. `symbol_table` (seed 0), `ustr`, and
//! `string_cache` already hash deterministically.
//!
//! Every measured region contains only the one interning/lookup call. Setup
//! (interner construction + population) is passed as the (un-measured) bench
//! argument, and every function returns the interner/atoms it touched so their
//! (potentially expensive) drops happen outside the measured region.
//!
//! Run with `cargo bench --bench counts` (requires `gungraun-runner` + Valgrind).

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::used_underscore_binding,
    reason = "benchmark harness code: index/stat casts and gungraun bindings are benign"
)]

use std::hint::black_box;

use foldhash::fast::FixedState;
use gungraun::prelude::*;
use internity::{Lexicon, Reader, Sym, ThreadedLexicon};

/// A string that is present in every populated interner (dynamic atom in
/// string_cache: 33 bytes > 7, so not inline).
const KEY: &str = "a-representative-identifier-name";
/// A string that is *not* present (used for the miss / insert path).
const NEW: &str = "a-brand-new-string-not-yet-present-92714";
/// Number of filler strings pre-interned so the tables have a realistic load.
const N: u32 = 1000;

// Fixed-seed types for the randomly-seeded competitors so Callgrind counts are
// reproducible. internity's default FxHash is already fixed-seed.
type RodeoFixed = lasso::Rodeo<lasso::Spur, FixedState>;
type SiFixed = string_interner::StringInterner<string_interner::DefaultBackend, FixedState>;

// ---------------------------------------------------------------------------
// internity (default hasher = FxHash, deterministic)
// ---------------------------------------------------------------------------

fn pop_internity() -> Lexicon {
    let mut it = Lexicon::new();
    for i in 0..N {
        it.intern(format!("filler-{i}"));
    }
    it.intern(KEY);
    it
}

fn pop_internity_sym() -> (Lexicon, Sym) {
    let it = pop_internity();
    let sym = it.get(KEY).expect("KEY interned");
    (it, sym)
}

fn pop_internity_threaded() -> ThreadedLexicon {
    let it = ThreadedLexicon::new();
    for i in 0..N {
        it.intern(format!("filler-{i}"));
    }
    it.intern(KEY);
    it
}

#[library_benchmark]
#[bench::case(pop_internity())]
fn insert_internity(mut it: Lexicon) -> (Lexicon, Sym) {
    let sym = black_box(it.intern(black_box(NEW)));
    (it, sym)
}

#[library_benchmark]
#[bench::case(pop_internity())]
fn reuse_internity(mut it: Lexicon) -> (Lexicon, Sym) {
    let sym = black_box(it.intern(black_box(KEY)));
    (it, sym)
}

#[library_benchmark]
#[bench::case(pop_internity_threaded())]
fn insert_internity_threaded(it: ThreadedLexicon) -> (ThreadedLexicon, Sym) {
    let sym = black_box(it.intern(black_box(NEW)));
    (it, sym)
}

#[library_benchmark]
#[bench::case(pop_internity_threaded())]
fn reuse_internity_threaded(it: ThreadedLexicon) -> (ThreadedLexicon, Sym) {
    let sym = black_box(it.intern(black_box(KEY)));
    (it, sym)
}

#[library_benchmark]
#[bench::case(pop_internity_sym())]
fn lookup_internity(input: (Lexicon, Sym)) -> (Lexicon, usize) {
    let (it, sym) = input;
    let n = black_box(it.resolve(black_box(sym)).len());
    (it, n)
}

fn pop_frozen() -> (impl Reader, Sym) {
    let mut it = Lexicon::new();
    for i in 0..N {
        it.intern(format!("filler-{i}"));
    }
    let sym = it.intern(KEY);
    (it.freeze(), sym)
}

#[library_benchmark]
#[bench::case(pop_frozen())]
fn lookup_internity_frozen<R: Reader>(input: (R, Sym)) -> (R, usize) {
    let (r, sym) = input;
    let n = black_box(r.resolve(black_box(sym)).len());
    (r, n)
}

// ---------------------------------------------------------------------------
// lasso (single-threaded Rodeo)
// ---------------------------------------------------------------------------

fn pop_lasso() -> RodeoFixed {
    let mut r = RodeoFixed::with_hasher(FixedState::default());
    for i in 0..N {
        r.get_or_intern(format!("filler-{i}"));
    }
    r.get_or_intern(KEY);
    r
}

fn pop_lasso_sym() -> (RodeoFixed, lasso::Spur) {
    let r = pop_lasso();
    let sym = r.get(KEY).expect("KEY interned");
    (r, sym)
}

#[library_benchmark]
#[bench::case(pop_lasso())]
fn insert_lasso(mut r: RodeoFixed) -> (RodeoFixed, lasso::Spur) {
    let sym = black_box(r.get_or_intern(black_box(NEW)));
    (r, sym)
}

#[library_benchmark]
#[bench::case(pop_lasso())]
fn reuse_lasso(mut r: RodeoFixed) -> (RodeoFixed, lasso::Spur) {
    let sym = black_box(r.get_or_intern(black_box(KEY)));
    (r, sym)
}

#[library_benchmark]
#[bench::case(pop_lasso_sym())]
fn lookup_lasso(input: (RodeoFixed, lasso::Spur)) -> (RodeoFixed, usize) {
    let (r, sym) = input;
    let n = black_box(r.resolve(&black_box(sym)).len());
    (r, n)
}

// ---------------------------------------------------------------------------
// string-interner
// ---------------------------------------------------------------------------

fn pop_si() -> SiFixed {
    let mut si = SiFixed::with_hasher(FixedState::default());
    for i in 0..N {
        si.get_or_intern(format!("filler-{i}"));
    }
    si.get_or_intern(KEY);
    si
}

fn pop_si_sym() -> (SiFixed, string_interner::DefaultSymbol) {
    let si = pop_si();
    let sym = si.get(KEY).expect("KEY interned");
    (si, sym)
}

#[library_benchmark]
#[bench::case(pop_si())]
fn insert_string_interner(mut si: SiFixed) -> (SiFixed, string_interner::DefaultSymbol) {
    let sym = black_box(si.get_or_intern(black_box(NEW)));
    (si, sym)
}

#[library_benchmark]
#[bench::case(pop_si())]
fn reuse_string_interner(mut si: SiFixed) -> (SiFixed, string_interner::DefaultSymbol) {
    let sym = black_box(si.get_or_intern(black_box(KEY)));
    (si, sym)
}

#[library_benchmark]
#[bench::case(pop_si_sym())]
fn lookup_string_interner(input: (SiFixed, string_interner::DefaultSymbol)) -> (SiFixed, usize) {
    let (si, sym) = input;
    let n = black_box(si.resolve(black_box(sym)).expect("resolves").len());
    (si, n)
}

// ---------------------------------------------------------------------------
// symbol_table (deterministic foldhash, seed 0)
// ---------------------------------------------------------------------------

fn pop_st() -> symbol_table::SymbolTable {
    let st = symbol_table::SymbolTable::new();
    for i in 0..N {
        st.intern(&format!("filler-{i}"));
    }
    st.intern(KEY);
    st
}

fn pop_st_sym() -> (symbol_table::SymbolTable, symbol_table::Symbol) {
    let st = pop_st();
    let sym = st.intern(KEY);
    (st, sym)
}

#[library_benchmark]
#[bench::case(pop_st())]
fn insert_symbol_table(st: symbol_table::SymbolTable) -> (symbol_table::SymbolTable, symbol_table::Symbol) {
    let sym = black_box(st.intern(black_box(NEW)));
    (st, sym)
}

#[library_benchmark]
#[bench::case(pop_st())]
fn reuse_symbol_table(st: symbol_table::SymbolTable) -> (symbol_table::SymbolTable, symbol_table::Symbol) {
    let sym = black_box(st.intern(black_box(KEY)));
    (st, sym)
}

#[library_benchmark]
#[bench::case(pop_st_sym())]
fn lookup_symbol_table(input: (symbol_table::SymbolTable, symbol_table::Symbol)) -> (symbol_table::SymbolTable, usize) {
    let (st, sym) = input;
    let n = black_box(st.resolve(black_box(sym)).len());
    (st, n)
}

// ---------------------------------------------------------------------------
// ustr (process-global cache; handles are Copy, no drop)
// ---------------------------------------------------------------------------

/// Populates the global cache with filler + KEY and returns KEY's handle.
fn pop_ustr() -> ustr::Ustr {
    for i in 0..N {
        ustr::ustr(&format!("filler-{i}"));
    }
    ustr::ustr(KEY)
}

#[library_benchmark]
#[bench::case(pop_ustr())]
fn insert_ustr(_seed: ustr::Ustr) -> ustr::Ustr {
    // NEW is unseen in this fresh process, so this is a genuine insert.
    black_box(ustr::ustr(black_box(NEW)))
}

#[library_benchmark]
#[bench::case(pop_ustr())]
fn reuse_ustr(_seed: ustr::Ustr) -> ustr::Ustr {
    black_box(ustr::ustr(black_box(KEY)))
}

#[library_benchmark]
#[bench::case(pop_ustr())]
fn lookup_ustr(u: ustr::Ustr) -> usize {
    black_box(u.as_str().len())
}

// ---------------------------------------------------------------------------
// string_cache (process-global; DefaultAtom has a refcounting Drop, so returned)
// ---------------------------------------------------------------------------

fn pop_string_cache() -> string_cache::DefaultAtom {
    for i in 0..N {
        let _ = string_cache::DefaultAtom::from(format!("filler-{i}").as_str());
    }
    string_cache::DefaultAtom::from(KEY)
}

#[library_benchmark]
#[bench::case(pop_string_cache())]
fn insert_string_cache(seed: string_cache::DefaultAtom) -> (string_cache::DefaultAtom, string_cache::DefaultAtom) {
    let a = black_box(string_cache::DefaultAtom::from(black_box(NEW)));
    (seed, a)
}

#[library_benchmark]
#[bench::case(pop_string_cache())]
fn reuse_string_cache(seed: string_cache::DefaultAtom) -> (string_cache::DefaultAtom, string_cache::DefaultAtom) {
    let a = black_box(string_cache::DefaultAtom::from(black_box(KEY)));
    // Return both so neither atom's (refcounting) drop is measured.
    (seed, a)
}

#[library_benchmark]
#[bench::case(pop_string_cache())]
fn lookup_string_cache(a: string_cache::DefaultAtom) -> (string_cache::DefaultAtom, usize) {
    let n = black_box((a.as_ref() as &str).len());
    (a, n)
}

library_benchmark_group!(
    name = ops,
    benchmarks = [
        insert_internity,
        reuse_internity,
        insert_internity_threaded,
        reuse_internity_threaded,
        lookup_internity,
        lookup_internity_frozen,
        insert_lasso,
        reuse_lasso,
        lookup_lasso,
        insert_string_interner,
        reuse_string_interner,
        lookup_string_interner,
        insert_symbol_table,
        reuse_symbol_table,
        lookup_symbol_table,
        insert_ustr,
        reuse_ustr,
        lookup_ustr,
        insert_string_cache,
        reuse_string_cache,
        lookup_string_cache,
    ]
);
