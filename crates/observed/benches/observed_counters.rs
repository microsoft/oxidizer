// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Direct atomic updates versus public bound counter emission.

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{Criterion, criterion_group, criterion_main};
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView, MetricBindings};
use observed::{FlushError, Sink, Value, emit, event};
use tick::SimpleClock;

#[event("request.completed")]
#[counter(bytes_sent, name = "bytes_sent", unit = "By")]
struct RequestCompleted {
    #[unredacted]
    bytes_sent: u32,
}

/// Benchmark-only bound destination updating the baseline atomic without native publishing.
struct Processor {
    bindings: MetricBindings<Arc<AtomicU64>>,
}

impl EventProcessor for Processor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        self.bindings.is_interested(description)
    }

    fn process(&self, event: &EventView<'_>) {
        self.bindings
            .record(event, |counter, value| {
                if let Value::U64(value) = value {
                    counter.fetch_add(value, Ordering::Relaxed);
                } else {
                    unreachable!("the event declares an unsigned counter");
                }
            })
            .expect("generated field metadata and visitor descriptors share their indices");
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

fn counters(c: &mut Criterion) {
    let counter = Arc::new(AtomicU64::new(0));
    let mut bindings = MetricBindings::new();
    bindings
        .bind_event::<RequestCompleted>(|_| Arc::clone(&counter))
        .expect("the event declares one unsigned counter and a typed identity");
    let sink = Sink::builder("counters")
        .processors(vec![Arc::new(Processor { bindings })])
        .clock(SimpleClock::new_system())
        .build();
    // Both cases use the same arbitrary nonzero increment; its magnitude is not a workload model.
    let increment = 4096_u32;
    let mut group = c.benchmark_group("observed_counters/record");
    group.bench_function("atomic", |b| {
        b.iter(|| black_box(counter.fetch_add(u64::from(black_box(increment)), Ordering::Relaxed)));
    });
    group.bench_function("emit", |b| {
        b.iter(|| {
            emit!(
                black_box(&sink),
                RequestCompleted {
                    bytes_sent: black_box(increment)
                }
            );
        });
    });
    group.finish();
}

criterion_group!(benches, counters);
criterion_main!(benches);
