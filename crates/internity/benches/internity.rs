// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consolidated benchmark suite for Internity's wall-clock and Callgrind checks.
//!
//! The customer-facing report still publishes Criterion wall-clock timings plus
//! the dedicated live-heap measurement from `internity_mem`. This target
//! consolidates the former Criterion and Callgrind benches into one metabench
//! binary so both engines share one build and one report format.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::used_underscore_binding,
    reason = "benchmark harness code: index/stat casts and benchmark-generated bindings are benign"
)]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    clippy::unnecessary_wraps,
    unreachable_pub,
    unused_qualifications,
    reason = "Triggered by benchmark macro expansion on every platform; the engines themselves decide what runs at runtime."
)]

use std::collections::BTreeSet;
use std::hint::black_box;
use std::sync::Barrier;
use std::thread;
use std::time::{Duration, Instant};

use criterion::{BatchSize, Criterion, Throughput};
use foldhash::fast::FixedState;
use gungraun::LibraryBenchmarkConfig;
use internity::{LocalLexicon, Reader, Sym, ThreadedLexicon};

type Si = string_interner::StringInterner<string_interner::DefaultBackend>;
type RodeoFixed = lasso::Rodeo<lasso::Spur, FixedState>;
type SiFixed = string_interner::StringInterner<string_interner::DefaultBackend, FixedState>;

const DEFAULT_CORPUS_SIZE: usize = 6000;
const CORPUS_SIZE_ENV: &str = "INTERNITY_BENCH_CORPUS_SIZE";

const INSERT_GROUP: &str = "internity_compare/insert";
const REUSE_GROUP: &str = "internity_compare/reuse";
const LOOKUP_GROUP: &str = "internity_compare/lookup";
const INSERT_CONCURRENT_GROUP: &str = "internity_compare/insert-concurrent";
const REUSE_CONCURRENT_GROUP: &str = "internity_compare/reuse-concurrent";
const LOOKUP_CONCURRENT_GROUP: &str = "internity_compare/lookup-concurrent";

/// A string present in every populated single-operation benchmark state.
const KEY: &str = "a-representative-identifier-name";
/// A string absent from every populated single-operation benchmark state.
const NEW: &str = "a-brand-new-string-not-yet-present-92714";
/// Filler strings pre-interned before the single-operation benchmarks.
const HOT_PATH_FILLER_COUNT: u32 = 1000;

fn corpus_size() -> usize {
    let Ok(value) = std::env::var(CORPUS_SIZE_ENV) else {
        return DEFAULT_CORPUS_SIZE;
    };
    let Ok(size) = value.parse() else {
        eprintln!("{CORPUS_SIZE_ENV} must be a positive integer, got {value:?}");
        std::process::exit(2);
    };
    if size == 0 {
        eprintln!("{CORPUS_SIZE_ENV} must be greater than zero");
        std::process::exit(2);
    }
    size
}

/// Deterministic corpus of identifier-like strings (lengths 3..=22).
fn corpus() -> Vec<String> {
    let corpus_size = corpus_size();
    let mut state: u64 = 0x1234_5678_9abc_def0;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let mut out = BTreeSet::new();
    while out.len() < corpus_size {
        let len = 3 + (next() % 20) as usize;
        let mut s = String::with_capacity(len);
        for _ in 0..len {
            s.push(alphabet[(next() as usize) % alphabet.len()] as char);
        }
        out.insert(s);
    }
    out.into_iter().collect()
}

/// Deterministic random permutation of `0..n` (Fisher–Yates with xorshift).
fn permutation(n: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    let mut state: u64 = 0xdead_beef_cafe_f00d;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for i in (1..n).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    order
}

/// Times only the concurrent-intern work for a fresh interner per round.
fn concurrent_fill<T, C, I>(iters: u64, threads: usize, corpus: &[String], construct: C, intern: I) -> Duration
where
    T: Sync,
    C: Fn() -> T,
    I: Fn(&T, &str) + Sync,
{
    let mut total = Duration::ZERO;
    for _ in 0..iters {
        let interner = construct();
        let start_barrier = Barrier::new(threads + 1);
        let end_barrier = Barrier::new(threads + 1);
        let intern = &intern;
        let interner = &interner;
        let round = thread::scope(|scope| {
            for worker in 0..threads {
                let start = corpus.len() * worker / threads;
                let end = corpus.len() * (worker + 1) / threads;
                let chunk = &corpus[start..end];
                let start_barrier = &start_barrier;
                let end_barrier = &end_barrier;
                scope.spawn(move || {
                    start_barrier.wait();
                    for string in chunk {
                        intern(interner, string);
                    }
                    end_barrier.wait();
                });
            }
            start_barrier.wait();
            let started = Instant::now();
            end_barrier.wait();
            started.elapsed()
        });
        total += round;
    }
    total
}

