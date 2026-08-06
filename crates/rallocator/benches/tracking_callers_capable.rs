//! Allocation benchmark with caller-tracking capability compiled in.

mod workloads;

rallocator::config!(CallersCapableConfig { track_callers: true });
rallocator::rallocator!(CallersCapableConfig);

fn main() {
    workloads::run();
}
