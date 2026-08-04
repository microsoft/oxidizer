use std::collections::HashMap;
use std::hint::black_box;
use std::sync::Arc;

use allocation_hints::heap::Heap;
use allocation_hints::heap::bump::Options as BumpOptions;
use allocation_hints::{Hint, with_hint};
use rallocator::telemetry::{snapshot, track_callers};

rallocator::config!(TrackingConfig {
    track_callers: true,
    track_aggregates: true,
});

rallocator::rallocator!(TrackingConfig);

fn main() {
    track_callers(true);

    for _ in 0..100 {
        let workers: Vec<_> = (0..4)
            .map(|worker| std::thread::spawn(move || worker_allocations(worker)))
            .collect();
        let retained: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker must not panic"))
            .collect();

        // let stats = stats().unwrap();
        // dbg!(stats);

        // let th = thread_heap().unwrap();
        // dbg!(th.info());
        // dbg!(th.usage().unwrap());

        let heap = Heap::from_thread_pool(BumpOptions::new());
        let _vec = with_hint(Hint::new().with_heap(&heap), || vec![1, 2, 3]);
        // dbg!(heap.usage().unwrap());

        // let snap = snapshot().unwrap();
        // snap.write_file("snapshot.rallocator").unwrap();
        // println!("wrote {} snapshot bytes", snap.as_bytes().len());

        drop(retained);
    }
    track_callers(false);

    let after_drop = snapshot().unwrap();
    after_drop.write_file("snapshot-after-drop2.rallocator").unwrap();
}

#[inline(never)]
fn worker_allocations(worker: usize) -> Vec<u8> {
    for round in 0..50 {
        let vectors: Vec<_> = (0..24)
            .map(|index| {
                let length = 8 + ((worker * 97 + round * 31 + index * 17) % 512);
                vec![(worker ^ round ^ index) as u64; length]
            })
            .collect();

        let map: HashMap<_, _> = (0..128)
            .map(|index| {
                let key = ((worker as u64) << 48) | ((round as u64) << 32) | index;
                (key, Arc::new([key; 8]))
            })
            .collect();

        black_box((&vectors, &map));
    }

    vec![worker as u8; 16 * 1024]
}