/// Times only the barrier-delimited parallel work with per-thread untimed setup.
fn timed_parallel_with_setup<S>(iters: u64, threads: usize, setup: impl Fn() -> S + Sync, work: impl Fn(&mut S) + Sync) -> Duration {
    let mut total = Duration::ZERO;
    let setup = &setup;
    let work = &work;
    for _ in 0..iters {
        let start_barrier = Barrier::new(threads + 1);
        let end_barrier = Barrier::new(threads + 1);
        let round = thread::scope(|scope| {
            for _ in 0..threads {
                let start_barrier = &start_barrier;
                let end_barrier = &end_barrier;
                scope.spawn(move || {
                    let mut state = setup();
                    start_barrier.wait();
                    work(&mut state);
                    end_barrier.wait();
                    black_box(&state);
                });
            }
            start_barrier.wait();
            let started = Instant::now();
            end_barrier.wait();
            started.elapsed()
        });
        total += round;
    }
    total
}

fn timed_parallel(iters: u64, threads: usize, work: impl Fn() + Sync) -> Duration {
    timed_parallel_with_setup(iters, threads, || false, |_| work())
}

fn timed_parallel_collect<R>(iters: u64, threads: usize, corpus: &[String], op: impl Fn(&str) -> R + Sync) -> Duration {
    timed_parallel_with_setup(
        iters,
        threads,
        || Vec::with_capacity(corpus.len()),
        |results| {
            for string in corpus {
                results.push(black_box(op(string)));
            }
        },
    )
}

fn gungraun_default_config() -> LibraryBenchmarkConfig {
    LibraryBenchmarkConfig::default()
}

fn populate_local() -> LocalLexicon {
    let mut interner = LocalLexicon::new();
    for index in 0..HOT_PATH_FILLER_COUNT {
        interner.intern(format!("filler-{index}"));
    }
    interner.intern(KEY);
    interner
}

fn populate_local_with_sym() -> (LocalLexicon, Sym) {
    let interner = populate_local();
    let sym = interner.get(KEY).expect("KEY is interned by populate_local");
    (interner, sym)
}

fn populate_threaded() -> ThreadedLexicon {
    let interner = ThreadedLexicon::new();
    for index in 0..HOT_PATH_FILLER_COUNT {
        interner.intern(format!("filler-{index}"));
    }
    interner.intern(KEY);
    interner
}

fn populate_frozen() -> (impl Reader, Sym) {
    let mut interner = LocalLexicon::new();
    for index in 0..HOT_PATH_FILLER_COUNT {
        interner.intern(format!("filler-{index}"));
    }
    let sym = interner.intern(KEY);
    (interner.freeze(), sym)
}

fn populate_lasso() -> RodeoFixed {
    let mut rodeo = RodeoFixed::with_hasher(FixedState::default());
    for index in 0..HOT_PATH_FILLER_COUNT {
        rodeo.get_or_intern(format!("filler-{index}"));
    }
    rodeo.get_or_intern(KEY);
    rodeo
}

fn populate_lasso_with_sym() -> (RodeoFixed, lasso::Spur) {
    let rodeo = populate_lasso();
    let sym = rodeo.get(KEY).expect("KEY is interned by populate_lasso");
    (rodeo, sym)
}

fn populate_string_interner() -> SiFixed {
    let mut interner = SiFixed::with_hasher(FixedState::default());
    for index in 0..HOT_PATH_FILLER_COUNT {
        interner.get_or_intern(format!("filler-{index}"));
    }
    interner.get_or_intern(KEY);
    interner
}

fn populate_string_interner_with_sym() -> (SiFixed, string_interner::DefaultSymbol) {
    let interner = populate_string_interner();
    let sym = interner.get(KEY).expect("KEY is interned by populate_string_interner");
    (interner, sym)
}

fn populate_symbol_table() -> symbol_table::SymbolTable {
    let table = symbol_table::SymbolTable::new();
    for index in 0..HOT_PATH_FILLER_COUNT {
        table.intern(&format!("filler-{index}"));
    }
    table.intern(KEY);
    table
}

fn populate_symbol_table_with_sym() -> (symbol_table::SymbolTable, symbol_table::Symbol) {
    let table = populate_symbol_table();
    let sym = table.intern(KEY);
    (table, sym)
}

fn populate_ustr() -> ustr::Ustr {
    for index in 0..HOT_PATH_FILLER_COUNT {
        ustr::ustr(&format!("filler-{index}"));
    }
    ustr::ustr(KEY)
}

