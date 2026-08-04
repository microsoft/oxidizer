mod workloads;

rallocator::config!(AggregatesConfig { track_aggregates: true });
rallocator::rallocator!(AggregatesConfig);

fn main() {
    workloads::run();
}
