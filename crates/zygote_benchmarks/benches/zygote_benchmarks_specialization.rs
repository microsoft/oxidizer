// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Child-specialization and target-setup benchmarks.

#![allow(
    missing_docs,
    clippy::missing_docs_in_private_items,
    clippy::unwrap_used,
    reason = "benchmark code needs no API documentation and should fail fast"
)]

mod support;

#[cfg(unix)]
use std::hint::black_box;
#[cfg(unix)]
use std::sync::OnceLock;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
use criterion::BenchmarkId;
use criterion::Criterion;
#[cfg(unix)]
use support::{
    CLOSE_FALLBACK_512, CLOSE_FALLBACK_4096, InputShape, SIGNALS_CUSTOM, SIGNALS_IGNORED, TRANSPARENT, accelerated_exit,
    fixture_working_directory, start,
};
#[cfg(target_os = "linux")]
use zygote_control::linux::{CommandExt as _, PrivilegePolicy};
#[cfg(unix)]
use zygote_control::unix::{CommandExt as _, Resource, ResourceLimit};
#[cfg(windows)]
use zygote_control::windows::{CommandExt as _, JobPolicy, Support};
#[cfg(unix)]
use zygote_control::{Command, Launcher, PreforkPoolConfig, Stdio, Zygote};
#[cfg(windows)]
use zygote_control::{Launcher, Stdio};

#[cfg(unix)]
const COMMAND_OPTIONS_GROUP: &str = "zygote_benchmarks_specialization/command_options";

#[cfg(unix)]
fn representative_launcher() -> &'static Launcher {
    static LAUNCHER: OnceLock<Launcher> = OnceLock::new();
    LAUNCHER.get_or_init(|| {
        let zygote = Box::leak(Box::new(start(TRANSPARENT, 1)));
        zygote.launcher()
    })
}

#[cfg(unix)]
fn configure_representative(command: &mut Command) {
    command
        .current_dir(fixture_working_directory())
        .umask(0o027)
        .resource_limit(ResourceLimit {
            resource: Resource::OpenFiles,
            soft: 1_024,
            hard: 1_024,
        });
}

#[cfg(unix)]
#[metabench::benchmark(REPRESENTATIVE_SPECIALIZATION, COMMAND_OPTIONS_GROUP, "request_to_exit/cwd_umask_limit")]
fn representative_specialization() -> i32 {
    let mut command = representative_launcher().command();
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    configure_representative(&mut command);
    let status = command.status().unwrap();
    assert!(status.success());
    status.code().unwrap()
}

#[cfg(unix)]
fn measure_acknowledgement<F>(iterations: u64, launcher: &Launcher, configure: F) -> Duration
where
    F: Fn(&mut Command),
{
    let mut elapsed = Duration::ZERO;
    for _ in 0..iterations {
        let mut command = launcher.command();
        command.env_clear().stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        configure(&mut command);
        let started = Instant::now();
        let mut child = command.spawn().unwrap();
        elapsed += started.elapsed();
        assert!(child.wait().unwrap().success());
    }
    elapsed
}

#[cfg(unix)]
fn target_setup(c: &mut Criterion) {
    let fixture_variants = [
        ("close_range", TRANSPARENT),
        ("close_fallback_512", CLOSE_FALLBACK_512),
        ("close_fallback_4096", CLOSE_FALLBACK_4096),
        ("signals_default", TRANSPARENT),
        ("signals_ignored", SIGNALS_IGNORED),
        ("signals_custom", SIGNALS_CUSTOM),
    ];
    let mut group = c.benchmark_group("zygote_benchmarks_specialization/target_setup");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));
    for (variant, program) in fixture_variants {
        let zygote = start(program, 1);
        let launcher = zygote.launcher();
        group.bench_function(BenchmarkId::new("acknowledgement", variant), |b| {
            // Child reaping is required between launches but excluded from the
            // request-to-acknowledgement duration returned to Criterion.
            b.iter_custom(|iterations| measure_acknowledgement(iterations, &launcher, |_| {}));
        });
        group.bench_function(BenchmarkId::new("request_to_exit", variant), |b| {
            b.iter(|| black_box(accelerated_exit(&launcher, InputShape::Empty)));
        });
        zygote.shutdown().unwrap();
    }
    group.finish();
}

