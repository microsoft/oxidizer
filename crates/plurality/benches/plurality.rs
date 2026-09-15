// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![allow(missing_docs, reason = "benchmark code")]
#![allow(
    clippy::allow_attributes,
    clippy::assertions_on_result_states,
    clippy::borrow_as_ptr,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::clone_on_ref_ptr,
    clippy::collection_is_never_read,
    clippy::doc_markdown,
    clippy::items_after_statements,
    clippy::many_single_char_names,
    clippy::missing_panics_doc,
    clippy::unwrap_used,
    missing_debug_implementations,
    reason = "benchmark code"
)]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "Triggered by Gungraun macro expansion (emitted for every platform; Gungraun itself only runs on Linux)."
)]

//! Consolidated Criterion and Gungraun benchmark suite for plurality.

use std::hint::black_box;

use criterion::Criterion;
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};

#[path = "plurality_ops_common/mod.rs"]
mod ops;

#[cfg(target_os = "linux")]
#[path = "pool_comparison/shared.rs"]
mod pool_shared;

const N: u64 = 1000;

#[cfg(target_os = "linux")]
const CHURN_COUNT: u64 = 10_000;

macro_rules! alloc_benchmark {
    ($const_name:ident, $fn_name:ident, $label:literal) => {
        #[metabench::benchmark($const_name, "alloc", $label)]
        #[bench::run(ops::setup_pool(ops::CAP))]
        fn $fn_name(pool: plurality::Pool<ops::Obj>) -> plurality::Pool<ops::Obj> {
            ops::$fn_name(black_box(&pool), 0);
            pool
        }
    };
    ($const_name:ident, $fn_name:ident, $label:literal, $setup:expr, $ty:ty) => {
        #[metabench::benchmark($const_name, "alloc", $label)]
        #[bench::run($setup)]
        fn $fn_name(pool: $ty) -> $ty {
            ops::$fn_name(black_box(&pool), 0);
            pool
        }
    };
}

alloc_benchmark!(ALLOC_BOX_VAL, box_val, "box_val");
alloc_benchmark!(ALLOC_BOX_WITH, box_with, "box_with");
alloc_benchmark!(ALLOC_BOX_UNINIT, box_uninit, "box_uninit");
alloc_benchmark!(ALLOC_BOX_UNSIZE, box_unsize, "box_unsize");
alloc_benchmark!(ALLOC_ARC_UNSIZE, arc_unsize, "arc_unsize");
alloc_benchmark!(ALLOC_ARC_VAL, arc_val, "arc_val");
alloc_benchmark!(ALLOC_ARC_WITH, arc_with, "arc_with");
alloc_benchmark!(ALLOC_ARC_UNINIT, arc_uninit, "arc_uninit");
alloc_benchmark!(ALLOC_ALLOC_VAL, alloc_val, "alloc_val");
alloc_benchmark!(ALLOC_ALLOC_WITH, alloc_with, "alloc_with");
alloc_benchmark!(ALLOC_ALLOC_UNINIT, alloc_uninit, "alloc_uninit");
alloc_benchmark!(ALLOC_RC_VAL, rc_val, "rc_val");
alloc_benchmark!(ALLOC_RC_WITH, rc_with, "rc_with");
alloc_benchmark!(ALLOC_RC_UNINIT, rc_uninit, "rc_uninit");
alloc_benchmark!(
    ALLOC_MULTI_BOX_VAL,
    multi_box_val,
    "multi_box_val",
    ops::setup_multi_pool(ops::CAP),
    plurality::MultiPool
);
alloc_benchmark!(
    ALLOC_MULTI_BOX_VAL_SPREAD,
    multi_box_val_spread,
    "multi_box_val_spread",
    ops::setup_multi_pool_spread(ops::CAP),
    plurality::MultiPool
);
alloc_benchmark!(
    ALLOC_MULTI_BOX_VAL_MISS,
    multi_box_val_miss,
    "multi_box_val_miss",
    ops::setup_multi_pool_miss(ops::CAP),
    plurality::MultiPool
);

