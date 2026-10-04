// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Isolated benchmark engine: balanced broad fanout with overlapping tail refs.
//! Lifetime windows are burst counts, not a claimed replay of trace lifetimes.
//! Queue/vector storage and workers persist across every sample and warmup.

use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::hint::black_box;
use std::ptr::NonNull;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Barrier};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const REQUESTED_BYTES: usize = 32 * 1024;
const MIXED_BYTES: [usize; 8] = [16_384, 32_768, 32_768, 32_768, 32_768, 65_536, 98_304, 262_144];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Route {
    Local,
    BroadRemote,
    SkewedRemote,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TailPolicy {
    ReclaimUnique,
    AlwaysDetach,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Sizes {
    Uniform32K,
    Mixed,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Configuration {
    pub threads: usize,
    pub packets_per_burst: usize,
    pub chunks_per_packet: usize,
    pub retained_bursts: usize,
    pub touched_bytes: usize,
    pub route: Route,
    pub tail_policy: TailPolicy,
    pub sizes: Sizes,
}

impl Configuration {
    fn validate(self) {
        assert!((1..=64).contains(&self.threads));
        assert!(self.route == Route::Local || self.threads > 1);
        assert!((1..=256).contains(&self.packets_per_burst));
        assert!((1..=256).contains(&self.chunks_per_packet));
        assert!(self.retained_bursts <= 16);
        assert!((1..=REQUESTED_BYTES).contains(&self.touched_bytes));
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Counts {
    pub chunks: u64,
    pub raw_allocations: u64,
    pub unique_reuses: u64,
    pub copied_bytes: u64,
}

impl Counts {
    fn add(&mut self, other: Self) {
        self.chunks += other.chunks;
        self.raw_allocations += other.raw_allocations;
        self.unique_reuses += other.unique_reuses;
        self.copied_bytes += other.copied_bytes;
    }
}

/// The requested payload allocation is exactly its layout size. Arc metadata
/// is a separate small allocation, never folded into the requested 32 KiB.
struct Payload {
    pointer: NonNull<u8>,
    layout: Layout,
    initialized: usize,
    #[cfg(test)]
    dropped: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

// SAFETY: Only construction or exclusive &mut access writes the initialized
// prefix. Shared references read that prefix; Drop releases the unique backing.
unsafe impl Send for Payload {}
// SAFETY: Published payload bytes are immutable while shared. Arc::get_mut is
// the only route to subsequent mutation and excludes every concurrent reader.
unsafe impl Sync for Payload {}

impl Payload {
    fn new(bytes: usize, initialized: usize, value: u8) -> Self {
        assert!(bytes > 0 && initialized <= bytes);
        let layout = Layout::from_size_align(bytes, 16).expect("all configured payload sizes have valid layouts");
        // SAFETY: The nonzero layout is valid; allocation failure is handled.
        let pointer = NonNull::new(unsafe { alloc(layout) }).unwrap_or_else(|| handle_alloc_error(layout));
        let mut payload = Self {
            pointer,
            layout,
            initialized,
            #[cfg(test)]
            dropped: None,
        };
        payload.fill(value);
        payload
    }

    fn fill(&mut self, value: u8) {
        assert!(self.initialized <= self.layout.size());
        // SAFETY: Exclusive ownership permits writing the bounded prefix.
        unsafe { self.pointer.as_ptr().write_bytes(value, self.initialized) };
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: This prefix was initialized before publication and remains
        // immutable for this borrow. The unread suffix is never exposed.
        unsafe { std::slice::from_raw_parts(self.pointer.as_ptr(), self.initialized) }
    }
}

impl Drop for Payload {
    fn drop(&mut self) {
        // SAFETY: The final owning Payload uses its original allocation layout.
        unsafe { dealloc(self.pointer.as_ptr(), self.layout) };
        #[cfg(test)]
        if let Some(dropped) = &self.dropped {
            dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

type Packet = Vec<Arc<Payload>>;
type Frame = Vec<Packet>;

fn empty_frame(configuration: Configuration) -> Frame {
    (0..configuration.packets_per_burst)
        .map(|_| Vec::with_capacity(configuration.chunks_per_packet))
        .collect()
}

fn offsets(threads: usize) -> Vec<usize> {
    let mut offsets: Vec<_> = (1..threads).collect();
    let mut seed = 0xA076_1D64_78BD_642F_u64;
    for index in (1..offsets.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let swap = usize::try_from(seed % (index as u64 + 1)).expect("shuffle index is bounded by the usize-sized vector");
        offsets.swap(index, swap);
    }
    offsets
}

fn destination(configuration: Configuration, offsets: &[usize], worker: usize, packet: usize, epoch: usize) -> usize {
    if configuration.route == Route::Local {
        return worker;
    }
    let ordinal = epoch.wrapping_mul(configuration.packets_per_burst).wrapping_add(packet);
    let offset = if configuration.route == Route::SkewedRemote && !ordinal.is_multiple_of(4) {
        1
    } else {
        offsets[ordinal % offsets.len()]
    };
    (worker + offset) % configuration.threads
}

fn publish_tail(
    tail: &mut Option<Arc<Payload>>,
    bytes: usize,
    initialized: usize,
    value: u8,
    policy: TailPolicy,
    counts: &mut Counts,
) -> Arc<Payload> {
    counts.chunks += 1;
    if policy == TailPolicy::ReclaimUnique
        && let Some(current) = tail
        && let Some(payload) = Arc::get_mut(current)
        && payload.layout.size() == bytes
    {
        payload.initialized = initialized;
        payload.fill(value);
        counts.unique_reuses += 1;
        return Arc::clone(current);
    }
    // Reserve a replacement BEFORE releasing the old producer tail. Its last
    // consumer may still be live; moving threads itself does not force a detach.
    let replacement = Arc::new(Payload::new(bytes, initialized, value));
    let published = Arc::clone(&replacement);
    *tail = Some(replacement);
    counts.raw_allocations += 1;
    published
}

struct Worker {
    configuration: Configuration,
    index: usize,
    offsets: Arc<[usize]>,
    outgoing: Vec<SyncSender<Packet>>,
    incoming: Receiver<Packet>,
    barrier: Arc<Barrier>,
    batches: Frame,
    retained: Vec<Frame>,
    tail: Option<Arc<Payload>>,
    scratch: Vec<u8>,
    epoch: usize,
}

impl Worker {
    fn run(&mut self, iterations: u64) -> Counts {
        let mut counts = Counts::default();
        for _ in 0..iterations {
            for (packet_index, mut packet) in self.batches.drain(..).enumerate() {
                assert!(packet.is_empty());
                for chunk in 0..self.configuration.chunks_per_packet {
                    let ordinal = packet_index * self.configuration.chunks_per_packet + chunk;
                    let bytes = match self.configuration.sizes {
                        Sizes::Uniform32K => REQUESTED_BYTES,
                        Sizes::Mixed => MIXED_BYTES[(ordinal + self.index + self.epoch) % MIXED_BYTES.len()],
                    };
                    let value = ordinal.wrapping_add(self.index).wrapping_add(self.epoch).to_le_bytes()[0];
                    packet.push(publish_tail(
                        &mut self.tail,
                        bytes,
                        self.configuration.touched_bytes.min(bytes),
                        value,
                        self.configuration.tail_policy,
                        &mut counts,
                    ));
                }
                let destination = destination(self.configuration, &self.offsets, self.index, packet_index, self.epoch);
                self.outgoing[destination]
                    .send(packet)
                    .expect("all consumers remain alive until the common shutdown");
            }
            for _ in 0..self.configuration.packets_per_burst {
                self.batches.push(
                    self.incoming
                        .recv()
                        .expect("balanced routing delivers one packet per producer phase"),
                );
            }
            if !self.retained.is_empty() {
                let slot = self.epoch % self.retained.len();
                std::mem::swap(&mut self.batches, &mut self.retained[slot]);
            }
            for packet in &mut self.batches {
                for payload in packet.iter() {
                    let bytes = payload.bytes();
                    self.scratch[..bytes.len()].copy_from_slice(bytes);
                    black_box(&self.scratch[..bytes.len()]);
                    counts.copied_bytes += bytes.len() as u64;
                }
                packet.clear();
            }
            // This bounds all queue occupancy to one burst and prevents a fast
            // producer from lapping another worker into a later routing epoch.
            // It is intentionally part of both local and remote measurements.
            self.barrier.wait();
            self.epoch = self.epoch.wrapping_add(1);
        }
        counts
    }
}

pub(crate) struct FanoutWorkers {
    starters: Vec<SyncSender<Option<u64>>>,
    finished: Receiver<Counts>,
    workers: Vec<JoinHandle<()>>,
}

impl FanoutWorkers {
    /// `processors` is empty for the unpinned control. Affinity is injected so
    /// this engine requires only std and shares the host benchmark's OS helper.
    pub(crate) fn new(configuration: Configuration, processors: &[usize], pin: fn(usize)) -> Self {
        configuration.validate();
        assert!(processors.is_empty() || processors.len() == configuration.threads);
        let offsets: Arc<[usize]> = offsets(configuration.threads).into();
        let barrier = Arc::new(Barrier::new(configuration.threads));
        let (senders, receivers): (Vec<_>, Vec<_>) = (0..configuration.threads)
            .map(|_| mpsc::sync_channel::<Packet>(configuration.packets_per_burst))
            .unzip();
        let (ready, ready_workers) = mpsc::sync_channel(0);
        let (finished, completed) = mpsc::sync_channel(configuration.threads);
        let mut starters = Vec::with_capacity(configuration.threads);
        let mut workers = Vec::with_capacity(configuration.threads);
        for (index, incoming) in receivers.into_iter().enumerate() {
            let (start, started) = mpsc::sync_channel::<Option<u64>>(0);
            starters.push(start);
            let outgoing = senders.clone();
            let offsets = Arc::clone(&offsets);
            let barrier = Arc::clone(&barrier);
            let ready = ready.clone();
            let finished = finished.clone();
            let processor = processors.get(index).copied();
            workers.push(thread::spawn(move || {
                if let Some(processor) = processor {
                    pin(processor);
                }
                let mut worker = Worker {
                    configuration,
                    index,
                    offsets,
                    outgoing,
                    incoming,
                    barrier,
                    batches: empty_frame(configuration),
                    retained: (0..configuration.retained_bursts).map(|_| empty_frame(configuration)).collect(),
                    tail: None,
                    scratch: vec![0; configuration.touched_bytes],
                    epoch: 0,
                };
                ready.send(()).expect("coordinator waits for worker-local initialization");
                while let Some(iterations) = started.recv().expect("coordinator sends an explicit shutdown") {
                    finished.send(worker.run(iterations)).expect("coordinator collects every sample");
                }
            }));
        }
        for _ in 0..configuration.threads {
            ready_workers.recv().expect("each worker signals initialized storage");
        }
        Self {
            starters,
            finished: completed,
            workers,
        }
    }

    pub(crate) fn time(&self, iterations: u64) -> (Duration, Counts) {
        let start = Instant::now();
        for starter in &self.starters {
            starter
                .send(Some(iterations))
                .expect("workers persist for the entire benchmark case");
        }
        let mut counts = Counts::default();
        for _ in &self.starters {
            counts.add(self.finished.recv().expect("every worker completes the same number of bursts"));
        }
        (start.elapsed(), counts)
    }
}

impl Drop for FanoutWorkers {
    fn drop(&mut self) {
        for starter in &self.starters {
            starter.send(None).expect("idle workers await case shutdown");
        }
        for worker in self.workers.drain(..) {
            worker
                .join()
                .expect("all worker-owned tails and retained frames are released at case shutdown");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Default for Configuration {
        fn default() -> Self {
            Self {
                threads: 2,
                packets_per_burst: 2,
                chunks_per_packet: 2,
                retained_bursts: 1,
                touched_bytes: 1024,
                route: Route::BroadRemote,
                tail_policy: TailPolicy::ReclaimUnique,
                sizes: Sizes::Uniform32K,
            }
        }
    }

    #[test]
    fn routing_is_balanced_remote_and_covers_every_other_worker() {
        for threads in [2, 8, 64] {
            let config = Configuration {
                threads,
                ..Configuration::default()
            };
            let offsets = offsets(threads);
            let mut visited = vec![vec![false; threads]; threads];
            for epoch in 0..threads {
                for packet in 0..config.packets_per_burst {
                    let mut incoming = vec![0; threads];
                    for (worker, destinations) in visited.iter_mut().enumerate() {
                        let destination = destination(config, &offsets, worker, packet, epoch);
                        assert_ne!(destination, worker);
                        incoming[destination] += 1;
                        destinations[destination] = true;
                    }
                    assert_eq!(incoming, vec![1; threads]);
                }
            }
            for (worker, destinations) in visited.into_iter().enumerate() {
                assert_eq!(destinations, (0..threads).map(|other| other != worker).collect::<Vec<_>>());
            }
        }
    }

    #[test]
    fn producer_tail_and_consumer_share_until_the_actual_last_reference() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dropped = Arc::new(AtomicUsize::new(0));
        let mut payload = Payload::new(REQUESTED_BYTES, 1024, 7);
        payload.dropped = Some(Arc::clone(&dropped));
        let tail = Arc::new(payload);
        let consumer = Arc::clone(&tail);
        thread::spawn(move || drop(tail)).join().unwrap();
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        assert_eq!(consumer.bytes(), &[7; 1024]);
        drop(consumer);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn consumer_keeps_backing_live_after_the_allocating_worker_exits() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dropped = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&dropped);
        let consumer = thread::spawn(move || {
            let mut payload = Payload::new(REQUESTED_BYTES, 1024, 9);
            payload.dropped = Some(observed);
            let tail = Arc::new(payload);
            Arc::clone(&tail)
        })
        .join()
        .unwrap();
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        assert_eq!(consumer.bytes(), &[9; 1024]);
        drop(consumer);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn consumer_releases_backing_while_the_allocating_worker_is_idle() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dropped = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&dropped);
        let (sent, received) = mpsc::sync_channel(1);
        let (release, idle) = mpsc::sync_channel(0);
        let producer = thread::spawn(move || {
            let mut payload = Payload::new(REQUESTED_BYTES, 1024, 11);
            payload.dropped = Some(observed);
            sent.send(Arc::new(payload)).unwrap();
            idle.recv().unwrap();
        });
        let consumer = received.recv().unwrap();
        drop(consumer);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        release.send(()).unwrap();
        producer.join().unwrap();
    }

    #[test]
    fn only_unique_matching_backing_is_reclaimed() {
        let mut tail = None;
        let mut counts = Counts::default();
        let first = publish_tail(&mut tail, REQUESTED_BYTES, 1024, 1, TailPolicy::ReclaimUnique, &mut counts);
        let second = publish_tail(&mut tail, REQUESTED_BYTES, 1024, 2, TailPolicy::ReclaimUnique, &mut counts);
        assert_eq!(first.bytes(), &[1; 1024]);
        let pointer = second.pointer;
        drop(second);
        let reused = publish_tail(&mut tail, REQUESTED_BYTES, 1024, 3, TailPolicy::ReclaimUnique, &mut counts);
        assert_eq!(reused.pointer, pointer);
        assert_eq!(reused.bytes(), &[3; 1024]);
        assert_eq!(
            counts,
            Counts {
                chunks: 3,
                raw_allocations: 2,
                unique_reuses: 1,
                copied_bytes: 0
            }
        );
    }

    #[test]
    fn retained_windows_survive_sample_boundaries_and_shutdown() {
        let config = Configuration {
            retained_bursts: 2,
            ..Configuration::default()
        };
        let workers = FanoutWorkers::new(config, &[], |_| {});
        let (_, first) = workers.time(1);
        let (_, second) = workers.time(3);
        assert_eq!(first.copied_bytes, 0);
        assert_eq!(first.chunks + second.chunks, 32);
        assert_eq!(second.copied_bytes, 16 * 1024);
        assert_eq!(first.raw_allocations + second.raw_allocations, 32);
        drop(workers);
    }

    #[test]
    fn local_mixed_and_skewed_controls_make_bounded_progress() {
        for (threads, route) in [(1, Route::Local), (2, Route::SkewedRemote), (8, Route::BroadRemote)] {
            let config = Configuration {
                threads,
                route,
                sizes: Sizes::Mixed,
                tail_policy: TailPolicy::AlwaysDetach,
                ..Configuration::default()
            };
            let workers = FanoutWorkers::new(config, &[], |_| {});
            let (_, counts) = workers.time(4);
            assert_eq!(counts.chunks, threads as u64 * 16);
            assert_eq!(counts.raw_allocations, counts.chunks);
            assert_eq!(counts.unique_reuses, 0);
            assert_eq!(counts.copied_bytes, threads as u64 * 12 * 1024);
        }
    }

    #[test]
    #[cfg(not(miri))]
    fn full_bursts_fill_and_recycle_the_four_burst_window() {
        for chunks_per_packet in [1, 4, 16] {
            let config = Configuration {
                threads: 8,
                packets_per_burst: 256 / chunks_per_packet,
                chunks_per_packet,
                retained_bursts: 4,
                touched_bytes: 16 * 1024,
                ..Configuration::default()
            };
            let workers = FanoutWorkers::new(config, &[], |_| {});
            let (_, counts) = workers.time(6);
            assert_eq!(
                counts,
                Counts {
                    chunks: 12_288,
                    raw_allocations: 12_288,
                    unique_reuses: 0,
                    copied_bytes: 67_108_864
                }
            );
        }
    }
}
