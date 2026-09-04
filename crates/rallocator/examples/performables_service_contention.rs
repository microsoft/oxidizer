// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Contention-heavy in-process service workload for unified allocator and runtime telemetry.

use std::collections::VecDeque;
use std::hint::black_box;
use std::pin::pin;
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use performables::arc::Arc;
use performables::sync::lock::RwLock;
use performables::sync::mutex::Mutex;

rallocator::rallocator!();

const REQUEST_WORKERS: usize = 8;
const ROUTE_COUNT: usize = 4_096;
const REQUEST_INTERVAL: Duration = Duration::from_millis(10);
const ROUTE_REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const COMPLETION_POLL_INTERVAL: Duration = Duration::from_millis(5);
const REPORT_INTERVAL: Duration = Duration::from_secs(10);

struct Service {
    routes: RwLock<RouteTable>,
    completions: Mutex<VecDeque<Completion>>,
    configuration: Arc<ConfigurationSnapshot>,
    requests_completed: AtomicU64,
}

struct RouteTable {
    generation: u64,
    routes: Vec<Route>,
}

struct Route {
    partition: u32,
    endpoint: String,
}

struct ConfigurationSnapshot {
    tenant: String,
    scoring_weights: Vec<f64>,
}

struct Request {
    id: u64,
    query: String,
    payload: Vec<u8>,
}

struct Completion {
    request: Arc<Request>,
    partition: u32,
    generation: u64,
    checksum: u64,
}

struct ThreadWaker(thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: StdArc<Self>) {
        self.0.unpark();
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _monitor = seismograph::monitor::Monitor::builder().name("3S service contention").start()?;

    let service = Arc::new(Service {
        routes: RwLock::new(build_routes(0)),
        completions: Mutex::new(VecDeque::with_capacity(REQUEST_WORKERS * 64)),
        configuration: Arc::new(ConfigurationSnapshot {
            tenant: "telemetry-load-test".to_owned(),
            scoring_weights: (0..128).map(|index| f64::from(index) / 127.0).collect(),
        }),
        requests_completed: AtomicU64::new(0),
    });

    println!(
        "3S workload running as PID {}; telemetry starts disabled and is controlled through `seismograph monitor`",
        std::process::id()
    );
    run_workload(&service)?;
    Ok(())
}

fn run_workload(service: &Arc<Service>) -> Result<(), std::io::Error> {
    let start = Arc::new(std::sync::Barrier::new(REQUEST_WORKERS + 3));

    let mut threads = Vec::with_capacity(REQUEST_WORKERS + 2);
    for worker_id in 0..REQUEST_WORKERS {
        let service = Arc::clone(service);
        let start = Arc::clone(&start);
        threads.push(
            thread::Builder::new()
                .name(format!("request-worker-{worker_id}"))
                .spawn(move || request_worker(worker_id, &service, &start))?,
        );
    }

    {
        let service = Arc::clone(service);
        let start = Arc::clone(&start);
        threads.push(
            thread::Builder::new()
                .name("routing-refresher".to_owned())
                .spawn(move || routing_refresher(&service, &start))?,
        );
    }
    {
        let service = Arc::clone(service);
        let start = Arc::clone(&start);
        threads.push(
            thread::Builder::new()
                .name("completion-consumer".to_owned())
                .spawn(move || completion_consumer(&service, &start))?,
        );
    }

    start.wait();
    println!("started {} throttled workload threads", threads.len());
    let mut previous = 0;
    loop {
        thread::sleep(REPORT_INTERVAL);
        let completed = service.requests_completed.load(Ordering::Relaxed);
        let interval = completed.saturating_sub(previous);
        let requests_per_second = interval / REPORT_INTERVAL.as_secs();
        println!("completed {completed} requests ({requests_per_second}/s)");
        previous = completed;
    }
}

fn request_worker(worker_id: usize, service: &Arc<Service>, start: &std::sync::Barrier) {
    start.wait();
    let mut sequence = worker_id as u64;
    loop {
        let request = Arc::new(Request {
            id: sequence,
            query: format!("tenant:{} query:{sequence}", service.configuration.tenant),
            payload: vec![(sequence & 0xff) as u8; 8 * 1024 + usize::try_from(sequence & 0x3fff).expect("masked sequence fits usize")],
        });
        let (partition, generation, checksum) = route_request(service, &request);
        publish_completion(
            service,
            Completion {
                request,
                partition,
                generation,
                checksum,
            },
        );
        service.requests_completed.fetch_add(1, Ordering::Relaxed);
        sequence = sequence.wrapping_add(REQUEST_WORKERS as u64);
        thread::sleep(REQUEST_INTERVAL);
    }
}

fn route_request(service: &Arc<Service>, request: &Arc<Request>) -> (u32, u64, u64) {
    let routes = block_on(service.routes.read());
    let checksum = request
        .payload
        .iter()
        .zip(service.configuration.scoring_weights.iter().cycle())
        .fold(0_u64, |checksum, (byte, weight)| {
            checksum.rotate_left(5) ^ u64::from(*byte) ^ weight.to_bits()
        });
    let mixed = checksum ^ u64::try_from(request.query.len()).expect("query length fits u64");
    let index = usize::try_from(mixed % routes.routes.len() as u64).expect("route index fits usize");
    let route = &routes.routes[index];
    black_box(route.endpoint.as_str());
    (route.partition, routes.generation, checksum)
}

fn publish_completion(service: &Arc<Service>, completion: Completion) {
    let mut completions = block_on(service.completions.lock());
    completions.push_back(completion);
    if completions.len().is_multiple_of(64) {
        serialize_completion_batch(&completions);
    }
}

fn serialize_completion_batch(completions: &VecDeque<Completion>) {
    let checksum = completions.iter().rev().take(64).fold(0_u64, |value, completion| {
        value ^ completion.checksum ^ completion.request.id ^ u64::from(completion.partition) ^ completion.generation
    });
    black_box(checksum);
    thread::sleep(Duration::from_micros(250));
}

fn routing_refresher(service: &Arc<Service>, start: &std::sync::Barrier) {
    start.wait();
    let mut generation = 1_u64;
    loop {
        thread::sleep(ROUTE_REFRESH_INTERVAL);
        let mut routes = block_on(service.routes.write());
        *routes = build_routes(generation);
        black_box(routes.routes.iter().map(|route| route.endpoint.len()).sum::<usize>());
        thread::sleep(Duration::from_millis(1));
        generation = generation.wrapping_add(1);
    }
}

fn completion_consumer(service: &Arc<Service>, start: &std::sync::Barrier) {
    start.wait();
    loop {
        let mut completions = block_on(service.completions.lock());
        let drain_count = completions.len().min(128);
        let drained = completions.drain(..drain_count).collect::<Vec<_>>();
        black_box(
            drained
                .iter()
                .map(|completion| completion.request.query.len() + completion.request.payload.len())
                .sum::<usize>(),
        );
        if !drained.is_empty() {
            thread::sleep(Duration::from_micros(400));
        }
        drop(completions);
        thread::sleep(COMPLETION_POLL_INTERVAL);
    }
}

fn build_routes(generation: u64) -> RouteTable {
    RouteTable {
        generation,
        routes: (0..ROUTE_COUNT)
            .map(|partition| Route {
                partition: u32::try_from(partition).expect("route count fits u32"),
                endpoint: format!("https://partition-{partition}.example.test/search"),
            })
            .collect(),
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(StdArc::new(ThreadWaker(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park(),
        }
    }
}