#[metabench::benchmark(CLONE_ARC_CLONE, "clone", "arc_clone")]
#[bench::run(ops::setup_arc(ops::CAP))]
fn arc_clone(setup: (plurality::Pool<ops::Obj>, plurality::Arc<ops::Obj>)) -> (plurality::Pool<ops::Obj>, plurality::Arc<ops::Obj>) {
    ops::arc_clone(black_box(&setup.1));
    setup
}

#[metabench::benchmark(CLONE_RC_CLONE, "clone", "rc_clone")]
#[bench::run(ops::setup_rc(ops::CAP))]
fn rc_clone(setup: (plurality::Pool<ops::Obj>, plurality::Rc<ops::Obj>)) -> (plurality::Pool<ops::Obj>, plurality::Rc<ops::Obj>) {
    ops::rc_clone(black_box(&setup.1));
    setup
}

#[metabench::benchmark(DYN_BOX_PLURALITY_BOX, "dyn_box", "plurality_box")]
#[bench::run(ops::setup_plurality(ops::CAP))]
fn plurality_box(pool: plurality::Pool<ops::Obj>) -> plurality::Pool<ops::Obj> {
    ops::plurality_box(black_box(&pool), 0);
    pool
}

#[metabench::benchmark(DYN_BOX_PLURALITY_MULTI_BOX, "dyn_box", "plurality_multi_box")]
#[bench::run(ops::setup_plurality_multi(ops::CAP))]
fn plurality_multi_box(pool: plurality::MultiPool) -> plurality::MultiPool {
    ops::plurality_multi_box(black_box(&pool), 0);
    pool
}

#[metabench::benchmark(DYN_BOX_INFINITY_PINNED, "dyn_box", "infinity_pinned")]
#[bench::run(ops::setup_infinity_pinned(ops::CAP))]
fn infinity_pinned(pool: infinity_pool::PinnedPool<ops::Obj>) -> infinity_pool::PinnedPool<ops::Obj> {
    ops::infinity_pinned(black_box(&pool), 0);
    pool
}

#[metabench::benchmark(DYN_BOX_INFINITY_LOCAL_PINNED, "dyn_box", "infinity_local_pinned")]
#[bench::run(ops::setup_infinity_local_pinned(ops::CAP))]
fn infinity_local_pinned(pool: infinity_pool::LocalPinnedPool<ops::Obj>) -> infinity_pool::LocalPinnedPool<ops::Obj> {
    ops::infinity_local_pinned(black_box(&pool), 0);
    pool
}

#[metabench::benchmark(DYN_BOX_INFINITY_BLIND, "dyn_box", "infinity_blind")]
#[bench::run(ops::setup_infinity_blind(ops::CAP))]
fn infinity_blind(pool: infinity_pool::BlindPool) -> infinity_pool::BlindPool {
    ops::infinity_blind(black_box(&pool), 0);
    pool
}

#[metabench::benchmark(DYN_BOX_INFINITY_LOCAL_BLIND, "dyn_box", "infinity_local_blind")]
#[bench::run(ops::setup_infinity_local_blind(ops::CAP))]
fn infinity_local_blind(pool: infinity_pool::LocalBlindPool) -> infinity_pool::LocalBlindPool {
    ops::infinity_local_blind(black_box(&pool), 0);
    pool
}

#[metabench::benchmark(DYN_BOX_STD_BOX, "dyn_box", "std_box")]
#[bench::run(ops::setup_std_box(ops::CAP))]
fn std_box(_setup: ()) {
    ops::std_box(0);
}

#[cfg(target_os = "linux")]
macro_rules! pool_benchmark {
    ($const_name:ident, $wrapper:ident, $op:path, $label:literal, $setup:expr, $ty:ty) => {
        #[metabench::benchmark($const_name, "pool_comparison/churn", $label)]
        #[bench::run($setup)]
        fn $wrapper(pool: $ty) -> $ty {
            for i in 0..CHURN_COUNT {
                $op(black_box(&pool), i);
            }
            pool
        }
    };
    (mut $const_name:ident, $wrapper:ident, $op:path, $label:literal, $setup:expr, $ty:ty) => {
        #[metabench::benchmark($const_name, "pool_comparison/churn", $label)]
        #[bench::run($setup)]
        fn $wrapper(mut pool: $ty) -> $ty {
            for i in 0..CHURN_COUNT {
                $op(black_box(&mut pool), i);
            }
            pool
        }
    };
}

