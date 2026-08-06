//! General allocation workload benchmark using rallocator.

mod workloads;

rallocator::rallocator!();

fn main() {
    rallocator::initialize();
    workloads::run();
}