struct StringCacheState {
    fillers: Vec<string_cache::DefaultAtom>,
    key: string_cache::DefaultAtom,
}

fn populate_string_cache() -> StringCacheState {
    let mut fillers = Vec::with_capacity(HOT_PATH_FILLER_COUNT as usize);
    for index in 0..HOT_PATH_FILLER_COUNT {
        let filler = format!("filler-{index}");
        fillers.push(string_cache::DefaultAtom::from(filler.as_str()));
    }
    let state = StringCacheState {
        fillers,
        key: string_cache::DefaultAtom::from(KEY),
    };
    debug_assert_eq!(state.fillers.len(), HOT_PATH_FILLER_COUNT as usize);
    state
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let corpus = corpus();
    let mut insert = criterion.benchmark_group(INSERT_INTERNITY.group_name());
    insert.throughput(Throughput::Elements(corpus.len() as u64));

    insert.bench_function(INSERT_INTERNITY.benchmark_name(), |bencher| {
        bencher.iter_batched(
            LocalLexicon::new,
            |mut interner| {
                for string in &corpus {
                    black_box(interner.intern(string));
                }
                interner
            },
            BatchSize::LargeInput,
        );
    });
    insert.bench_function(INSERT_INTERNITY_THREADED.benchmark_name(), |bencher| {
        bencher.iter_batched(
            ThreadedLexicon::new,
            |interner| {
                for string in &corpus {
                    black_box(interner.intern(string));
                }
                interner
            },
            BatchSize::LargeInput,
        );
    });
    insert.bench_function(INSERT_LASSO.benchmark_name(), |bencher| {
        bencher.iter_batched(
            lasso::Rodeo::default,
            |mut rodeo| {
                for string in &corpus {
                    black_box(rodeo.get_or_intern(string));
                }
                rodeo
            },
            BatchSize::LargeInput,
        );
    });
    insert.bench_function(INSERT_STRING_INTERNER.benchmark_name(), |bencher| {
        bencher.iter_batched(
            Si::new,
            |mut interner| {
                for string in &corpus {
                    black_box(interner.get_or_intern(string));
                }
                interner
            },
            BatchSize::LargeInput,
        );
    });
    insert.bench_function(INSERT_SYMBOL_TABLE.benchmark_name(), |bencher| {
        bencher.iter_batched(
            symbol_table::SymbolTable::new,
            |table| {
                for string in &corpus {
                    black_box(table.intern(string));
                }
                table
            },
            BatchSize::LargeInput,
        );
    });
    insert.finish();

    let mut reuse = criterion.benchmark_group(REUSE_INTERNITY.group_name());
    reuse.throughput(Throughput::Elements(corpus.len() as u64));

    let mut local = LocalLexicon::new();
    for string in &corpus {
        local.intern(string);
    }
    reuse.bench_function(REUSE_INTERNITY.benchmark_name(), |bencher| {
        bencher.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for string in &corpus {
                    results.push(black_box(local.intern(string)));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });

    let threaded = ThreadedLexicon::new();
    for string in &corpus {
        threaded.intern(string);
    }
    reuse.bench_function(REUSE_INTERNITY_THREADED.benchmark_name(), |bencher| {
        bencher.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for string in &corpus {
                    results.push(black_box(threaded.intern(string)));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });

    let mut lasso = lasso::Rodeo::default();
    for string in &corpus {
        lasso.get_or_intern(string);
    }
    reuse.bench_function(REUSE_LASSO.benchmark_name(), |bencher| {
        bencher.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for string in &corpus {
                    results.push(black_box(lasso.get_or_intern(string)));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });

    let mut string_interner = Si::new();
    for string in &corpus {
        string_interner.get_or_intern(string);
    }
    reuse.bench_function(REUSE_STRING_INTERNER.benchmark_name(), |bencher| {
        bencher.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for string in &corpus {
                    results.push(black_box(string_interner.get_or_intern(string)));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });

    let symbol_table = symbol_table::SymbolTable::new();
    for string in &corpus {
        symbol_table.intern(string);
    }
    reuse.bench_function(REUSE_SYMBOL_TABLE.benchmark_name(), |bencher| {
        bencher.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for string in &corpus {
                    results.push(black_box(symbol_table.intern(string)));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });

    for string in &corpus {
        ustr::ustr(string);
    }
    reuse.bench_function(REUSE_USTR.benchmark_name(), |bencher| {
        bencher.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for string in &corpus {
                    results.push(black_box(ustr::ustr(string)));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });

    let _string_cache_atoms: Vec<string_cache::DefaultAtom> = corpus
        .iter()
        .map(|string| string_cache::DefaultAtom::from(string.as_str()))
        .collect();
    reuse.bench_function(REUSE_STRING_CACHE.benchmark_name(), |bencher| {
        bencher.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for string in &corpus {
                    results.push(black_box(string_cache::DefaultAtom::from(string.as_str())));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });
    reuse.finish();

    let order = permutation(corpus.len());
    let mut lookup = criterion.benchmark_group(LOOKUP_INTERNITY.group_name());
    lookup.throughput(Throughput::Elements(corpus.len() as u64));

    let mut live = LocalLexicon::new();
    let live_syms: Vec<_> = corpus.iter().map(|string| live.intern(string)).collect();
    lookup.bench_function(LOOKUP_INTERNITY.benchmark_name(), |bencher| {
        bencher.iter(|| {
            for &index in &order {
                black_box(live.resolve(live_syms[index]));
            }
        });
    });

    let mut frozen = LocalLexicon::new();
    let frozen_syms: Vec<_> = corpus.iter().map(|string| frozen.intern(string)).collect();
    let frozen = frozen.freeze();
    lookup.bench_function(LOOKUP_INTERNITY_FROZEN.benchmark_name(), |bencher| {
        bencher.iter(|| {
            for &index in &order {
                black_box(frozen.resolve(frozen_syms[index]));
            }
        });
    });

    let mut lasso = lasso::Rodeo::default();
    let lasso_syms: Vec<_> = corpus.iter().map(|string| lasso.get_or_intern(string)).collect();
    lookup.bench_function(LOOKUP_LASSO.benchmark_name(), |bencher| {
        bencher.iter(|| {
            for &index in &order {
                black_box(lasso.resolve(&lasso_syms[index]));
            }
        });
    });

    let mut string_interner = Si::new();
    let string_interner_syms: Vec<_> = corpus.iter().map(|string| string_interner.get_or_intern(string)).collect();
    lookup.bench_function(LOOKUP_STRING_INTERNER.benchmark_name(), |bencher| {
        bencher.iter(|| {
            for &index in &order {
                black_box(
                    string_interner
                        .resolve(string_interner_syms[index])
                        .expect("symbol was produced by this interner"),
                );
            }
        });
    });

    let symbol_table = symbol_table::SymbolTable::new();
    let symbol_table_syms: Vec<_> = corpus.iter().map(|string| symbol_table.intern(string)).collect();
    lookup.bench_function(LOOKUP_SYMBOL_TABLE.benchmark_name(), |bencher| {
        bencher.iter(|| {
            for &index in &order {
                black_box(symbol_table.resolve(symbol_table_syms[index]));
            }
        });
    });

    let ustrs: Vec<ustr::Ustr> = corpus.iter().map(|string| ustr::ustr(string)).collect();
    lookup.bench_function(LOOKUP_USTR.benchmark_name(), |bencher| {
        bencher.iter(|| {
            for &index in &order {
                black_box(ustrs[index].as_str());
            }
        });
    });

    let atoms: Vec<string_cache::DefaultAtom> = corpus
        .iter()
        .map(|string| string_cache::DefaultAtom::from(string.as_str()))
        .collect();
    lookup.bench_function(LOOKUP_STRING_CACHE.benchmark_name(), |bencher| {
        bencher.iter(|| {
            for &index in &order {
                black_box(atoms[index].as_ref() as &str);
            }
        });
    });
    lookup.finish();

    let mut insert_concurrent = criterion.benchmark_group(INSERT_CONCURRENT_GROUP);
    for threads in [1usize, 2, 4, 8] {
        insert_concurrent.throughput(Throughput::Elements(corpus.len() as u64));
        match threads {
            1 => {
                insert_concurrent.bench_function("internity/1", |bencher| {
                    bencher.iter_custom(|iters| {
                        concurrent_fill(iters, threads, &corpus, ThreadedLexicon::new, |interner, string| {
                            black_box(interner.intern(string));
                        })
                    });
                });
                insert_concurrent.bench_function("lasso-threaded/1", |bencher| {
                    bencher.iter_custom(|iters| {
                        concurrent_fill(iters, threads, &corpus, lasso::ThreadedRodeo::default, |rodeo, string| {
                            black_box(rodeo.get_or_intern(string));
                        })
                    });
                });
                insert_concurrent.bench_function("symbol_table/1", |bencher| {
                    bencher.iter_custom(|iters| {
                        concurrent_fill(iters, threads, &corpus, symbol_table::SymbolTable::new, |table, string| {
                            black_box(table.intern(string));
                        })
                    });
                });
            }
            2 => {
                insert_concurrent.bench_function("internity/2", |bencher| {
                    bencher.iter_custom(|iters| {
                        concurrent_fill(iters, threads, &corpus, ThreadedLexicon::new, |interner, string| {
                            black_box(interner.intern(string));
                        })
                    });
                });
                insert_concurrent.bench_function("lasso-threaded/2", |bencher| {
                    bencher.iter_custom(|iters| {
                        concurrent_fill(iters, threads, &corpus, lasso::ThreadedRodeo::default, |rodeo, string| {
                            black_box(rodeo.get_or_intern(string));
                        })
                    });
                });
                insert_concurrent.bench_function("symbol_table/2", |bencher| {
                    bencher.iter_custom(|iters| {
                        concurrent_fill(iters, threads, &corpus, symbol_table::SymbolTable::new, |table, string| {
                            black_box(table.intern(string));
                        })
                    });
                });
            }
            4 => {
                insert_concurrent.bench_function("internity/4", |bencher| {
                    bencher.iter_custom(|iters| {
                        concurrent_fill(iters, threads, &corpus, ThreadedLexicon::new, |interner, string| {
                            black_box(interner.intern(string));
                        })
                    });
                });
                insert_concurrent.bench_function("lasso-threaded/4", |bencher| {
                    bencher.iter_custom(|iters| {
                        concurrent_fill(iters, threads, &corpus, lasso::ThreadedRodeo::default, |rodeo, string| {
                            black_box(rodeo.get_or_intern(string));
                        })
                    });
                });
                insert_concurrent.bench_function("symbol_table/4", |bencher| {
                    bencher.iter_custom(|iters| {
                        concurrent_fill(iters, threads, &corpus, symbol_table::SymbolTable::new, |table, string| {
                            black_box(table.intern(string));
                        })
                    });
                });
            }
            8 => {
                insert_concurrent.bench_function("internity/8", |bencher| {
                    bencher.iter_custom(|iters| {
                        concurrent_fill(iters, threads, &corpus, ThreadedLexicon::new, |interner, string| {
                            black_box(interner.intern(string));
                        })
                    });
                });
                insert_concurrent.bench_function("lasso-threaded/8", |bencher| {
                    bencher.iter_custom(|iters| {
                        concurrent_fill(iters, threads, &corpus, lasso::ThreadedRodeo::default, |rodeo, string| {
                            black_box(rodeo.get_or_intern(string));
                        })
                    });
                });
                insert_concurrent.bench_function("symbol_table/8", |bencher| {
                    bencher.iter_custom(|iters| {
                        concurrent_fill(iters, threads, &corpus, symbol_table::SymbolTable::new, |table, string| {
                            black_box(table.intern(string));
                        })
                    });
                });
            }
            _ => unreachable!("only compile-time thread counts are registered"),
        }
    }
    insert_concurrent.finish();

    let mut reuse_concurrent = criterion.benchmark_group(REUSE_CONCURRENT_GROUP);
    let threaded = ThreadedLexicon::new();
    let lasso = lasso::ThreadedRodeo::default();
    let symbol_table = symbol_table::SymbolTable::new();
    for string in &corpus {
        threaded.intern(string);
        lasso.get_or_intern(string);
        symbol_table.intern(string);
        ustr::ustr(string);
    }
    let _string_cache_atoms: Vec<string_cache::DefaultAtom> = corpus
        .iter()
        .map(|string| string_cache::DefaultAtom::from(string.as_str()))
        .collect();

    for threads in [1usize, 2, 4, 8] {
        reuse_concurrent.throughput(Throughput::Elements((threads * corpus.len()) as u64));
        match threads {
            1 => {
                reuse_concurrent.bench_function("internity/1", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, |string| threaded.intern(string)));
                });
                reuse_concurrent.bench_function("lasso-threaded/1", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, |string| lasso.get_or_intern(string)));
                });
                reuse_concurrent.bench_function("symbol_table/1", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, |string| symbol_table.intern(string)));
                });
                reuse_concurrent.bench_function("ustr/1", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, ustr::ustr));
                });
                reuse_concurrent.bench_function("string_cache/1", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel_collect(iters, threads, &corpus, |string| string_cache::DefaultAtom::from(string))
                    });
                });
            }
            2 => {
                reuse_concurrent.bench_function("internity/2", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, |string| threaded.intern(string)));
                });
                reuse_concurrent.bench_function("lasso-threaded/2", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, |string| lasso.get_or_intern(string)));
                });
                reuse_concurrent.bench_function("symbol_table/2", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, |string| symbol_table.intern(string)));
                });
                reuse_concurrent.bench_function("ustr/2", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, ustr::ustr));
                });
                reuse_concurrent.bench_function("string_cache/2", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel_collect(iters, threads, &corpus, |string| string_cache::DefaultAtom::from(string))
                    });
                });
            }
            4 => {
                reuse_concurrent.bench_function("internity/4", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, |string| threaded.intern(string)));
                });
                reuse_concurrent.bench_function("lasso-threaded/4", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, |string| lasso.get_or_intern(string)));
                });
                reuse_concurrent.bench_function("symbol_table/4", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, |string| symbol_table.intern(string)));
                });
                reuse_concurrent.bench_function("ustr/4", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, ustr::ustr));
                });
                reuse_concurrent.bench_function("string_cache/4", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel_collect(iters, threads, &corpus, |string| string_cache::DefaultAtom::from(string))
                    });
                });
            }
            8 => {
                reuse_concurrent.bench_function("internity/8", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, |string| threaded.intern(string)));
                });
                reuse_concurrent.bench_function("lasso-threaded/8", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, |string| lasso.get_or_intern(string)));
                });
                reuse_concurrent.bench_function("symbol_table/8", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, |string| symbol_table.intern(string)));
                });
                reuse_concurrent.bench_function("ustr/8", |bencher| {
                    bencher.iter_custom(|iters| timed_parallel_collect(iters, threads, &corpus, ustr::ustr));
                });
                reuse_concurrent.bench_function("string_cache/8", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel_collect(iters, threads, &corpus, |string| string_cache::DefaultAtom::from(string))
                    });
                });
            }
            _ => unreachable!("only compile-time thread counts are registered"),
        }
    }
    reuse_concurrent.finish();

    let mut lookup_concurrent = criterion.benchmark_group(LOOKUP_CONCURRENT_GROUP);
    let order = permutation(corpus.len());

    let threaded = ThreadedLexicon::new();
    let threaded_syms: Vec<_> = corpus.iter().map(|string| threaded.intern(string)).collect();
    let threaded_reader = threaded.freeze();

    let lasso = lasso::ThreadedRodeo::default();
    let lasso_syms: Vec<_> = corpus.iter().map(|string| lasso.get_or_intern(string)).collect();
    let lasso_resolver = lasso.into_resolver();

    let symbol_table = symbol_table::SymbolTable::new();
    let symbol_table_syms: Vec<_> = corpus.iter().map(|string| symbol_table.intern(string)).collect();

    let ustrs: Vec<ustr::Ustr> = corpus.iter().map(|string| ustr::ustr(string)).collect();
    let atoms: Vec<string_cache::DefaultAtom> = corpus
        .iter()
        .map(|string| string_cache::DefaultAtom::from(string.as_str()))
        .collect();

    for threads in [1usize, 2, 4, 8] {
        lookup_concurrent.throughput(Throughput::Elements((threads * corpus.len()) as u64));
        match threads {
            1 => {
                lookup_concurrent.bench_function("internity/1", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(threaded_reader.resolve(threaded_syms[index]));
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("lasso-resolver/1", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(lasso_resolver.resolve(&lasso_syms[index]));
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("symbol_table/1", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(symbol_table.resolve(symbol_table_syms[index]));
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("ustr/1", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(ustrs[index].as_str());
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("string_cache/1", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(atoms[index].as_ref() as &str);
                            }
                        })
                    });
                });
            }
            2 => {
                lookup_concurrent.bench_function("internity/2", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(threaded_reader.resolve(threaded_syms[index]));
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("lasso-resolver/2", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(lasso_resolver.resolve(&lasso_syms[index]));
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("symbol_table/2", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(symbol_table.resolve(symbol_table_syms[index]));
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("ustr/2", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(ustrs[index].as_str());
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("string_cache/2", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(atoms[index].as_ref() as &str);
                            }
                        })
                    });
                });
            }
            4 => {
                lookup_concurrent.bench_function("internity/4", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(threaded_reader.resolve(threaded_syms[index]));
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("lasso-resolver/4", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(lasso_resolver.resolve(&lasso_syms[index]));
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("symbol_table/4", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(symbol_table.resolve(symbol_table_syms[index]));
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("ustr/4", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(ustrs[index].as_str());
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("string_cache/4", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(atoms[index].as_ref() as &str);
                            }
                        })
                    });
                });
            }
            8 => {
                lookup_concurrent.bench_function("internity/8", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(threaded_reader.resolve(threaded_syms[index]));
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("lasso-resolver/8", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(lasso_resolver.resolve(&lasso_syms[index]));
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("symbol_table/8", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(symbol_table.resolve(symbol_table_syms[index]));
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("ustr/8", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(ustrs[index].as_str());
                            }
                        })
                    });
                });
                lookup_concurrent.bench_function("string_cache/8", |bencher| {
                    bencher.iter_custom(|iters| {
                        timed_parallel(iters, threads, || {
                            for &index in &order {
                                black_box(atoms[index].as_ref() as &str);
                            }
                        })
                    });
                });
            }
            _ => unreachable!("only compile-time thread counts are registered"),
        }
    }
    lookup_concurrent.finish();
}