#[cfg(target_os = "linux")]
pool_benchmark!(
    POOL_COMPARISON_CHURN_PLURALITY_BOX,
    pool_plurality_box,
    pool_shared::plurality_box,
    "plurality_box",
    pool_shared::setup_plurality(pool_shared::CAP),
    plurality::Pool<pool_shared::Obj>
);
#[cfg(target_os = "linux")]
pool_benchmark!(
    POOL_COMPARISON_CHURN_PLURALITY_ALLOC,
    pool_plurality_alloc,
    pool_shared::plurality_alloc,
    "plurality_alloc",
    pool_shared::setup_plurality(pool_shared::CAP),
    plurality::Pool<pool_shared::Obj>
);
#[cfg(target_os = "linux")]
pool_benchmark!(
    mut POOL_COMPARISON_CHURN_SLAB_INSERT_REMOVE,
    pool_slab_insert_remove,
    pool_shared::slab_insert_remove,
    "slab_insert_remove",
    pool_shared::setup_slab(pool_shared::CAP),
    slab::Slab<pool_shared::Obj>
);
#[cfg(target_os = "linux")]
pool_benchmark!(
    POOL_COMPARISON_CHURN_SHARDED_SLAB_INSERT_REMOVE,
    pool_sharded_slab_insert_remove,
    pool_shared::sharded_slab_insert_remove,
    "sharded_slab_insert_remove",
    pool_shared::setup_sharded_slab(pool_shared::CAP),
    sharded_slab::Slab<pool_shared::Obj>
);
#[cfg(target_os = "linux")]
pool_benchmark!(
    mut POOL_COMPARISON_CHURN_SLOTMAP_INSERT_REMOVE,
    pool_slotmap_insert_remove,
    pool_shared::slotmap_insert_remove,
    "slotmap_insert_remove",
    pool_shared::setup_slotmap(pool_shared::CAP),
    slotmap::SlotMap<slotmap::DefaultKey, pool_shared::Obj>
);
#[cfg(target_os = "linux")]
pool_benchmark!(
    POOL_COMPARISON_CHURN_OBJECT_POOL_PULL,
    pool_object_pool_pull,
    pool_shared::object_pool_pull,
    "object_pool_pull",
    pool_shared::setup_object_pool(pool_shared::CAP),
    object_pool::Pool<pool_shared::Obj>
);
#[cfg(target_os = "linux")]
pool_benchmark!(
    POOL_COMPARISON_CHURN_OPOOL_GET,
    pool_opool_get,
    pool_shared::opool_get,
    "opool_get",
    pool_shared::setup_opool(pool_shared::CAP),
    opool::Pool<pool_shared::ObjAllocator, pool_shared::Obj>
);
#[cfg(target_os = "linux")]
pool_benchmark!(
    POOL_COMPARISON_CHURN_DEADPOOL_GET,
    pool_deadpool_get,
    pool_shared::deadpool_get,
    "deadpool_get",
    pool_shared::setup_deadpool(pool_shared::CAP),
    deadpool::unmanaged::Pool<pool_shared::Obj>
);
#[cfg(target_os = "linux")]
pool_benchmark!(
    POOL_COMPARISON_CHURN_INFINITY_PINNED,
    pool_infinity_pinned,
    pool_shared::infinity_pinned,
    "infinity_pinned",
    pool_shared::setup_infinity_pinned(pool_shared::CAP),
    infinity_pool::PinnedPool<pool_shared::Obj>
);
#[cfg(target_os = "linux")]
pool_benchmark!(
    mut POOL_COMPARISON_CHURN_INFINITY_RAW,
    pool_infinity_raw,
    pool_shared::infinity_raw,
    "infinity_raw",
    pool_shared::setup_infinity_raw(pool_shared::CAP),
    infinity_pool::RawPinnedPool<pool_shared::Obj>
);

