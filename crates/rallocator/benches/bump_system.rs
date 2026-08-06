//! Bump workload benchmark using the system allocator.

mod bump_workloads;
mod ordinary_workloads;

fn main() {
    bump_workloads::run(ordinary_workloads::WORKLOADS);
}
