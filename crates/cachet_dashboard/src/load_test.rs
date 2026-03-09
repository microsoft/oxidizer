// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Load test engine: configurable concurrent workers hitting a multi-tier cache.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cachet::{Cache, CacheEntry, FallbackPromotionPolicy};
use cachet_redis::RedisCache;
use cachet_tier::{CacheTier, DynamicCache};
use layered::Layer;
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use seatbelt::{RecoveryInfo, ResilienceContext, retry::Retry, timeout::Timeout};
use serde::{Deserialize, Serialize};
use tick::Clock;
use tokio::sync::watch;

/// Configuration for a load test run.
#[derive(Debug, Clone, Deserialize)]
pub struct LoadTestConfig {
    /// Number of concurrent worker tasks.
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// Total test duration in seconds.
    #[serde(default = "default_duration")]
    pub duration_secs: u64,
    /// Number of distinct keys to use.
    #[serde(default = "default_key_count")]
    pub key_count: u64,
    /// Fraction of operations that are reads (0.0–1.0).
    #[serde(default = "default_read_ratio")]
    pub read_ratio: f64,
    /// TTL for L1 memory tier (seconds). Short values cause L1 misses that fall through to L2.
    #[serde(default = "default_memory_ttl")]
    pub memory_ttl_secs: u64,
    /// TTL for L2 Redis tier (seconds). 0 = no TTL.
    #[serde(default = "default_redis_ttl")]
    pub redis_ttl_secs: u64,
    /// Key prefix for load test keys.
    #[serde(default = "default_prefix")]
    pub key_prefix: String,
    /// Wrap Redis tier with seatbelt retry+timeout.
    #[serde(default)]
    pub use_resilience: bool,
    /// Enable stampede protection (coalesces concurrent misses for the same key).
    #[serde(default = "default_stampede_protection")]
    pub use_stampede_protection: bool,
}

fn default_concurrency() -> usize {
    8
}
fn default_duration() -> u64 {
    10
}
fn default_key_count() -> u64 {
    1000
}
fn default_read_ratio() -> f64 {
    0.8
}
fn default_memory_ttl() -> u64 {
    5
}
fn default_redis_ttl() -> u64 {
    60
}
fn default_prefix() -> String {
    "loadtest:".to_string()
}
fn default_stampede_protection() -> bool {
    true
}

/// Live metrics snapshot sent over SSE.
#[derive(Debug, Clone, Serialize, Default)]
pub struct LoadTestMetrics {
    pub ops_per_sec: f64,
    pub total_ops: u64,
    pub total_hits: u64,
    pub total_misses: u64,
    pub total_inserts: u64,
    pub total_errors: u64,
    pub total_service_calls: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub elapsed_secs: f64,
    pub running: bool,
    /// OpenTelemetry metrics collected from the cache's telemetry integration.
    pub telemetry: Vec<TelemetryMetric>,
}

/// A single OpenTelemetry metric data point, serialized for the frontend.
#[derive(Debug, Clone, Serialize, Default)]
pub struct TelemetryMetric {
    pub name: String,
    pub metric_type: String,
    pub attributes: Vec<(String, String)>,
    /// For counters (Sum).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// For histograms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// For gauges.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gauge_value: Option<f64>,
}

/// Shared atomic counters for workers.
struct Counters {
    ops: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    inserts: AtomicU64,
    errors: AtomicU64,
}

impl Counters {
    fn new() -> Self {
        Self {
            ops: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            inserts: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }
}

/// Simulated external service tier (L3). Always returns a value, tracking call count.
struct SimulatedServiceTier {
    call_count: Arc<AtomicU64>,
}

impl CacheTier<String, String> for SimulatedServiceTier {
    async fn get(&self, key: &String) -> Result<Option<CacheEntry<String>>, cachet::Error> {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        Ok(Some(CacheEntry::new(format!("svc-{key}"))))
    }

    async fn insert(
        &self,
        _key: &String,
        _entry: CacheEntry<String>,
    ) -> Result<(), cachet::Error> {
        Ok(())
    }

    async fn invalidate(&self, _key: &String) -> Result<(), cachet::Error> {
        Ok(())
    }