mod graph_churn {
    use core::ops::Deref;
    use std::alloc::{GlobalAlloc, Layout};
    use std::ptr::NonNull;
    use std::time::Duration;

    use plurality::Pool;

    pub(super) const DEG: usize = 8;
    pub(super) const CAP: usize = 250_000;
    pub(super) const TARGET_INSERTS: usize = 1_000_000;
    const ITERS: usize = 5;

    #[derive(Clone)]
    pub(super) struct Node {
        id: u64,
        payload: [u64; 6],
        degree: u32,
        neighbors: [u32; DEG],
    }

    #[derive(Clone)]
    pub(super) enum Op {
        Insert {
            slot: u32,
            id: u64,
            degree: u8,
            neighbors: [u32; DEG],
        },
        Remove {
            slot: u32,
        },
    }

    pub(super) struct Scenario {
        ops: Box<[Op]>,
        checksum: u64,
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    struct MiBox(NonNull<Node>);

    impl Deref for MiBox {
        type Target = Node;

        fn deref(&self) -> &Self::Target {
            // SAFETY: `self.0` points to a live, initialized `Node` owned by this handle.
            unsafe { self.0.as_ref() }
        }
    }

    impl Drop for MiBox {
        fn drop(&mut self) {
            let layout = Layout::new::<Node>();
            // SAFETY: this handle allocated `self.0` with MiMalloc using `layout` and drops it once.
            unsafe {
                GlobalAlloc::dealloc(&mimalloc::MiMalloc, self.0.cast().as_ptr(), layout);
            }
        }
    }

    #[allow(clippy::cast_ptr_alignment, reason = "MiMalloc honors the requested layout alignment.")]
    fn mibox(node: Node) -> MiBox {
        let layout = Layout::new::<Node>();
        // SAFETY: MiMalloc allocates a block for exactly one `Node` with `layout`'s size and alignment.
        let ptr = unsafe { GlobalAlloc::alloc(&mimalloc::MiMalloc, layout).cast::<Node>() };
        let ptr = NonNull::new(ptr).expect("mimalloc returned null for Node allocation");
        // SAFETY: `ptr` is valid for writes of one `Node` and is not yet initialized.
        unsafe {
            ptr.as_ptr().write(node);
        }
        MiBox(ptr)
    }

    fn generate_ops() -> Vec<Op> {
        let mut rng = Rng(0x0C0F_FEE1_2345_6789);
        let mut free: Vec<u32> = (0..CAP as u32).rev().collect();
        let mut live: Vec<u32> = Vec::with_capacity(CAP);
        let mut ops: Vec<Op> = Vec::with_capacity(TARGET_INSERTS * 2);
        let mut inserts = 0usize;
        let mut next_id = 0u64;

        while inserts < TARGET_INSERTS {
            let do_insert = if live.is_empty() {
                true
            } else if free.is_empty() {
                false
            } else {
                rng.below(100) < 60
            };

            if do_insert {
                let slot = free.pop().expect("free slot available");
                let mut neighbors = [0u32; DEG];
                let mut degree = 0u8;
                if !live.is_empty() {
                    let k = rng.below(DEG as u64 + 1) as usize;
                    for n in neighbors.iter_mut().take(k) {
                        *n = live[rng.below(live.len() as u64) as usize];
                        degree += 1;
                    }
                }
                ops.push(Op::Insert {
                    slot,
                    id: next_id,
                    degree,
                    neighbors,
                });
                next_id += 1;
                live.push(slot);
                inserts += 1;
            } else {
                let i = rng.below(live.len() as u64) as usize;
                let slot = live.swap_remove(i);
                free.push(slot);
                ops.push(Op::Remove { slot });
            }
        }

        ops
    }

