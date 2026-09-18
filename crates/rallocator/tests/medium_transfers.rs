// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Medium allocation ownership across thread handoff and thread exit.

use std::sync::mpsc;
use std::thread;

rallocator::rallocator!();

const WORKERS: usize = if cfg!(miri) { 2 } else { 8 };
const BLOCKS: usize = if cfg!(miri) { 4 } else { 32 };
const ROUNDS: usize = if cfg!(miri) { 2 } else { 16 };
const SIZES: [usize; 4] = [32 * 1024, 64 * 1024, 96 * 1024, 256 * 1024];

fn batch(worker: usize, round: usize) -> Vec<Vec<u8>> {
    (0..BLOCKS)
        .map(|block| vec![pattern(worker, round, block); SIZES[block % SIZES.len()]])
        .collect()
}

fn pattern(worker: usize, round: usize, block: usize) -> u8 {
    (worker * 31 + round * 17 + block).to_le_bytes()[0]
}

fn verify(buffers: &[Vec<u8>], worker: usize, round: usize) {
    assert_eq!(buffers.len(), BLOCKS);
    for (block, buffer) in buffers.iter().enumerate() {
        let expected = pattern(worker, round, block);
        assert_eq!(buffer.len(), SIZES[block % SIZES.len()]);
        assert!(
            buffer.iter().all(|&byte| byte == expected),
            "buffer changed: worker={worker}, round={round}, block={block}"
        );
    }
}

#[test]
fn medium_buffers_survive_allocating_thread_exit() {
    let buffers = thread::scope(|scope| {
        #[expect(
            clippy::needless_collect,
            reason = "All producers must start before any join to preserve concurrent allocation pressure"
        )]
        let workers: Vec<_> = (0..WORKERS).map(|worker| scope.spawn(move || batch(worker, 0))).collect();
        workers.into_iter().map(|worker| worker.join().unwrap()).collect::<Vec<_>>()
    });
    thread::spawn(move || {
        for (worker, buffers) in buffers.into_iter().enumerate() {
            verify(&buffers, worker, 0);
            drop(buffers);
            let reused = batch(worker, 1);
            verify(&reused, worker, 1);
        }
    })
    .join()
    .unwrap();
}

#[test]
#[cfg_attr(miri, ignore = "cross-thread reuse stress is exercised by native tests")]
fn remote_medium_frees_complete_while_allocating_workers_wait() {
    thread::scope(|scope| {
        let (sender, receiver) = mpsc::sync_channel(WORKERS);
        let mut acknowledgements = Vec::with_capacity(WORKERS);
        for worker in 0..WORKERS {
            let sender = sender.clone();
            let (acknowledge, acknowledged) = mpsc::sync_channel(0);
            acknowledgements.push(acknowledge);
            scope.spawn(move || {
                for round in 0..ROUNDS {
                    sender.send((worker, round, batch(worker, round))).unwrap();
                    acknowledged.recv().unwrap();
                }
            });
        }
        drop(sender);
        for (worker, round, buffers) in receiver {
            verify(&buffers, worker, round);
            drop(buffers);
            // Reuse backing on the consumer before the original owner runs again.
            let reused = batch(worker, round + 1);
            verify(&reused, worker, round + 1);
            drop(reused);
            acknowledgements[worker].send(()).unwrap();
        }
    });
}
