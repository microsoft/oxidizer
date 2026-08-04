// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Head-to-head wall-clock benchmarks against the main Rust interners.
//!
//! Three operations, each in a **single-threaded** flavor and **multi-threaded**
//! flavors at 1/2/4/8 threads:
//! * `insert`  / `insert-concurrent`  — intern fresh (never-seen) strings.
//! * `reuse`   / `reuse-concurrent`   — re-intern already-present strings (dedup hits).
//! * `lookup`  / `lookup-concurrent`  — resolve handle → &str (frozen readers).
//!
//! Single-threaded flavors compare the single-thread-capable crates (internity
//! `LocalLexicon`, `lasso::Rodeo`, `string-interner`, plus the globals for reuse/lookup);
//! the `*-concurrent` flavors compare the concurrent crates (internity
//! `ThreadedLexicon`, `lasso::ThreadedRodeo`, `symbol_table`, `ustr`, `string_cache`).

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::used_underscore_binding,
    reason = "benchmark harness code: index/stat casts and gungraun bindings are benign"
)]

use std::collections::BTreeSet;
use std::hint::black_box;
use std::sync::Barrier;
use std::thread;
use std::time::{Duration, Instant};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use internity::{LocalLexicon, Reader, ThreadedLexicon};

/// Concrete `string-interner` type (default StringBackend + hasher).
type Si = string_interner::StringInterner<string_interner::DefaultBackend>;

const DEFAULT_CORPUS_SIZE: usize = 6000;
const CORPUS_SIZE_ENV: &str = "INTERNITY_BENCH_CORPUS_SIZE";

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

fn bench_insert(c: &mut Criterion) {
    let corpus = corpus();
    let mut g = c.benchmark_group("internity_compare/insert");
    g.throughput(Throughput::Elements(corpus.len() as u64));

    // `iter_batched` builds the interner in the (untimed) setup and drops the
    // returned interner in an (untimed) teardown, so only the inserts are timed.
    g.bench_function("internity", |b| {
        b.iter_batched(
            LocalLexicon::new,
            |mut it| {
                for s in &corpus {
                    black_box(it.intern(s));
                }
                it
            },
            BatchSize::LargeInput,
        );
    });

    g.bench_function("internity-threaded", |b| {
        b.iter_batched(
            ThreadedLexicon::new,
            |it| {
                for s in &corpus {
                    black_box(it.intern(s));
                }
                it
            },
            BatchSize::LargeInput,
        );
    });

    g.bench_function("lasso", |b| {
        b.iter_batched(
            lasso::Rodeo::default,
            |mut r| {
                for s in &corpus {
                    black_box(r.get_or_intern(s));
                }
                r
            },
            BatchSize::LargeInput,
        );
    });

    g.bench_function("string-interner", |b| {
        b.iter_batched(
            Si::new,
            |mut si| {
                for s in &corpus {
                    black_box(si.get_or_intern(s));
                }
                si
            },
            BatchSize::LargeInput,
        );
    });

    g.bench_function("symbol_table", |b| {
        b.iter_batched(
            symbol_table::SymbolTable::new,
            |st| {
                for s in &corpus {
                    black_box(st.intern(s));
                }
                st
            },
            BatchSize::LargeInput,
        );
    });

    g.finish();
}