    fn replay<H, F>(ops: &[Op], mut make: F) -> (Duration, u64)
    where
        H: Deref<Target = Node>,
        F: FnMut(Node) -> H,
    {
        let mut slab: Vec<Option<H>> = Vec::with_capacity(CAP);
        slab.resize_with(CAP, || None);
        let mut checksum = 0u64;

        let start = std::time::Instant::now();
        for op in ops {
            match *op {
                Op::Insert {
                    slot,
                    id,
                    degree,
                    neighbors,
                } => {
                    let mut acc = id;
                    for &nb in &neighbors[..degree as usize] {
                        if let Some(node) = &slab[nb as usize] {
                            acc ^= node.id.wrapping_mul(0x0000_0100_0000_01B3);
                        }
                    }
                    let node = Node {
                        id,
                        payload: [acc, id, !id, acc ^ id, 0, 0],
                        degree: u32::from(degree),
                        neighbors,
                    };
                    checksum = checksum.wrapping_add(acc);
                    slab[slot as usize] = Some(make(node));
                }
                Op::Remove { slot } => {
                    if let Some(node) = slab[slot as usize].take() {
                        checksum ^= node.id.rotate_left(node.degree & 63) ^ node.payload[0] ^ u64::from(node.neighbors[0]);
                    }
                }
            }
        }
        for entry in &mut slab {
            if let Some(node) = entry.take() {
                checksum ^= node.id ^ node.payload[1];
            }
        }
        (start.elapsed(), checksum)
    }

    pub(super) fn prepare() -> Scenario {
        let ops = generate_ops().into_boxed_slice();
        let scenario = Scenario { ops, checksum: 0 };
        let (_, std_checksum) = bench_mimalloc(&scenario);
        let (_, pool_checksum) = bench_pool(&scenario);
        assert_eq!(
            std_checksum, pool_checksum,
            "shared graph-churn workload must produce identical checksums"
        );
        Scenario {
            ops: scenario.ops,
            checksum: std_checksum,
        }
    }

    pub(super) fn expected_checksum(scenario: &Scenario) -> u64 {
        scenario.checksum
    }

    pub(super) fn bench_pool(scenario: &Scenario) -> (Duration, u64) {
        let mut best = Duration::MAX;
        let mut checksum = 0;
        for _ in 0..ITERS {
            let pool = Pool::<Node>::builder().chunk_size(8192).build();
            let (elapsed, sum) = replay(&scenario.ops, |node| pool.alloc_box(node));
            best = best.min(elapsed);
            checksum = sum;
        }
        (best, checksum)
    }