    async fn clear(&self) -> Result<(), cachet::Error> {
        Ok(())
    }
}

/// Launches the load test orchestrator. Returns a stop flag that can be set to `true`.
pub fn start(
    config: LoadTestConfig,
    conn: redis::aio::ConnectionManager,
    metrics_tx: watch::Sender<Option<LoadTestMetrics>>,
    stop_flag: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run_load_test(config, conn, metrics_tx, stop_flag).await;
    })
}

async fn run_load_test(
    config: LoadTestConfig,
    conn: redis::aio::ConnectionManager,
    metrics_tx: watch::Sender<Option<LoadTestMetrics>>,
    stop_flag: Arc<AtomicBool>,
) {
    let clock = Clock::new_tokio();

    // Set up OpenTelemetry in-memory metrics collection
    let exporter = InMemoryMetricExporter::default();
    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter.clone())
        .build();
    let otel = OtelState {
        provider: meter_provider.clone(),
        exporter,
    };

    // Build the multi-tier cache with telemetry enabled
    let service_calls = Arc::new(AtomicU64::new(0));
    let cache = Arc::new(build_cache(
        &config,
        conn,
        &clock,
        &meter_provider,
        Arc::clone(&service_calls),
    ));

    let counters = Arc::new(Counters::new());
    let latencies: Arc<parking_lot::Mutex<Vec<u64>>> =
        Arc::new(parking_lot::Mutex::new(Vec::with_capacity(10_000)));

    let deadline = Instant::now() + Duration::from_secs(config.duration_secs);
    let start_time = Instant::now();

    // Spawn worker tasks
    let mut workers = Vec::with_capacity(config.concurrency);
    for _ in 0..config.concurrency {
        let cache = Arc::clone(&cache);
        let counters = Arc::clone(&counters);
        let latencies = Arc::clone(&latencies);
        let stop = Arc::clone(&stop_flag);
        let cfg = config.clone();

        workers.push(tokio::spawn(async move {
            worker_loop(cache, cfg, counters, latencies, deadline, stop).await;
        }));
    }

    // Metrics snapshot loop (every 500ms)
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    let mut prev_ops: u64 = 0;
    let mut prev_time = start_time;

    loop {
        interval.tick().await;

        if stop_flag.load(Ordering::Relaxed) || Instant::now() >= deadline {
            break;
        }

        let snapshot = build_snapshot(
            &counters,
            &latencies,
            &mut prev_ops,
            &mut prev_time,
            start_time,
            true,
            &otel,
            &service_calls,
        );

        let _ = metrics_tx.send(Some(snapshot));
    }

    // Signal stop and wait for workers
    stop_flag.store(true, Ordering::Relaxed);
    for w in workers {
        let _ = w.await;
    }

    // Final snapshot
    let final_snapshot = build_snapshot(
        &counters,
        &latencies,
        &mut prev_ops,
        &mut prev_time,
        start_time,
        false,
        &otel,
        &service_calls,
    );

    let _ = metrics_tx.send(Some(final_snapshot));
}

/// Bundled telemetry state for the in-memory metrics pipeline.
struct OtelState {
    provider: SdkMeterProvider,
    exporter: InMemoryMetricExporter,
}