#[cfg(unix)]
fn command_specialization(c: &mut Criterion) {
    let _representative_launcher = representative_launcher();
    let zygote = start(TRANSPARENT, 1);
    let launcher = zygote.launcher();
    let working_directory = fixture_working_directory();
    let variants = ["baseline", "cwd_umask_limit", "process_group", "new_session"];
    let mut group = c.benchmark_group(COMMAND_OPTIONS_GROUP);
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));
    for variant in variants {
        let configure = |command: &mut Command| match variant {
            "baseline" => {}
            "cwd_umask_limit" => {
                command.current_dir(&working_directory).umask(0o027).resource_limit(ResourceLimit {
                    resource: Resource::OpenFiles,
                    soft: 1_024,
                    hard: 1_024,
                });
            }
            "process_group" => {
                command.process_group(0);
            }
            "new_session" => {
                command.new_session();
            }
            _ => unreachable!(),
        };
        group.bench_function(BenchmarkId::new("acknowledgement", variant), |b| {
            b.iter_custom(|iterations| measure_acknowledgement(iterations, &launcher, configure));
        });
        group.bench_function(BenchmarkId::new("request_to_exit", variant), |b| {
            if variant == "cwd_umask_limit" {
                b.iter(|| black_box(representative_specialization()));
            } else {
                b.iter(|| {
                    let mut command = launcher.command();
                    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                    configure(&mut command);
                    let status = command.status().unwrap();
                    assert!(status.success());
                    black_box(status.code());
                });
            }
        });
    }
    group.finish();
    zygote.shutdown().unwrap();
}

#[cfg(target_os = "linux")]
fn sandbox_specialization(c: &mut Criterion) {
    let zygote = start(TRANSPARENT, 1);
    let launcher = zygote.launcher();
    let mut prefork = PreforkPoolConfig::new(1).unwrap();
    prefork.min_idle(1).unwrap();
    let mut pool_builder = Zygote::builder(TRANSPARENT);
    pool_builder.prefork_pool(prefork);
    let pooled = pool_builder.spawn().unwrap();
    let pooled_launcher = pooled.launcher();
    let policy = PrivilegePolicy::builder().no_new_privileges(true).build().unwrap();
    let mut probe = launcher.command();
    probe.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    probe.privilege_policy(policy);
    match probe.status() {
        Ok(status) if status.success() => {}
        Err(error) if matches!(error.kind(), std::io::ErrorKind::Unsupported | std::io::ErrorKind::PermissionDenied) => {
            eprintln!("SKIP no_new_privileges: {error}");
            zygote.shutdown().unwrap();
            return;
        }
        result => unreachable!("no_new_privileges probe failed unexpectedly: {result:?}"),
    }
    let mut group = c.benchmark_group("zygote_benchmarks_specialization/linux_sandbox");
    group.sample_size(10);
    for (path, measured_launcher) in [("direct_fork", &launcher), ("pool_hit", &pooled_launcher)] {
        for enabled in [false, true] {
            let label = if enabled { "no_new_privileges" } else { "baseline" };
            let configure = |command: &mut Command| {
                if enabled {
                    command.privilege_policy(policy);
                }
            };
            group.bench_function(BenchmarkId::new(format!("{path}/acknowledgement"), label), |b| {
                b.iter_custom(|iterations| measure_acknowledgement(iterations, measured_launcher, configure));
            });
            group.bench_function(BenchmarkId::new(format!("{path}/request_to_exit"), label), |b| {
                b.iter(|| {
                    let mut command = measured_launcher.command();
                    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                    configure(&mut command);
                    let status = command.status().unwrap();
                    assert!(status.success());
                    black_box(status.code());
                });
            });
        }
    }
    group.finish();
    eprintln!("pool health after sandbox samples: {:?}", pooled.health());
    zygote.shutdown().unwrap();
    pooled.shutdown().unwrap();
}