fn bench_reuse(c: &mut Criterion) {
    let corpus = corpus();
    let mut g = c.benchmark_group("internity_compare/reuse");
    g.throughput(Throughput::Elements(corpus.len() as u64));

    let mut it = LocalLexicon::new();
    for s in &corpus {
        it.intern(s);
    }
    g.bench_function("internity", |b| {
        b.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for s in &corpus {
                    results.push(black_box(it.intern(s)));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });

    let it_t = ThreadedLexicon::new();
    for s in &corpus {
        it_t.intern(s);
    }
    g.bench_function("internity-threaded", |b| {
        b.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for s in &corpus {
                    results.push(black_box(it_t.intern(s)));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });

    let mut r = lasso::Rodeo::default();
    for s in &corpus {
        r.get_or_intern(s);
    }
    g.bench_function("lasso", |b| {
        b.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for s in &corpus {
                    results.push(black_box(r.get_or_intern(s)));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });

    let mut si = Si::new();
    for s in &corpus {
        si.get_or_intern(s);
    }
    g.bench_function("string-interner", |b| {
        b.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for s in &corpus {
                    results.push(black_box(si.get_or_intern(s)));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });

    let st = symbol_table::SymbolTable::new();
    for s in &corpus {
        st.intern(s);
    }
    g.bench_function("symbol_table", |b| {
        b.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for s in &corpus {
                    results.push(black_box(st.intern(s)));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });

    for s in &corpus {
        ustr::ustr(s);
    }
    g.bench_function("ustr", |b| {
        b.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for s in &corpus {
                    results.push(black_box(ustr::ustr(s)));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });

    let _string_cache_atoms: Vec<string_cache::DefaultAtom> = corpus.iter().map(|s| string_cache::DefaultAtom::from(s.as_str())).collect();
    g.bench_function("string_cache", |b| {
        b.iter_batched(
            || Vec::with_capacity(corpus.len()),
            |mut results| {
                for s in &corpus {
                    results.push(black_box(string_cache::DefaultAtom::from(s.as_str())));
                }
                results
            },
            BatchSize::LargeInput,
        );
    });

    g.finish();
}

fn bench_lookup(c: &mut Criterion) {
    let corpus = corpus();
    // A shared random permutation so every interner resolves in the same random
    // order. Otherwise interners that hand out sequential symbols (lasso,
    // string-interner) get an unfair sequential-scan / prefetch advantage.
    let order = permutation(corpus.len());
    let mut g = c.benchmark_group("internity_compare/lookup");
    g.throughput(Throughput::Elements(corpus.len() as u64));

    let mut it = LocalLexicon::new();
    let it_syms: Vec<_> = corpus.iter().map(|s| it.intern(s)).collect();
    g.bench_function("internity", |b| {
        b.iter(|| {
            for &i in &order {
                black_box(it.resolve(it_syms[i]));
            }
        });
    });

    let mut frozen = LocalLexicon::new();
    let frozen_syms: Vec<_> = corpus.iter().map(|s| frozen.intern(s)).collect();
    let frozen = frozen.freeze();
    g.bench_function("internity-frozen", |b| {
        b.iter(|| {
            for &i in &order {
                black_box(frozen.resolve(frozen_syms[i]));
            }
        });
    });

    let mut r = lasso::Rodeo::default();
    let r_syms: Vec<_> = corpus.iter().map(|s| r.get_or_intern(s)).collect();
    g.bench_function("lasso", |b| {
        b.iter(|| {
            for &i in &order {
                black_box(r.resolve(&r_syms[i]));
            }
        });
    });

    let mut si = Si::new();
    let si_syms: Vec<_> = corpus.iter().map(|s| si.get_or_intern(s)).collect();
    g.bench_function("string-interner", |b| {
        b.iter(|| {
            for &i in &order {
                black_box(si.resolve(si_syms[i]).expect("symbol was produced by this interner"));
            }
        });
    });

    let st = symbol_table::SymbolTable::new();
    let st_syms: Vec<_> = corpus.iter().map(|s| st.intern(s)).collect();
    g.bench_function("symbol_table", |b| {
        b.iter(|| {
            for &i in &order {
                black_box(st.resolve(st_syms[i]));
            }
        });
    });

    let us: Vec<ustr::Ustr> = corpus.iter().map(|s| ustr::ustr(s)).collect();
    g.bench_function("ustr", |b| {
        b.iter(|| {
            for &i in &order {
                black_box(us[i].as_str());
            }
        });
    });

    let atoms: Vec<string_cache::DefaultAtom> = corpus.iter().map(|s| string_cache::DefaultAtom::from(s.as_str())).collect();
    g.bench_function("string_cache", |b| {
        b.iter(|| {
            for &i in &order {
                black_box(atoms[i].as_ref() as &str);
            }
        });
    });

    g.finish();
}

fn bench_insert_concurrent(c: &mut Criterion) {
    let corpus = corpus();
    let mut g = c.benchmark_group("internity_compare/insert-concurrent");

    // Global interners (ustr, string_cache) are intentionally excluded here: they
    // cannot be reset, so a fresh concurrent *fill* is not expressible for them and
    // would not be equivalent to the fresh-interner crates below.
    for threads in [1usize, 2, 4, 8] {
        // Each string is assigned to exactly one worker, so every operation is a
        // genuine insertion into the fresh interner.
        g.throughput(Throughput::Elements(corpus.len() as u64));

        g.bench_with_input(BenchmarkId::new("internity", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                concurrent_fill(iters, t, &corpus, ThreadedLexicon::new, |it, s| {
                    black_box(it.intern(s));
                })
            });
        });

        g.bench_with_input(BenchmarkId::new("lasso-threaded", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                concurrent_fill(iters, t, &corpus, lasso::ThreadedRodeo::default, |r, s| {
                    black_box(r.get_or_intern(s));
                })
            });
        });

        g.bench_with_input(BenchmarkId::new("symbol_table", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                concurrent_fill(iters, t, &corpus, symbol_table::SymbolTable::new, |st, s| {
                    black_box(st.intern(s));
                })
            });
        });
    }

    g.finish();
}

