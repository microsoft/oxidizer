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
//! `Lexicon`, `lasso::Rodeo`, `string-interner`, plus the globals for reuse/lookup);
//! the `*-concurrent` flavors compare the concurrent crates (internity
//! `ThreadedLexicon`, `lasso::ThreadedRodeo`, `symbol_table`, `ustr`, `string_cache`).

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::used_underscore_binding,
    reason = "benchmark harness code: index/stat casts and gungraun bindings are benign"
)]

use std::hint::black_box;
use std::sync::Barrier;
use std::thread;
use std::time::{Duration, Instant};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use internity::{Lexicon, Reader, ThreadedLexicon};

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
    let mut out = Vec::new();
    while out.len() < corpus_size {
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
    let mut g = c.benchmark_group("insert");
    g.throughput(Throughput::Elements(corpus.len() as u64));

    // `iter_batched` builds the interner in the (untimed) setup and drops the
    // returned interner in an (untimed) teardown, so only the inserts are timed.
    g.bench_function("internity", |b| {
        b.iter_batched(
            Lexicon::new,
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
    let mut g = c.benchmark_group("reuse");
    g.throughput(Throughput::Elements(corpus.len() as u64));

    let mut it = Lexicon::new();
    for s in &corpus {
        it.intern(s);
    }
    g.bench_function("internity", |b| {
        b.iter(|| {
            for s in &corpus {
                black_box(it.intern(s));
            }
        });
    });

    let it_t = ThreadedLexicon::new();
    for s in &corpus {
        it_t.intern(s);
    }
    g.bench_function("internity-threaded", |b| {
        b.iter(|| {
            for s in &corpus {
                black_box(it_t.intern(s));
            }
        });
    });

    let mut r = lasso::Rodeo::default();
    for s in &corpus {
        r.get_or_intern(s);
    }
    g.bench_function("lasso", |b| {
        b.iter(|| {
            for s in &corpus {
                black_box(r.get_or_intern(s));
            }
        });
    });

    let mut si = Si::new();
    for s in &corpus {
        si.get_or_intern(s);
    }
    g.bench_function("string-interner", |b| {
        b.iter(|| {
            for s in &corpus {
                black_box(si.get_or_intern(s));
            }
        });
    });

    let st = symbol_table::SymbolTable::new();
    for s in &corpus {
        st.intern(s);
    }
    g.bench_function("symbol_table", |b| {
        b.iter(|| {
            for s in &corpus {
                black_box(st.intern(s));
            }
        });
    });

    for s in &corpus {
        ustr::ustr(s);
    }
    g.bench_function("ustr", |b| {
        b.iter(|| {
            for s in &corpus {
                black_box(ustr::ustr(s));
            }
        });
    });

    for s in &corpus {
        let _ = string_cache::DefaultAtom::from(s.as_str());
    }
    g.bench_function("string_cache", |b| {
        b.iter(|| {
            for s in &corpus {
                black_box(string_cache::DefaultAtom::from(s.as_str()));
            }
        });
    });

    g.finish();
}

fn bench_lookup(c: &mut Criterion) {
    let corpus = corpus();
    // A shared random permutation so every interner resolves in the same random
    // order. Otherwise interners that hand out sequential symbols (lasso,
    // string-interner) get an unfair sequential-scan / prefetch advantage.
    let order = permutation(corpus.len());
    let mut g = c.benchmark_group("lookup");
    g.throughput(Throughput::Elements(corpus.len() as u64));

    let mut it = Lexicon::new();
    let it_syms: Vec<_> = corpus.iter().map(|s| it.intern(s)).collect();
    g.bench_function("internity", |b| {
        b.iter(|| {
            for &i in &order {
                black_box(it.resolve(it_syms[i]));
            }
        });
    });

    let mut frozen = Lexicon::new();
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
    let mut g = c.benchmark_group("insert-concurrent");

    // Global interners (ustr, string_cache) are intentionally excluded here: they
    // cannot be reset, so a fresh concurrent *fill* is not expressible for them and
    // would not be equivalent to the fresh-interner crates below.
    for threads in [1usize, 2, 4, 8] {
        // Total intern operations = threads * corpus (each thread interns the corpus).
        g.throughput(Throughput::Elements((threads * corpus.len()) as u64));

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
/// a fresh interner (untimed), spawns `t` scoped threads (untimed), and uses a
/// barrier so the measured region is exactly the parallel `intern` calls — no
/// construction, thread spawn/join, or drop is included.
fn concurrent_fill<T, C, I>(iters: u64, t: usize, corpus: &[String], construct: C, intern: I) -> Duration
where
    T: Sync,
    C: Fn() -> T,
    I: Fn(&T, &str) + Sync,
{
    let mut total = Duration::ZERO;
    for _ in 0..iters {
        let it = construct(); // untimed
        let barrier = Barrier::new(t + 1);
        let intern = &intern;
        thread::scope(|scope| {
            for _ in 0..t {
                scope.spawn(|| {
                    barrier.wait(); // all threads start together
                    for s in corpus {
                        intern(&it, s);
                    }
                    barrier.wait(); // signal completion
                });
            }
            barrier.wait(); // release the workers, then start timing
            let start = Instant::now();
            barrier.wait(); // wait for all workers to finish the intern work
            total += start.elapsed();
        });
        // `it` is dropped here, outside the timed region.
    }
    total
}

/// Times **only** the parallel work. For each of `iters` rounds it spawns `t`
/// scoped threads (untimed) and uses a barrier so the measured region is exactly
/// the parallel `work` calls — no thread spawn/join is included. Unlike
/// [`concurrent_fill`], the shared state is built once by the caller (used for
/// read-mostly workloads — reuse/lookup — where the op doesn't mutate structure).
fn timed_parallel(iters: u64, t: usize, work: impl Fn() + Sync) -> Duration {
    let mut total = Duration::ZERO;
    let work = &work;
    for _ in 0..iters {
        let barrier = Barrier::new(t + 1);
        thread::scope(|scope| {
            for _ in 0..t {
                scope.spawn(|| {
                    barrier.wait(); // all threads start together
                    work();
                    barrier.wait(); // signal completion
                });
            }
            barrier.wait(); // release the workers, then start timing
            let start = Instant::now();
            barrier.wait(); // wait for all workers to finish
            total += start.elapsed();
        });
    }
    total
}

fn bench_reuse_concurrent(c: &mut Criterion) {
    let corpus = corpus();
    let mut g = c.benchmark_group("reuse-concurrent");

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
        let _ = string_cache::DefaultAtom::from(s.as_str());
    }

    for threads in [1usize, 2, 4, 8] {
        g.throughput(Throughput::Elements((threads * corpus.len()) as u64));

        g.bench_with_input(BenchmarkId::new("internity", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                timed_parallel(iters, t, || {
                    for s in &corpus {
                        black_box(it.intern(s));
                    }
                })
            });
        });
        g.bench_with_input(BenchmarkId::new("lasso-threaded", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                timed_parallel(iters, t, || {
                    for s in &corpus {
                        black_box(rodeo.get_or_intern(s));
                    }
                })
            });
        });
        g.bench_with_input(BenchmarkId::new("symbol_table", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                timed_parallel(iters, t, || {
                    for s in &corpus {
                        black_box(st.intern(s));
                    }
                })
            });
        });
        g.bench_with_input(BenchmarkId::new("ustr", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                timed_parallel(iters, t, || {
                    for s in &corpus {
                        black_box(ustr::ustr(s));
                    }
                })
            });
        });
        g.bench_with_input(BenchmarkId::new("string_cache", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                timed_parallel(iters, t, || {
                    for s in &corpus {
                        black_box(string_cache::DefaultAtom::from(s.as_str()));
                    }
                })
            });
        });
    }

    g.finish();
}

fn bench_lookup_concurrent(c: &mut Criterion) {
    let corpus = corpus();
    let order = permutation(corpus.len());
    let mut g = c.benchmark_group("lookup-concurrent");

    // Build each frozen reader / handle set once; timed threads resolve in the
    // same shared random order.
    let it = ThreadedLexicon::new();
    let it_syms: Vec<_> = corpus.iter().map(|s| it.intern(s)).collect();
    let reader = it.freeze();

    let rodeo = lasso::ThreadedRodeo::default();
    let r_syms: Vec<_> = corpus.iter().map(|s| rodeo.get_or_intern(s)).collect();

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
        g.bench_with_input(BenchmarkId::new("lasso-threaded", threads), &threads, |b, &t| {
            b.iter_custom(|iters| {
                timed_parallel(iters, t, || {
                    for &i in &order {
                        black_box(rodeo.resolve(&r_syms[i]));
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
