// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native perf-control callbacks through the same import as generated adapters.

#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test helpers report unexpected results as test failures"
)]

use std::panic::catch_unwind;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use gungraun::__internal::{InternalBenchRunMode, InternalLibFunctionKind};

static ACTIVE_WORKLOAD: AtomicBool = AtomicBool::new(false);
static PANIC_IN_WORKLOAD: AtomicBool = AtomicBool::new(false);
static COMPLETED: AtomicUsize = AtomicUsize::new(0);

mod native {
    use metabench::__private::gungraun;

    use super::*;

    #[gungraun::library_benchmark]
    fn native_case() {
        assert!(!PANIC_IN_WORKLOAD.load(Ordering::SeqCst), "workload failed");
        assert!(!ACTIVE_WORKLOAD.swap(true, Ordering::SeqCst));
        thread::yield_now();
        COMPLETED.fetch_add(1, Ordering::SeqCst);
        ACTIVE_WORKLOAD.store(false, Ordering::SeqCst);
    }
}

#[cfg(target_os = "linux")]
fn duplicate_fd_at_least(source: &impl std::os::fd::AsRawFd, minimum: std::os::fd::RawFd) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{FromRawFd as _, OwnedFd};

    // SAFETY: fcntl duplicates a live descriptor without changing its owner.
    let duplicate = unsafe { libc::fcntl(source.as_raw_fd(), libc::F_DUPFD_CLOEXEC, minimum) };
    if duplicate == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fcntl returned a new descriptor, which has no other Rust owner.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

#[cfg(target_os = "linux")]
fn control_pipes() -> (std::io::PipeReader, std::os::fd::OwnedFd, std::os::fd::OwnedFd) {
    use std::io::{Write as _, pipe};

    let (control_reader, control_writer) = pipe().unwrap();
    let (acknowledgement_reader, mut acknowledgement_writer) = pipe().unwrap();
    let minimum = gungraun::__internal::PERF_CTL_FD_WRITE.max(gungraun::__internal::PERF_ACK_FD_READ) + 1;
    acknowledgement_writer.write_all(&[1; 9]).unwrap();
    (
        control_reader,
        duplicate_fd_at_least(&control_writer, minimum).unwrap(),
        duplicate_fd_at_least(&acknowledgement_reader, minimum).unwrap(),
    )
}