#[metabench::benchmark(
    INSERT_INTERNITY,
    INSERT_GROUP,
    "internity",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_local())]
fn insert_internity_hot(mut interner: LocalLexicon) -> (LocalLexicon, Sym) {
    let sym = black_box(interner.intern(black_box(NEW)));
    (interner, sym)
}

#[metabench::benchmark(
    INSERT_INTERNITY_THREADED,
    INSERT_GROUP,
    "internity-threaded",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_threaded())]
fn insert_internity_threaded_hot(interner: ThreadedLexicon) -> (ThreadedLexicon, Sym) {
    let sym = black_box(interner.intern(black_box(NEW)));
    (interner, sym)
}

#[metabench::benchmark(INSERT_LASSO, INSERT_GROUP, "lasso", gungraun_config = gungraun_default_config())]
#[bench::run(populate_lasso())]
fn insert_lasso_hot(mut rodeo: RodeoFixed) -> (RodeoFixed, lasso::Spur) {
    let sym = black_box(rodeo.get_or_intern(black_box(NEW)));
    (rodeo, sym)
}

#[metabench::benchmark(
    INSERT_STRING_INTERNER,
    INSERT_GROUP,
    "string-interner",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_string_interner())]
fn insert_string_interner_hot(mut interner: SiFixed) -> (SiFixed, string_interner::DefaultSymbol) {
    let sym = black_box(interner.get_or_intern(black_box(NEW)));
    (interner, sym)
}