/// Times **only** the concurrent-intern work. For each of `iters` rounds it builds
/// a fresh interner (not timed), spawns `t` scoped threads (not timed), and partitions
/// the corpus among workers so each string is inserted exactly once.
///
/// The coordinator (this thread) owns a single wall-clock interval: it joins a
/// start barrier so every worker is released simultaneously, records the start
/// instant, then joins an end barrier that trips only once **every** worker has
/// finished its intern loop, and records the elapsed span. The round's duration is
/// therefore the elapsed time from parallel release through the last worker's
/// completion — not the maximum individual worker time, which would understate
/// wall-clock when workers are staggered. Construction, thread spawn/join, and
/// drop stay outside the timed region.
fn concurrent_fill<T, C, I>(iters: u64, t: usize, corpus: &[String], construct: C, intern: I) -> Duration
where
    T: Sync,
    C: Fn() -> T,
    I: Fn(&T, &str) + Sync,
{
    let mut total = Duration::ZERO;
    for _ in 0..iters {
        let it = construct(); // untimed
        // `t` workers plus this coordinator thread participate in both barriers.
        let start_barrier = Barrier::new(t + 1);
        let end_barrier = Barrier::new(t + 1);
        let intern = &intern;
        let it = &it;
        let round = thread::scope(|scope| {
            for worker in 0..t {
                let start = corpus.len() * worker / t;
                let end = corpus.len() * (worker + 1) / t;
                let chunk = &corpus[start..end];
                let start_barrier = &start_barrier;
                let end_barrier = &end_barrier;
                scope.spawn(move || {
                    start_barrier.wait(); // released together with the coordinator
                    for s in chunk {
                        intern(it, s);
                    }
                    end_barrier.wait(); // signal completion to the coordinator
                });
            }
            start_barrier.wait();
            let started = Instant::now();
            end_barrier.wait();
            started.elapsed()
        });
        total += round;
        // `it` is dropped here, outside the timed region.
    }
    total
}