#[cfg(target_os = "linux")]
/// # Safety
///
/// Sources must be live descriptors above both reserved targets. No Rust owner
/// can access a previous target after this call. Gungraun owns the replacements.
unsafe fn install_control_fds(control: std::os::fd::RawFd, acknowledgement: std::os::fd::RawFd) -> std::io::Result<()> {
    for (source, target) in [
        (control, gungraun::__internal::PERF_CTL_FD_WRITE),
        (acknowledgement, gungraun::__internal::PERF_ACK_FD_READ),
    ] {
        // SAFETY: The caller isolates these targets from live Rust owners.
        // Both sources are above the targets, so neither replacement closes the other source.
        if unsafe { libc::dup2(source, target) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn assert_control_commands(mut control: std::io::PipeReader) {
    use std::io::Read as _;

    let mut commands = [0; 128];
    let count = control.read(&mut commands).unwrap();
    assert_eq!(&commands[..count], format!("{}enable\n", "enable\ndisable\n".repeat(4)).as_bytes());
}

#[cfg(target_os = "linux")]
fn run_with_isolated_control_fds() {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::process::CommandExt as _;
    use std::process::Command;

    let (reader, control, acknowledgement) = control_pipes();
    // Keep free target slots occupied so Command cannot use them for its exec-error pipe.
    // An occupied target stays unchanged; fcntl then reserves a higher descriptor instead.
    let reservations = [
        duplicate_fd_at_least(&control, gungraun::__internal::PERF_CTL_FD_WRITE).unwrap(),
        duplicate_fd_at_least(&control, gungraun::__internal::PERF_ACK_FD_READ).unwrap(),
    ];
    let control_fd = control.as_raw_fd();
    let acknowledgement_fd = acknowledgement.as_raw_fd();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "generated_callbacks_serialize_and_reject_reuse_after_unwinding",
            "--nocapture",
        ])
        .env("METABENCH_TEST_GUNGRAUN_PERF_CHILD", "1");
    // SAFETY: Only async-signal-safe dup2 calls run after fork. The source owners
    // stay alive through output(), and exec discards any previous target owners.
    unsafe {
        command.pre_exec(move || {
            // Occupy both reserved slots with the wrong endpoints before remapping them.
            install_control_fds(acknowledgement_fd, control_fd)?;
            install_control_fds(control_fd, acknowledgement_fd)
        });
    }
    let output = command.output().unwrap();
    drop((control, acknowledgement, reservations));
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_control_commands(reader);
}

#[test]
fn generated_callbacks_serialize_and_reject_reuse_after_unwinding() {
    #[cfg(target_os = "linux")]
    if !cfg!(miri) && std::env::var_os("METABENCH_TEST_GUNGRAUN_PERF_CHILD").is_none() {
        run_with_isolated_control_fds();
        return;
    }

    #[cfg(all(target_os = "linux", miri))]
    let (reader, _control, _acknowledgement) = {
        use std::os::fd::AsRawFd as _;

        let pipes = control_pipes();
        // SAFETY: This isolated Miri process has no Rust owners for the reserved
        // targets. Gungraun takes ownership when the first callback starts.
        unsafe {
            install_control_fds(pipes.2.as_raw_fd(), pipes.1.as_raw_fd()).unwrap();
            install_control_fds(pipes.1.as_raw_fd(), pipes.2.as_raw_fd()).unwrap();
        }
        pipes
    };

    exercise_generated_callbacks();

    #[cfg(all(target_os = "linux", miri))]
    assert_control_commands(reader);
}

fn exercise_generated_callbacks() {
    let InternalLibFunctionKind::Default(run) = native::native_case::__BENCHES[0].func else {
        panic!("expected a single native case");
    };
    let start = Barrier::new(2);
    thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                start.wait();
                for _ in 0..2 {
                    metabench::__private::run_gungraun_perf(InternalBenchRunMode::PerfOnce, || run(InternalBenchRunMode::PerfOnce));
                }
            });
        }
    });
    assert_eq!(COMPLETED.load(Ordering::SeqCst), 4);

    metabench::__private::run_gungraun_perf(InternalBenchRunMode::PerfOnce, || {
        metabench::__private::run_gungraun_perf(InternalBenchRunMode::Default, || {});
        let entered = AtomicBool::new(false);
        catch_unwind(|| {
            metabench::__private::run_gungraun_perf(InternalBenchRunMode::PerfOnce, || entered.store(true, Ordering::SeqCst));
        })
        .unwrap_err();
        assert!(!entered.load(Ordering::SeqCst));
    });

    PANIC_IN_WORKLOAD.store(true, Ordering::SeqCst);
    catch_unwind(|| {
        metabench::__private::run_gungraun_perf(InternalBenchRunMode::PerfOnce, || run(InternalBenchRunMode::PerfOnce));
    })
    .unwrap_err();
    let entered = AtomicBool::new(false);
    catch_unwind(|| {
        metabench::__private::run_gungraun_perf(InternalBenchRunMode::PerfOnce, || entered.store(true, Ordering::SeqCst));
    })
    .unwrap_err();
    assert!(!entered.load(Ordering::SeqCst));
    PANIC_IN_WORKLOAD.store(false, Ordering::SeqCst);
    metabench::__private::run_gungraun_perf(InternalBenchRunMode::Default, || run(InternalBenchRunMode::Default));
    assert_eq!(COMPLETED.load(Ordering::SeqCst), 5);
}