#[metabench::benchmark(
    INSERT_SYMBOL_TABLE,
    INSERT_GROUP,
    "symbol_table",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_symbol_table())]
fn insert_symbol_table_hot(table: symbol_table::SymbolTable) -> (symbol_table::SymbolTable, symbol_table::Symbol) {
    let sym = black_box(table.intern(black_box(NEW)));
    (table, sym)
}

#[metabench::benchmark(INSERT_USTR, INSERT_GROUP, "ustr", gungraun_config = gungraun_default_config())]
#[bench::run(populate_ustr())]
fn insert_ustr_hot(_seed: ustr::Ustr) -> ustr::Ustr {
    black_box(ustr::ustr(black_box(NEW)))
}

#[metabench::benchmark(
    INSERT_STRING_CACHE,
    INSERT_GROUP,
    "string_cache",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_string_cache())]
fn insert_string_cache_hot(state: StringCacheState) -> (StringCacheState, string_cache::DefaultAtom) {
    let atom = black_box(string_cache::DefaultAtom::from(black_box(NEW)));
    (state, atom)
}

#[metabench::benchmark(
    REUSE_INTERNITY,
    REUSE_GROUP,
    "internity",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_local())]
fn reuse_internity_hot(mut interner: LocalLexicon) -> (LocalLexicon, Sym) {
    let sym = black_box(interner.intern(black_box(KEY)));
    (interner, sym)
}

