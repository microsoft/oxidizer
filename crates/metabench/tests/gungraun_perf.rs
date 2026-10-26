// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native perf-control callbacks through the same import as generated adapters.

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
fn control_pipes() -> std::io::PipeReader {
    use std::io::{Write as _, pipe};
    use std::os::fd::{AsRawFd as _, IntoRawFd as _};

    let mut fillers = Vec::new();
    let mut control = None;
    let mut acknowledgement = None;
    while control.is_none() || acknowledgement.is_none() {
        let (reader, writer) = pipe().unwrap();
        assert!(writer.as_raw_fd() <= gungraun::__internal::PERF_ACK_FD_READ + 1);
        if writer.as_raw_fd() == gungraun::__internal::PERF_CTL_FD_WRITE {
            control = Some((reader, writer));
        } else if reader.as_raw_fd() == gungraun::__internal::PERF_ACK_FD_READ {
            acknowledgement = Some((reader, writer));
        } else {
            fillers.push((reader, writer));
        }
    }

    let (control_reader, control_writer) = control.unwrap();
    let (acknowledgement_reader, mut acknowledgement_writer) = acknowledgement.unwrap();
    acknowledgement_writer.write_all(&[1; 9]).unwrap();
    assert_eq!(control_writer.into_raw_fd(), gungraun::__internal::PERF_CTL_FD_WRITE);
    assert_eq!(acknowledgement_reader.into_raw_fd(), gungraun::__internal::PERF_ACK_FD_READ);
    control_reader
}

#[test]
fn generated_callbacks_serialize_and_reject_reuse_after_unwinding() {
    #[cfg(target_os = "linux")]
    let mut control = control_pipes();

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

    #[cfg(target_os = "linux")]
    {
        use std::io::Read as _;

        let mut commands = [0; 128];
        let count = control.read(&mut commands).unwrap();
        assert_eq!(&commands[..count], format!("{}enable\n", "enable\ndisable\n".repeat(4)).as_bytes());
    }
}
