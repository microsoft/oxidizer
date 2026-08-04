mod bump_workloads;
mod ordinary_workloads;

rallocator::rallocator!();

fn main() {
    bump_workloads::run(ordinary_workloads::WORKLOADS);
}