#[metabench::benchmark(
    REUSE_INTERNITY_THREADED,
    REUSE_GROUP,
    "internity-threaded",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_threaded())]
fn reuse_internity_threaded_hot(interner: ThreadedLexicon) -> (ThreadedLexicon, Sym) {
    let sym = black_box(interner.intern(black_box(KEY)));
    (interner, sym)
}

#[metabench::benchmark(REUSE_LASSO, REUSE_GROUP, "lasso", gungraun_config = gungraun_default_config())]
#[bench::run(populate_lasso())]
fn reuse_lasso_hot(mut rodeo: RodeoFixed) -> (RodeoFixed, lasso::Spur) {
    let sym = black_box(rodeo.get_or_intern(black_box(KEY)));
    (rodeo, sym)
}

#[metabench::benchmark(
    REUSE_STRING_INTERNER,
    REUSE_GROUP,
    "string-interner",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_string_interner())]
fn reuse_string_interner_hot(mut interner: SiFixed) -> (SiFixed, string_interner::DefaultSymbol) {
    let sym = black_box(interner.get_or_intern(black_box(KEY)));
    (interner, sym)
}

#[metabench::benchmark(
    REUSE_SYMBOL_TABLE,
    REUSE_GROUP,
    "symbol_table",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_symbol_table())]
