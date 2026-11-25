// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph::monitor::Monitor;
use seismograph_runtime::{RuntimeHandle, RuntimeMetadata, RuntimeRegistration};

use crate::runtime::Error;
use crate::runtime::config::RuntimeName;
use crate::runtime::policy::MonitorPolicy;

#[cfg(any(test, feature = "test-util"))]
thread_local! {
    static TEST_IO: std::cell::RefCell<Option<seismograph_protocol::io::MonitorIo>> = const { std::cell::RefCell::new(None) };
}

#[derive(Debug)]
pub(crate) struct RuntimeMonitoring {
    monitor: Monitor,
    registration: RuntimeRegistration,
}

impl RuntimeMonitoring {
    pub(crate) fn start(policy: &MonitorPolicy, name: &RuntimeName, workers: usize) -> Result<Option<Self>, Error> {
        if !policy.is_enabled() {
            return Ok(None);
        }
        let workers = u32::try_from(workers).map_err(Error::new)?;
        let monitor_name = if name.as_str().is_empty() { "arty" } else { name.as_str() };
        let builder = Monitor::builder().name(monitor_name);
        #[cfg(any(test, feature = "test-util"))]
        let builder = builder.io(TEST_IO
            .with_borrow(Clone::clone)
            .unwrap_or_else(|| seismograph_protocol::io::MonitorIo::new_mocked().0));
        let monitor = builder.start().map_err(Error::new)?;
        let registration = seismograph_runtime::register_runtime(RuntimeMetadata::new(name.as_str(), workers));
        Ok(Some(Self { monitor, registration }))
    }

    pub(crate) fn handle(&self) -> RuntimeHandle {
        self.registration.handle()
    }

    pub(crate) fn finish(self) -> Result<(), Error> {
        // #[gamma::skip(stmt.delete_call, reason = "Dropping self below also stops the registration through RuntimeRegistration::drop.")]
        self.registration.stopped();
        let error = self.monitor.last_error();
        drop(self);
        error.map_or(Ok(()), |message| Err(Error::new(MonitorFailure::new(message))))
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn with_test_io<T>(io: seismograph_protocol::io::MonitorIo, run: impl FnOnce() -> T) -> T {
        struct Restore(Option<seismograph_protocol::io::MonitorIo>);
        impl Drop for Restore {
            fn drop(&mut self) {
                TEST_IO.set(self.0.take());
            }
        }
        let _restore = Restore(TEST_IO.replace(Some(io)));
        run()
    }
}

#[ohno::error]
#[display("the runtime monitor failed: {message}")]
struct MonitorFailure {
    message: String,
}

#[cfg(all(test, not(miri)))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use seismograph_protocol::io::{IoOperation, MonitorIo};

    use super::*;

    #[test]
    fn monitor_start_returns_the_directory_error_without_panicking() {
        let (io, control) = MonitorIo::new_mocked();
        control.fail_next(IoOperation::CreateDirectory, std::io::Error::other("monitor directory failed"));

        let error = RuntimeMonitoring::with_test_io(io, || {
            RuntimeMonitoring::start(&MonitorPolicy::enabled(), &RuntimeName::default(), 1)
        })
        .unwrap_err();

        let mut source: &dyn std::error::Error = &error;
        while let Some(inner) = source.source() {
            source = inner;
        }
        assert_eq!(source.to_string(), "monitor directory failed");
    }

    #[test]
    fn finishing_reports_a_background_monitor_failure() {
        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let (io, control) = MonitorIo::new_mocked();
        control.fail_next(IoOperation::Accept, std::io::Error::other("monitor listener failed"));
        let monitoring = RuntimeMonitoring::with_test_io(io, || {
            RuntimeMonitoring::start(&MonitorPolicy::enabled(), &RuntimeName::default(), 1)
        })
        .unwrap()
        .unwrap();
        while monitoring.monitor.last_error().is_none() {
            std::thread::yield_now();
        }

        let error = monitoring.finish().unwrap_err();

        let mut source: &dyn std::error::Error = &error;
        while let Some(inner) = source.source() {
            source = inner;
        }
        assert_eq!(source.to_string(), "the runtime monitor failed: monitor listener failed");
    }

    #[test]
    fn monitor_connections_keep_named_runtimes_and_supply_an_empty_name_fallback() {
        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        for (name, connection_name) in [("requests", "requests"), ("", "arty")] {
            let monitoring = RuntimeMonitoring::start(&MonitorPolicy::enabled(), &RuntimeName::new(name), 1)
                .unwrap()
                .unwrap();

            assert_eq!(monitoring.monitor.descriptor().name, connection_name);
            monitoring.finish().unwrap();
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn an_unrepresentable_worker_count_returns_an_error() {
        RuntimeMonitoring::start(&MonitorPolicy::enabled(), &RuntimeName::default(), usize::MAX).unwrap_err();
    }
}