#[expect(clippy::cast_precision_loss, reason = "counters are fine as f64")]
fn build_snapshot(
    counters: &Counters,
    latencies: &parking_lot::Mutex<Vec<u64>>,
    prev_ops: &mut u64,
    prev_time: &mut Instant,
    start_time: Instant,
    running: bool,
    otel: &OtelState,
    service_calls: &AtomicU64,
) -> LoadTestMetrics {
    let now = Instant::now();
    let current_ops = counters.ops.load(Ordering::Relaxed);
    let dt = now.duration_since(*prev_time).as_secs_f64();

    let ops_per_sec = if running && dt > 0.0 {
        (current_ops - *prev_ops) as f64 / dt
    } else if !running {
        let elapsed = now.duration_since(start_time).as_secs_f64();
        if elapsed > 0.0 {
            current_ops as f64 / elapsed
        } else {
            0.0
        }
    } else {
        0.0
    };

    *prev_ops = current_ops;
    *prev_time = now;

    let (p50, p95, p99) = compute_percentiles(latencies);

    // Collect OpenTelemetry telemetry
    let telemetry = collect_otel_metrics(&otel.provider, &otel.exporter);

    LoadTestMetrics {
        ops_per_sec,
        total_ops: current_ops,
        total_hits: counters.hits.load(Ordering::Relaxed),
        total_misses: counters.misses.load(Ordering::Relaxed),
        total_inserts: counters.inserts.load(Ordering::Relaxed),
        total_errors: counters.errors.load(Ordering::Relaxed),
        total_service_calls: service_calls.load(Ordering::Relaxed),
        p50_us: p50,
        p95_us: p95,
        p99_us: p99,
        elapsed_secs: now.duration_since(start_time).as_secs_f64(),
        running,
        telemetry,
    }
}

/// Flush and read all OpenTelemetry metrics from the in-memory exporter.
fn collect_otel_metrics(
    provider: &SdkMeterProvider,
    exporter: &InMemoryMetricExporter,
) -> Vec<TelemetryMetric> {
    // Flush to ensure latest data is available
    let _ = provider.force_flush();

    let Ok(resource_metrics) = exporter.get_finished_metrics() else {
        return Vec::new();
    };

    let mut result = Vec::new();

    // Only process the last batch — cumulative temporality means it contains the latest totals.
    // Processing all batches would sum stale cumulative snapshots.
    let Some(rm) = resource_metrics.last() else {
        return result;
    };
    for sm in rm.scope_metrics() {
        for metric in sm.metrics() {
            let name = metric.name().to_string();
            extract_data_points(&name, metric.data(), &mut result);
        }
    }

    result
}

fn attrs_to_vec(attrs: impl Iterator<Item = KeyValue>) -> Vec<(String, String)> {
    attrs
        .map(|kv| (kv.key.to_string(), kv.value.to_string()))
        .collect()
}