    pub(super) fn bench_mimalloc(scenario: &Scenario) -> (Duration, u64) {
        let mut best = Duration::MAX;
        let mut checksum = 0;
        for _ in 0..ITERS {
            let (elapsed, sum) = replay(&scenario.ops, mibox);
            best = best.min(elapsed);
            checksum = sum;
        }
        (best, checksum)
    }
}

#[expect(clippy::too_many_lines, reason = "preserving the original benchmark group layout")]
fn criterion_benchmarks(criterion: &mut Criterion) {
    let pool = ops::setup_pool(ops::CAP);
    let multi = ops::setup_multi_pool(ops::CAP);
    let multi_spread = ops::setup_multi_pool_spread(ops::CAP);

    let mut alloc = criterion.benchmark_group(ALLOC_BOX_VAL.group_name());
    macro_rules! alloc_loop {
        ($const_name:ident, $op:ident, $pool:expr) => {
            alloc.bench_function($const_name.benchmark_name(), |bencher| {
                bencher.iter(|| {
                    for i in 0..N {
                        ops::$op(black_box($pool), i);
                    }
                });
            });
        };
    }
    alloc_loop!(ALLOC_BOX_VAL, box_val, &pool);
    alloc_loop!(ALLOC_BOX_WITH, box_with, &pool);
    alloc_loop!(ALLOC_BOX_UNINIT, box_uninit, &pool);
    alloc_loop!(ALLOC_BOX_UNSIZE, box_unsize, &pool);
    alloc_loop!(ALLOC_ARC_UNSIZE, arc_unsize, &pool);
    alloc_loop!(ALLOC_ARC_VAL, arc_val, &pool);
    alloc_loop!(ALLOC_ARC_WITH, arc_with, &pool);
    alloc_loop!(ALLOC_ARC_UNINIT, arc_uninit, &pool);
    alloc_loop!(ALLOC_ALLOC_VAL, alloc_val, &pool);
    alloc_loop!(ALLOC_ALLOC_WITH, alloc_with, &pool);
    alloc_loop!(ALLOC_ALLOC_UNINIT, alloc_uninit, &pool);
    alloc_loop!(ALLOC_RC_VAL, rc_val, &pool);
    alloc_loop!(ALLOC_RC_WITH, rc_with, &pool);
    alloc_loop!(ALLOC_RC_UNINIT, rc_uninit, &pool);
    alloc_loop!(ALLOC_MULTI_BOX_VAL, multi_box_val, &multi);
    alloc_loop!(ALLOC_MULTI_BOX_VAL_SPREAD, multi_box_val_spread, &multi_spread);
    alloc.bench_function(ALLOC_MULTI_BOX_VAL_MISS.benchmark_name(), |bencher| {
        bencher.iter_batched(
            || ops::setup_multi_pool_miss(ops::CAP),
            |pool| {
                ops::multi_box_val_miss(black_box(&pool), 0);
                pool
            },
            criterion::BatchSize::LargeInput,
        );
    });
    alloc.finish();

    let (_arc_pool, arc_base) = ops::setup_arc(ops::CAP);
    let (_rc_pool, rc_base) = ops::setup_rc(ops::CAP);
    let mut clone = criterion.benchmark_group(CLONE_ARC_CLONE.group_name());
    clone.bench_function(CLONE_ARC_CLONE.benchmark_name(), |bencher| {
        bencher.iter(|| {
            for _ in 0..N {
                ops::arc_clone(black_box(&arc_base));
            }
        });
    });
    clone.bench_function(CLONE_RC_CLONE.benchmark_name(), |bencher| {
        bencher.iter(|| {
            for _ in 0..N {
                ops::rc_clone(black_box(&rc_base));
            }
        });
    });
    clone.finish();

    let plurality = ops::setup_plurality(ops::CAP);
    let plurality_multi = ops::setup_plurality_multi(ops::CAP);
    let infinity = ops::setup_infinity_pinned(ops::CAP);
    let infinity_local = ops::setup_infinity_local_pinned(ops::CAP);
    let infinity_blind = ops::setup_infinity_blind(ops::CAP);
    let infinity_local_blind = ops::setup_infinity_local_blind(ops::CAP);
    ops::setup_std_box(ops::CAP);

    let mut dyn_box = criterion.benchmark_group(DYN_BOX_PLURALITY_BOX.group_name());
    macro_rules! dyn_loop {
        ($const_name:ident, $op:ident, $pool:expr) => {
            dyn_box.bench_function($const_name.benchmark_name(), |bencher| {
                bencher.iter(|| {
                    for i in 0..N {
                        ops::$op(black_box($pool), i);
                    }
                });
            });
        };
    }
    dyn_loop!(DYN_BOX_PLURALITY_BOX, plurality_box, &plurality);
    dyn_loop!(DYN_BOX_PLURALITY_MULTI_BOX, plurality_multi_box, &plurality_multi);
    dyn_loop!(DYN_BOX_INFINITY_PINNED, infinity_pinned, &infinity);
    dyn_loop!(DYN_BOX_INFINITY_LOCAL_PINNED, infinity_local_pinned, &infinity_local);
    dyn_loop!(DYN_BOX_INFINITY_BLIND, infinity_blind, &infinity_blind);
    dyn_loop!(DYN_BOX_INFINITY_LOCAL_BLIND, infinity_local_blind, &infinity_local_blind);
    dyn_box.bench_function(DYN_BOX_STD_BOX.benchmark_name(), |bencher| {
        bencher.iter(|| {
            for i in 0..N {
                ops::std_box(i);
            }
        });
    });
    dyn_box.finish();

    #[cfg(target_os = "linux")]
    {
        let plurality = pool_shared::setup_plurality(pool_shared::CAP);
        let mut slab = pool_shared::setup_slab(pool_shared::CAP);
        let sharded = pool_shared::setup_sharded_slab(pool_shared::CAP);
        let mut slotmap = pool_shared::setup_slotmap(pool_shared::CAP);
        let object_pool = pool_shared::setup_object_pool(pool_shared::CAP);
        let opool = pool_shared::setup_opool(pool_shared::CAP);
        let deadpool = pool_shared::setup_deadpool(pool_shared::CAP);
        let infinity = pool_shared::setup_infinity_pinned(pool_shared::CAP);
        let mut infinity_raw = pool_shared::setup_infinity_raw(pool_shared::CAP);

        let mut churn = criterion.benchmark_group(POOL_COMPARISON_CHURN_PLURALITY_BOX.group_name());
        macro_rules! churn_loop {
            ($const_name:ident, $op:path, $pool:expr) => {
                churn.bench_function($const_name.benchmark_name(), |bencher| {
                    bencher.iter(|| {
                        for i in 0..N {
                            $op(black_box($pool), i);
                        }
                    });
                });
            };
        }
        churn_loop!(POOL_COMPARISON_CHURN_PLURALITY_BOX, pool_shared::plurality_box, &plurality);
        churn_loop!(POOL_COMPARISON_CHURN_PLURALITY_ALLOC, pool_shared::plurality_alloc, &plurality);
        churn_loop!(POOL_COMPARISON_CHURN_SLAB_INSERT_REMOVE, pool_shared::slab_insert_remove, &mut slab);
        churn_loop!(
            POOL_COMPARISON_CHURN_SHARDED_SLAB_INSERT_REMOVE,
            pool_shared::sharded_slab_insert_remove,
            &sharded
        );
        churn_loop!(
            POOL_COMPARISON_CHURN_SLOTMAP_INSERT_REMOVE,
            pool_shared::slotmap_insert_remove,
            &mut slotmap
        );
        churn_loop!(POOL_COMPARISON_CHURN_OBJECT_POOL_PULL, pool_shared::object_pool_pull, &object_pool);
        churn_loop!(POOL_COMPARISON_CHURN_OPOOL_GET, pool_shared::opool_get, &opool);
        churn_loop!(POOL_COMPARISON_CHURN_DEADPOOL_GET, pool_shared::deadpool_get, &deadpool);
        churn_loop!(POOL_COMPARISON_CHURN_INFINITY_PINNED, pool_shared::infinity_pinned, &infinity);
        churn_loop!(POOL_COMPARISON_CHURN_INFINITY_RAW, pool_shared::infinity_raw, &mut infinity_raw);
        churn.finish();
    }

    let graph = graph_churn::prepare();
    let mut churn = criterion.benchmark_group("graph_churn");
    churn.bench_function("std_box_mimalloc", |bencher| {
        bencher.iter_custom(|iters| {
            let mut total = std::time::Duration::ZERO;
            for _ in 0..iters {
                let (elapsed, checksum) = graph_churn::bench_mimalloc(&graph);
                assert_eq!(checksum, graph_churn::expected_checksum(&graph));
                total += elapsed;
            }
            total
        });
    });
    churn.bench_function("plurality_pool", |bencher| {
        bencher.iter_custom(|iters| {
            let mut total = std::time::Duration::ZERO;
            for _ in 0..iters {
                let (elapsed, checksum) = graph_churn::bench_pool(&graph);
                assert_eq!(checksum, graph_churn::expected_checksum(&graph));
                total += elapsed;
            }
            total
        });
    });
    churn.finish();
}

fn callgrind_branch_config() -> LibraryBenchmarkConfig {
    let mut config = LibraryBenchmarkConfig::default();
    config.tool(Callgrind::with_args(["--branch-sim=yes"]).format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]));
    config
}