/// Times **only** the parallel work. For each of `iters` rounds it spawns `t`
/// scoped threads (not timed); each worker runs its (untimed) `setup`, then the
/// coordinator owns a single wall-clock interval spanning parallel release through
/// the last worker's completion — see [`concurrent_fill`] for the barrier protocol.
/// Unlike it, the shared state is built once by the caller (used for read-mostly
/// workloads — reuse/lookup — where the op doesn't mutate structure).
fn timed_parallel_with_setup<S>(iters: u64, t: usize, setup: impl Fn() -> S + Sync, work: impl Fn(&mut S) + Sync) -> Duration {
    let mut total = Duration::ZERO;
    let setup = &setup;
    let work = &work;
    for _ in 0..iters {
        let start_barrier = Barrier::new(t + 1);
        let end_barrier = Barrier::new(t + 1);
        let round = thread::scope(|scope| {
            for _ in 0..t {
                let start_barrier = &start_barrier;
                let end_barrier = &end_barrier;
                scope.spawn(move || {
                    let mut state = setup(); // untimed
                    start_barrier.wait(); // released together with the coordinator
                    work(&mut state);
                    end_barrier.wait(); // signal completion before dropping state
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

fn timed_parallel(iters: u64, t: usize, work: impl Fn() + Sync) -> Duration {
    timed_parallel_with_setup(iters, t, || false, |_| work())
}

fn timed_parallel_collect<R>(iters: u64, t: usize, corpus: &[String], op: impl Fn(&str) -> R + Sync) -> Duration {
    timed_parallel_with_setup(
        iters,
        t,
        || Vec::with_capacity(corpus.len()),
        |results| {
            for s in corpus {
                results.push(black_box(op(s)));
            }
        },
    )
}

fn bench_reuse_concurrent(c: &mut Criterion) {
    let corpus = corpus();
    let mut g = c.benchmark_group("internity_compare/reuse-concurrent");

    // Pre-fill once; every timed thread re-interns the corpus (all dedup hits),
    // which never mutates the interner's structure, so the shared state is reused
    // across rounds. Globals (ustr, string_cache) are pre-seeded once here too.
    let it = ThreadedLexicon::new();
    let rodeo = lasso::ThreadedRodeo::default();
    let st = symbol_table::SymbolTable::new();
    for s in &corpus {
        it.intern(s);
        rodeo.get_or_intern(s);
        st.intern(s);
        ustr::ustr(s);
    }
    let _string_cache_atoms: Vec<string_cache::DefaultAtom> = corpus.iter().map(|s| string_cache::DefaultAtom::from(s.as_str())).collect();

    for threads in [1usize, 2, 4, 8] {
        g.throughput(Throughput::Elements((threads * corpus.len()) as u64));

        g.bench_with_input(BenchmarkId::new("internity", threads), &threads, |b, &t| {
            b.iter_custom(|iters| timed_parallel_collect(iters, t, &corpus, |s| it.intern(s)));
        });
        g.bench_with_input(BenchmarkId::new("lasso-threaded", threads), &threads, |b, &t| {
            b.iter_custom(|iters| timed_parallel_collect(iters, t, &corpus, |s| rodeo.get_or_intern(s)));
        });
        g.bench_with_input(BenchmarkId::new("symbol_table", threads), &threads, |b, &t| {
            b.iter_custom(|iters| timed_parallel_collect(iters, t, &corpus, |s| st.intern(s)));
        });
        g.bench_with_input(BenchmarkId::new("ustr", threads), &threads, |b, &t| {
            b.iter_custom(|iters| timed_parallel_collect(iters, t, &corpus, ustr::ustr));
        });
        g.bench_with_input(BenchmarkId::new("string_cache", threads), &threads, |b, &t| {
            b.iter_custom(|iters| timed_parallel_collect(iters, t, &corpus, |s| string_cache::DefaultAtom::from(s)));
        });
    }

    g.finish();
}

fn bench_lookup_concurrent(c: &mut Criterion) {
    let corpus = corpus();
    let order = permutation(corpus.len());
    let mut g = c.benchmark_group("internity_compare/lookup-concurrent");

    // Build each frozen reader / handle set once; timed threads resolve in the
    // same shared random order.
    let it = ThreadedLexicon::new();
    let it_syms: Vec<_> = corpus.iter().map(|s| it.intern(s)).collect();
    let reader = it.freeze();

    let rodeo = lasso::ThreadedRodeo::default();
    let r_syms: Vec<_> = corpus.iter().map(|s| rodeo.get_or_intern(s)).collect();
    let resolver = rodeo.into_resolver();

    let st = symbol_table::SymbolTable::new();
    let st_syms: Vec<_> = corpus.iter().map(|s| st.intern(s)).collect();

    let us: Vec<ustr::Ustr> = corpus.iter().map(|s| ustr::ustr(s)).collect();
    let atoms: Vec<string_cache::DefaultAtom> = corpus.iter().map(|s| string_cache::DefaultAtom::from(s.as_str())).collect();

    for threads in [1usize, 2, 4, 8] {
        g.throughput(Throughput::Elements((threads * corpus.len()) as u64));

        g.bench_with_input(BenchmarkId::new("internity", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                timed_parallel(iters, t, || {
                    for &i in &order {
                        black_box(reader.resolve(it_syms[i]));
                    }
                })
            });
        });
        g.bench_with_input(BenchmarkId::new("lasso-resolver", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                timed_parallel(iters, t, || {
                    for &i in &order {
                        black_box(resolver.resolve(&r_syms[i]));
                    }
                })
            });
        });
        g.bench_with_input(BenchmarkId::new("symbol_table", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                timed_parallel(iters, t, || {
                    for &i in &order {
                        black_box(st.resolve(st_syms[i]));
                    }
                })
            });
        });
        g.bench_with_input(BenchmarkId::new("ustr", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                timed_parallel(iters, t, || {
                    for &i in &order {
                        black_box(us[i].as_str());
                    }
                })
            });
        });
        g.bench_with_input(BenchmarkId::new("string_cache", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                timed_parallel(iters, t, || {
                    for &i in &order {
                        black_box(atoms[i].as_ref() as &str);
                    }
                })
            });
        });
    }

    g.finish();
}

criterion_group!(
    benches,
    bench_insert,
    bench_insert_concurrent,
    bench_reuse,
    bench_reuse_concurrent,
    bench_lookup,
    bench_lookup_concurrent,
);
criterion_main!(benches);