#[expect(clippy::too_many_lines, reason = "repetitive match arms for each OTel aggregation type")]
fn extract_data_points(
    name: &str,
    data: &AggregatedMetrics,
    out: &mut Vec<TelemetryMetric>,
) {
    match data {
        AggregatedMetrics::U64(md) => match md {
            MetricData::Sum(sum) => {
                for dp in sum.data_points() {
                    out.push(TelemetryMetric {
                        name: name.to_string(),
                        metric_type: "counter".to_string(),
                        attributes: attrs_to_vec(dp.attributes().cloned()),
                        #[expect(clippy::cast_precision_loss, reason = "counter value ok as f64")]
                        value: Some(dp.value() as f64),
                        ..TelemetryMetric::default()
                    });
                }
            }
            MetricData::Gauge(gauge) => {
                for dp in gauge.data_points() {
                    out.push(TelemetryMetric {
                        name: name.to_string(),
                        metric_type: "gauge".to_string(),
                        attributes: attrs_to_vec(dp.attributes().cloned()),
                        #[expect(clippy::cast_precision_loss, reason = "gauge value ok as f64")]
                        gauge_value: Some(dp.value() as f64),
                        ..TelemetryMetric::default()
                    });
                }
            }
            MetricData::Histogram(hist) => {
                for dp in hist.data_points() {
                    out.push(TelemetryMetric {
                        name: name.to_string(),
                        metric_type: "histogram".to_string(),
                        attributes: attrs_to_vec(dp.attributes().cloned()),
                        count: Some(dp.count()),
                        #[expect(clippy::cast_precision_loss, reason = "histogram sum ok as f64")]
                        sum: Some(dp.sum() as f64),
                        #[expect(clippy::cast_precision_loss, reason = "histogram min ok as f64")]
                        min: dp.min().map(|v| v as f64),
                        #[expect(clippy::cast_precision_loss, reason = "histogram max ok as f64")]
                        max: dp.max().map(|v| v as f64),
                        ..TelemetryMetric::default()
                    });
                }
            }
            MetricData::ExponentialHistogram(_) => {}
        },
        AggregatedMetrics::F64(md) => match md {
            MetricData::Sum(sum) => {
                for dp in sum.data_points() {
                    out.push(TelemetryMetric {
                        name: name.to_string(),
                        metric_type: "counter".to_string(),
                        attributes: attrs_to_vec(dp.attributes().cloned()),
                        value: Some(dp.value()),
                        ..TelemetryMetric::default()
                    });
                }
            }
            MetricData::Gauge(gauge) => {
                for dp in gauge.data_points() {
                    out.push(TelemetryMetric {
                        name: name.to_string(),
                        metric_type: "gauge".to_string(),
                        attributes: attrs_to_vec(dp.attributes().cloned()),
                        gauge_value: Some(dp.value()),
                        ..TelemetryMetric::default()
                    });
                }
            }
            MetricData::Histogram(hist) => {
                for dp in hist.data_points() {
                    out.push(TelemetryMetric {
                        name: name.to_string(),
                        metric_type: "histogram".to_string(),
                        attributes: attrs_to_vec(dp.attributes().cloned()),
                        count: Some(dp.count()),
                        sum: Some(dp.sum()),
                        min: dp.min(),
                        max: dp.max(),
                        ..TelemetryMetric::default()
                    });
                }
            }
            MetricData::ExponentialHistogram(_) => {}
        },
        AggregatedMetrics::I64(md) => match md {
            MetricData::Sum(sum) => {
                for dp in sum.data_points() {
                    out.push(TelemetryMetric {
                        name: name.to_string(),
                        metric_type: "counter".to_string(),
                        attributes: attrs_to_vec(dp.attributes().cloned()),
                        #[expect(clippy::cast_precision_loss, reason = "counter value ok as f64")]
                        value: Some(dp.value() as f64),
                        ..TelemetryMetric::default()
                    });
                }
            }
            MetricData::Gauge(gauge) => {
                for dp in gauge.data_points() {
                    out.push(TelemetryMetric {
                        name: name.to_string(),
                        metric_type: "gauge".to_string(),
                        attributes: attrs_to_vec(dp.attributes().cloned()),
                        #[expect(clippy::cast_precision_loss, reason = "gauge value ok as f64")]
                        gauge_value: Some(dp.value() as f64),
                        ..TelemetryMetric::default()
                    });
                }
            }
            MetricData::Histogram(hist) => {
                for dp in hist.data_points() {
                    out.push(TelemetryMetric {
                        name: name.to_string(),
                        metric_type: "histogram".to_string(),
                        attributes: attrs_to_vec(dp.attributes().cloned()),
                        count: Some(dp.count()),
                        #[expect(clippy::cast_precision_loss, reason = "histogram sum ok as f64")]
                        sum: Some(dp.sum() as f64),
                        #[expect(clippy::cast_precision_loss, reason = "histogram min ok as f64")]
                        min: dp.min().map(|v| v as f64),
                        #[expect(clippy::cast_precision_loss, reason = "histogram max ok as f64")]
                        max: dp.max().map(|v| v as f64),
                        ..TelemetryMetric::default()
                    });
                }
            }
            MetricData::ExponentialHistogram(_) => {}
        },
    }
}

type DynCache = Arc<Cache<String, String, DynamicCache<String, String>>>;