#[cfg(target_os = "linux")]
metabench::main!(
    criterion = criterion_benchmarks,
    groups = {
        ALLOC {
            benchmarks = [
                ALLOC_BOX_VAL,
                ALLOC_BOX_WITH,
                ALLOC_BOX_UNINIT,
                ALLOC_BOX_UNSIZE,
                ALLOC_ARC_UNSIZE,
                ALLOC_ARC_VAL,
                ALLOC_ARC_WITH,
                ALLOC_ARC_UNINIT,
                ALLOC_ALLOC_VAL,
                ALLOC_ALLOC_WITH,
                ALLOC_ALLOC_UNINIT,
                ALLOC_RC_VAL,
                ALLOC_RC_WITH,
                ALLOC_RC_UNINIT,
                ALLOC_MULTI_BOX_VAL,
                ALLOC_MULTI_BOX_VAL_SPREAD,
                ALLOC_MULTI_BOX_VAL_MISS,
            ],
            gungraun_config = callgrind_branch_config(),
        },
        CLONE {
            benchmarks = [CLONE_ARC_CLONE, CLONE_RC_CLONE],
            gungraun_config = callgrind_branch_config(),
        },
        DYN_BOX {
            benchmarks = [
                DYN_BOX_PLURALITY_BOX,
                DYN_BOX_PLURALITY_MULTI_BOX,
                DYN_BOX_INFINITY_PINNED,
                DYN_BOX_INFINITY_LOCAL_PINNED,
                DYN_BOX_INFINITY_BLIND,
                DYN_BOX_INFINITY_LOCAL_BLIND,
                DYN_BOX_STD_BOX,
            ],
            gungraun_config = callgrind_branch_config(),
        },
        POOL_COMPARISON {
            benchmarks = [
                POOL_COMPARISON_CHURN_PLURALITY_BOX,
                POOL_COMPARISON_CHURN_PLURALITY_ALLOC,
                POOL_COMPARISON_CHURN_SLAB_INSERT_REMOVE,
                POOL_COMPARISON_CHURN_SHARDED_SLAB_INSERT_REMOVE,
                POOL_COMPARISON_CHURN_SLOTMAP_INSERT_REMOVE,
                POOL_COMPARISON_CHURN_OBJECT_POOL_PULL,
                POOL_COMPARISON_CHURN_OPOOL_GET,
                POOL_COMPARISON_CHURN_DEADPOOL_GET,
                POOL_COMPARISON_CHURN_INFINITY_PINNED,
                POOL_COMPARISON_CHURN_INFINITY_RAW,
            ],
        },
    },
);

