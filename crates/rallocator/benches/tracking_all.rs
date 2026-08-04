mod workloads;

rallocator::config!(AllTrackingConfig {
    track_aggregates: true,
    track_callers: true,
});
rallocator::rallocator!(AllTrackingConfig);

fn main() {
    workloads::run();
}
