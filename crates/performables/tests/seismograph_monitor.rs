// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native end-to-end telemetry workload for the monitor TUI.

// Native IPC runs here; portable primitive and recorder tests retain Miri coverage.
#![cfg(all(feature = "seismograph", not(miri), not(loom)))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

#[cfg(test)]
mod tests {
    use std::io::{self, Write as _};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use performables::arc::Arc;
    use performables::sync::mutex::Mutex;
    use seismograph::monitor::Monitor;
    use seismograph::recorder::event::EventKind;
    use seismograph::recorder::{Configuration, EventBufferCapacity, RecordingPolicy};
    use seismograph_protocol::io::MonitorIo;
    use seismograph_protocol::message::{Request, Response, SnapshotOptions};
    use seismograph_protocol::transport::Connection;
    use seismograph_protocol::{read_response, write_request};

    const NAME: &str = "performables-integration";
    const WORKERS: usize = 3;
    // Deliberately hold the mutex briefly so the TUI also shows contention.
    const HOLD_LOCK: Duration = Duration::from_millis(2);
    const PAUSE: Duration = Duration::from_millis(10);
    // The recorder is process-global, including when ignored tests are selected.
    static SESSION: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct StopOnExit<'a>(&'a AtomicBool);

    impl Drop for StopOnExit<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    fn with_workload(instance: &str, inspect: impl FnOnce(&Monitor)) {
        let _session = SESSION.lock().unwrap();
        let monitor = Monitor::builder().name(NAME).instance(instance).start().unwrap();
        seismograph::recorder(Configuration {
            general_events: RecordingPolicy::all(true),
            arc_dereferences: RecordingPolicy::all(true),
            event_capacity_per_thread: EventBufferCapacity::new(4_096).unwrap(),
            ..Default::default()
        });
        let stop = AtomicBool::new(false);
        let counter = Mutex::<u64>::new(0);
        let payload = Arc::new(vec![7_u8; 256]);
        thread::scope(|scope| {
            let _stop_on_exit = StopOnExit(&stop);
            let (ready, started) = mpsc::channel();
            for worker in 0..WORKERS {
                let (stop, counter, payload) = (&stop, &counter, &payload);
                let mut ready = Some(ready.clone());
                thread::Builder::new()
                    .name(format!("monitor-worker-{worker}"))
                    .spawn_scoped(scope, move || {
                        while !stop.load(Ordering::Acquire) {
                            let copy = Arc::clone(payload);
                            {
                                let mut count = counter.lock();
                                *count += 1;
                                std::hint::black_box(copy[worker]);
                                thread::sleep(HOLD_LOCK);
                            }
                            drop(copy);
                            if let Some(ready) = ready.take() {
                                ready.send(()).unwrap();
                            }
                            thread::sleep(PAUSE);
                        }
                    })
                    .unwrap();
            }
            drop(ready);
            for _ in 0..WORKERS {
                started.recv_timeout(Duration::from_secs(10)).unwrap();
            }
            inspect(&monitor);
        });
        drop(payload);
        drop(monitor);
        seismograph::recorder(Configuration::default());
    }

    #[test]
    #[ignore = "interactive workload: run with --ignored --nocapture; press Enter to stop"]
    fn live_monitor() {
        with_workload("manual", |monitor| {
            println!(
                "\n{name} is ready (PID {pid}).\nEndpoint: {endpoint}\n\
                 In another terminal: cargo +1.95.0 run -p seismograph_cli -- monitor\n\
                 Select {name}, press Enter, then s; inspect Primitives and Threads.\n\
                 Press Enter HERE to stop the workload.\n",
                name = NAME,
                pid = monitor.descriptor().process_id,
                endpoint = monitor.descriptor().endpoint,
            );
            io::stdout().flush().unwrap();
            io::stdin().read_line(&mut String::new()).unwrap();
        });
    }

    #[test]
    fn records_real_primitive_data_over_native_transport() {
        with_workload("smoke", |monitor| {
            let io = MonitorIo::new_native();
            let descriptor = monitor.descriptor();
            let discovered = seismograph_protocol::discovery::directories(&io)
                .unwrap()
                .filter_map(Result::ok)
                .flat_map(|directory| match directory.descriptors() {
                    Ok(entries) => entries.collect::<Vec<_>>(),
                    Err(error) => {
                        eprintln!("ignoring unavailable registry {}: {error}", directory.path().display());
                        Vec::new()
                    }
                })
                .any(|candidate| candidate.as_ref().is_ok_and(|candidate| candidate == descriptor));
            assert!(discovered, "the same discovery path used by the CLI must find the workload");
            let mut stream = Connection::connect(&io, descriptor, Duration::from_secs(2)).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
            write_request(&mut stream, 1, &Request::Hello).unwrap();
            assert!(
                matches!(read_response(&mut stream).unwrap(), (1, Response::Hello { instance_id, .. }) if instance_id == descriptor.instance_id)
            );
            write_request(&mut stream, 2, &Request::ReadRecorderActivity).unwrap();
            let (2, Response::RecorderActivity(activity)) = read_response(&mut stream).unwrap() else {
                panic!("the monitor must return live activity");
            };
            for worker in 0..WORKERS {
                assert!(
                    activity
                        .threads
                        .iter()
                        .any(|thread| thread.name == format!("monitor-worker-{worker}"))
                );
            }
            write_request(&mut stream, 3, &Request::CaptureSnapshot(SnapshotOptions::default())).unwrap();
            let (3, Response::Snapshot(bytes)) = read_response(&mut stream).unwrap() else {
                panic!("the monitor must return a native snapshot");
            };
            let snapshot = seismograph::snapshot::decode(&bytes).unwrap();
            for kind in [
                EventKind::MutexAccess,
                EventKind::MutexRelease,
                EventKind::ArcClone,
                EventKind::ArcDeref,
            ] {
                assert!(
                    snapshot.events.events.iter().any(|event| event.kind == kind),
                    "missing {kind:?} telemetry"
                );
            }
        });
    }
}