#[cfg(not(target_os = "linux"))]
metabench::main!(
    criterion = criterion_benchmarks,
    groups = {
        ALLOC {
            benchmarks = [
                ALLOC_BOX_VAL,
                ALLOC_BOX_WITH,
                ALLOC_BOX_UNINIT,
                ALLOC_BOX_UNSIZE,
                ALLOC_ARC_UNSIZE,
                ALLOC_ARC_VAL,
                ALLOC_ARC_WITH,
                ALLOC_ARC_UNINIT,
                ALLOC_ALLOC_VAL,
                ALLOC_ALLOC_WITH,
                ALLOC_ALLOC_UNINIT,
                ALLOC_RC_VAL,
                ALLOC_RC_WITH,
                ALLOC_RC_UNINIT,
                ALLOC_MULTI_BOX_VAL,
                ALLOC_MULTI_BOX_VAL_SPREAD,
                ALLOC_MULTI_BOX_VAL_MISS,
            ],
            gungraun_config = callgrind_branch_config(),
        },
        CLONE {
            benchmarks = [CLONE_ARC_CLONE, CLONE_RC_CLONE],
            gungraun_config = callgrind_branch_config(),
        },
        DYN_BOX {
            benchmarks = [
                DYN_BOX_PLURALITY_BOX,
                DYN_BOX_PLURALITY_MULTI_BOX,
                DYN_BOX_INFINITY_PINNED,
                DYN_BOX_INFINITY_LOCAL_PINNED,
                DYN_BOX_INFINITY_BLIND,
                DYN_BOX_INFINITY_LOCAL_BLIND,
                DYN_BOX_STD_BOX,
            ],
            gungraun_config = callgrind_branch_config(),
        },
    },
);