#[cfg(unix)]
fn criterion_benchmarks(c: &mut Criterion) {
    target_setup(c);
    command_specialization(c);
    #[cfg(target_os = "linux")]
    sandbox_specialization(c);
}

#[cfg(windows)]
fn representative_windows_launcher() -> &'static Launcher {
    static LAUNCHER: std::sync::OnceLock<Launcher> = std::sync::OnceLock::new();
    LAUNCHER.get_or_init(|| {
        let zygote = Box::leak(Box::new(support::start(support::TRANSPARENT, 1)));
        zygote.launcher()
    })
}

#[cfg(windows)]
#[metabench::benchmark(
    REPRESENTATIVE_SPECIALIZATION,
    "zygote_benchmarks_specialization/windows_creation",
    "inherited_overrides_16"
)]
fn representative_specialization() -> i32 {
    let mut command = representative_windows_launcher().command();
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    for index in 0..16 {
        command.env(format!("BENCH_{index:04}"), "value");
    }
    command.env("Path", "bench").env("PATH", "bench");
    let status = command.status().unwrap();
    assert!(status.success());
    status.code().unwrap()
}

#[cfg(not(any(unix, windows)))]
#[metabench::benchmark(REPRESENTATIVE_SPECIALIZATION, "zygote_benchmarks_specialization/unsupported", "unsupported")]
fn representative_specialization() -> i32 {
    panic!("Unix specialization metabench is unsupported on this platform")
}

#[cfg(windows)]
fn criterion_benchmarks(criterion: &mut Criterion) {
    let _representative_launcher = representative_windows_launcher();
    let zygote = support::start(support::TRANSPARENT, 1);
    let launcher = zygote.launcher();
    let mut group = criterion.benchmark_group("zygote_benchmarks_specialization/windows_creation");
    group.sample_size(10);
    for count in [0_usize, 16, 256, 1_024] {
        let label = format!("inherited_overrides_{count}");
        group.bench_function(label, |b| {
            if count == 16 {
                b.iter(|| std::hint::black_box(representative_specialization()));
            } else {
                b.iter(|| {
                    let mut command = launcher.command();
                    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                    for index in 0..count {
                        command.env(format!("BENCH_{index:04}"), "value");
                    }
                    if count != 0 {
                        command.env("Path", "bench").env("PATH", "bench");
                    }
                    let status = command.status().unwrap();
                    assert!(status.success());
                    std::hint::black_box(status.code())
                });
            }
        });
    }
    match Support::probe() {
        Ok(support) if support.startup_attributes => {
            let policy = JobPolicy::builder().kill_on_close(true).build().unwrap();
            match policy.probe() {
                Ok(()) => {
                    group.bench_function("job_kill_on_close", |b| {
                        b.iter(|| {
                            let mut command = launcher.command();
                            command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                            command.job(policy).unwrap();
                            let status = command.status().unwrap();
                            assert!(status.success());
                            std::hint::black_box(status.code())
                        });
                    });
                }
                Err(error) => eprintln!("SKIP job_kill_on_close: host rejected policy: {error}"),
            }
        }
        Ok(_) => eprintln!("SKIP job_kill_on_close: startup attributes unavailable"),
        Err(error) => eprintln!("SKIP job_kill_on_close: attribute probe failed: {error}"),
    }
    eprintln!("Windows samples include preparation and process creation; no separate CreateProcess/token attribution is available");
    group.finish();
    zygote.shutdown().unwrap();
}

#[cfg(not(any(unix, windows)))]
fn criterion_benchmarks(_criterion: &mut Criterion) {}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [REPRESENTATIVE_SPECIALIZATION]);
