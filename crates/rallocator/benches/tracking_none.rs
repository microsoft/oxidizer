//! Allocation benchmark with telemetry disabled.

mod workloads;

rallocator::rallocator!();

fn main() {
    workloads::run();
}