fn build_cache(
    config: &LoadTestConfig,
    conn: redis::aio::ConnectionManager,
    clock: &Clock,
    meter_provider: &SdkMeterProvider,
    service_calls: Arc<AtomicU64>,
) -> Cache<String, String, DynamicCache<String, String>> {
    let redis_cache = RedisCache::<String, String>::builder(conn)
        .key_prefix(&config.key_prefix)
        .build();

    let service = SimulatedServiceTier {
        call_count: service_calls,
    };
    let l3 = Cache::builder::<String, String>(clock.clone())
        .name("l3-service")
        .storage(service);

    if config.use_resilience {
        let context = ResilienceContext::new(clock);

        let timeout_layer = Timeout::layer("dashboard-timeout", &context)
            .timeout(Duration::from_secs(2))
            .timeout_error(|_| cachet::Error::from_message("redis operation timed out"));

        let retry_layer = Retry::layer("dashboard-retry", &context)
            .clone_input()
            .recovery_with(|res: &Result<_, _>, _| match res {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            });

        let resilient_service = retry_layer.layer(timeout_layer.layer(redis_cache));
        let mut l2 = Cache::builder::<String, String>(clock.clone())
            .name("l2-redis")
            .service(resilient_service);
        if config.redis_ttl_secs > 0 {
            l2 = l2.ttl(Duration::from_secs(config.redis_ttl_secs));
        }

        let mut l1 = Cache::builder::<String, String>(clock.clone())
            .name("l1-memory")
            .memory();
        if config.memory_ttl_secs > 0 {
            l1 = l1.ttl(Duration::from_secs(config.memory_ttl_secs));
        }

        let fb = l1
            .use_metrics(meter_provider)
            .use_logs()
            .fallback(l2)
            .promotion_policy(FallbackPromotionPolicy::always())
            .fallback(l3)
            .name("multi-tier")
            .promotion_policy(FallbackPromotionPolicy::always());
        let fb = if config.use_stampede_protection {
            fb.stampede_protection()
        } else {
            fb
        };
        fb.build().into_dynamic()
    } else {
        let mut l2 = Cache::builder::<String, String>(clock.clone())
            .name("l2-redis")
            .storage(redis_cache);
        if config.redis_ttl_secs > 0 {
            l2 = l2.ttl(Duration::from_secs(config.redis_ttl_secs));
        }

        let mut l1 = Cache::builder::<String, String>(clock.clone())
            .name("l1-memory")
            .memory();
        if config.memory_ttl_secs > 0 {
            l1 = l1.ttl(Duration::from_secs(config.memory_ttl_secs));
        }

        let fb = l1
            .use_metrics(meter_provider)
            .use_logs()
            .fallback(l2)
            .promotion_policy(FallbackPromotionPolicy::always())
            .fallback(l3)
            .name("multi-tier")
            .promotion_policy(FallbackPromotionPolicy::always());
        let fb = if config.use_stampede_protection {
            fb.stampede_protection()
        } else {
            fb
        };
        fb.build().into_dynamic()
    }
}

async fn worker_loop(
    cache: DynCache,
    config: LoadTestConfig,
    counters: Arc<Counters>,
    latencies: Arc<parking_lot::Mutex<Vec<u64>>>,
    deadline: Instant,
    stop_flag: Arc<AtomicBool>,
) {
    while Instant::now() < deadline && !stop_flag.load(Ordering::Relaxed) {
        let is_read = fastrand::f64() < config.read_ratio;
        let key_idx = fastrand::u64(0..config.key_count);
        let key = format!("{key_idx}");

        let start = Instant::now();

        if is_read {
            match cache.get(&key).await {
                Ok(Some(_)) => {
                    counters.hits.fetch_add(1, Ordering::Relaxed);
                }
                Ok(None) => {
                    counters.misses.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    counters.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        } else {
            let value = format!("value-{key_idx}");
            let entry = CacheEntry::new(value);
            match cache.insert(&key, entry).await {
                Ok(()) => {
                    counters.inserts.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    counters.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        let elapsed_us = start.elapsed().as_micros();
        counters.ops.fetch_add(1, Ordering::Relaxed);

        // Sample latencies (keep at most ~50k to avoid unbounded growth)
        let mut lat = latencies.lock();
        if lat.len() < 50_000 {
            #[expect(clippy::cast_possible_truncation, reason = "micros fits in u64")]
            lat.push(elapsed_us as u64);
        }
    }
}

fn compute_percentiles(latencies: &parking_lot::Mutex<Vec<u64>>) -> (u64, u64, u64) {
    let mut samples = latencies.lock().clone();
    if samples.is_empty() {
        return (0, 0, 0);
    }
    samples.sort_unstable();
    let len = samples.len();
    let p50 = samples[len * 50 / 100];
    let p95 = samples[len.saturating_mul(95) / 100];
    let p99 = samples[len.saturating_mul(99) / 100];
    (p50, p95, p99)
}