fn reuse_symbol_table_hot(table: symbol_table::SymbolTable) -> (symbol_table::SymbolTable, symbol_table::Symbol) {
    let sym = black_box(table.intern(black_box(KEY)));
    (table, sym)
}

#[metabench::benchmark(REUSE_USTR, REUSE_GROUP, "ustr", gungraun_config = gungraun_default_config())]
#[bench::run(populate_ustr())]
fn reuse_ustr_hot(_seed: ustr::Ustr) -> ustr::Ustr {
    black_box(ustr::ustr(black_box(KEY)))
}

#[metabench::benchmark(
    REUSE_STRING_CACHE,
    REUSE_GROUP,
    "string_cache",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_string_cache())]
fn reuse_string_cache_hot(state: StringCacheState) -> (StringCacheState, string_cache::DefaultAtom) {
    let atom = black_box(string_cache::DefaultAtom::from(black_box(KEY)));
    (state, atom)
}

#[metabench::benchmark(
    LOOKUP_INTERNITY,
    LOOKUP_GROUP,
    "internity",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_local_with_sym())]
fn lookup_internity_hot(input: (LocalLexicon, Sym)) -> (LocalLexicon, usize) {
    let (interner, sym) = input;
    let len = black_box(interner.resolve(black_box(sym)).len());
    (interner, len)
}

