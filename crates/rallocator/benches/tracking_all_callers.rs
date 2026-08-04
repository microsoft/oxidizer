mod workloads;

rallocator::config!(AllTrackingWithCallersConfig {
    track_aggregates: true,
    track_callers: true,
});
rallocator::rallocator!(AllTrackingWithCallersConfig);

fn main() {
    rallocator::telemetry::track_callers(true);
    workloads::run();
}
