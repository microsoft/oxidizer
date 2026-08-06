//! General allocation workload benchmark using mimalloc.

mod workloads;

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() {
    workloads::run();
}