#[metabench::benchmark(
    LOOKUP_INTERNITY_FROZEN,
    LOOKUP_GROUP,
    "internity-frozen",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_frozen())]
fn lookup_internity_frozen_hot<R: Reader>(input: (R, Sym)) -> (R, usize) {
    let (reader, sym) = input;
    let len = black_box(reader.resolve(black_box(sym)).len());
    (reader, len)
}

#[metabench::benchmark(LOOKUP_LASSO, LOOKUP_GROUP, "lasso", gungraun_config = gungraun_default_config())]
#[bench::run(populate_lasso_with_sym())]
fn lookup_lasso_hot(input: (RodeoFixed, lasso::Spur)) -> (RodeoFixed, usize) {
    let (rodeo, sym) = input;
    let len = black_box(rodeo.resolve(&black_box(sym)).len());
    (rodeo, len)
}

#[metabench::benchmark(
    LOOKUP_STRING_INTERNER,
    LOOKUP_GROUP,
    "string-interner",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_string_interner_with_sym())]
fn lookup_string_interner_hot(input: (SiFixed, string_interner::DefaultSymbol)) -> (SiFixed, usize) {
    let (interner, sym) = input;
    let len = black_box(
        interner
            .resolve(black_box(sym))
            .expect("symbol was produced by this interner")
            .len(),
    );
    (interner, len)
}

#[metabench::benchmark(
    LOOKUP_SYMBOL_TABLE,
    LOOKUP_GROUP,
    "symbol_table",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_symbol_table_with_sym())]
fn lookup_symbol_table_hot(input: (symbol_table::SymbolTable, symbol_table::Symbol)) -> (symbol_table::SymbolTable, usize) {
    let (table, sym) = input;
    let len = black_box(table.resolve(black_box(sym)).len());
    (table, len)
}

#[metabench::benchmark(LOOKUP_USTR, LOOKUP_GROUP, "ustr", gungraun_config = gungraun_default_config())]
#[bench::run(populate_ustr())]
fn lookup_ustr_hot(handle: ustr::Ustr) -> usize {
    black_box(handle.as_str().len())
}

#[metabench::benchmark(
    LOOKUP_STRING_CACHE,
    LOOKUP_GROUP,
    "string_cache",
    gungraun_config = gungraun_default_config()
)]
#[bench::run(populate_string_cache())]
fn lookup_string_cache_hot(state: StringCacheState) -> (StringCacheState, usize) {
    let len = black_box((state.key.as_ref() as &str).len());
    (state, len)
}

metabench::main!(
    criterion = criterion_benchmarks,
    groups = {
        SINGLE_THREADED {
            benchmarks = [
                INSERT_INTERNITY,
                INSERT_INTERNITY_THREADED,
                INSERT_LASSO,
                INSERT_STRING_INTERNER,
                INSERT_SYMBOL_TABLE,
                INSERT_USTR,
                INSERT_STRING_CACHE,
                REUSE_INTERNITY,
                REUSE_INTERNITY_THREADED,
                REUSE_LASSO,
                REUSE_STRING_INTERNER,
                REUSE_SYMBOL_TABLE,
                REUSE_USTR,
                REUSE_STRING_CACHE,
                LOOKUP_INTERNITY,
                LOOKUP_INTERNITY_FROZEN,
                LOOKUP_LASSO,
                LOOKUP_STRING_INTERNER,
                LOOKUP_SYMBOL_TABLE,
                LOOKUP_USTR,
                LOOKUP_STRING_CACHE,
            ],
            gungraun_max_parallel = 1,
        },
    },
);
