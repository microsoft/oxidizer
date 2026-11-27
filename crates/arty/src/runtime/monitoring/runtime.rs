// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Mutex;

use seismograph::monitor::Monitor;
use seismograph_runtime::Runtime;

use crate::runtime::Error;
use crate::runtime::config::RuntimeConfig;

#[cfg(test)]
thread_local! {
    static TEST_IO: std::cell::RefCell<Option<seismograph_protocol::io::MonitorIo>> = const { std::cell::RefCell::new(None) };
}

#[derive(Debug)]
pub(crate) struct Monitoring {
    monitor: Mutex<Option<Monitor>>,
    runtime: Runtime,
}

impl Monitoring {
    pub(crate) fn from_config(config: &RuntimeConfig, workers: usize) -> Result<Option<Self>, Error> {
        if !config.monitor_policy.is_enabled() {
            return Ok(None);
        }
        let workers = u32::try_from(workers).map_err(Error::new)?;
        let monitor_name = if config.name.as_str().is_empty() {
            "arty"
        } else {
            config.name.as_str()
        };
        let builder = Monitor::builder().name(monitor_name);
        #[cfg(test)]
        let builder = builder.io(TEST_IO
            .with_borrow(Clone::clone)
            .unwrap_or_else(|| seismograph_protocol::io::MonitorIo::new_mocked().0));
        let monitor = builder.start().map_err(Error::new)?;
        let runtime = Runtime::builder(config.name.as_str()).configured_workers(workers).build();
        Ok(Some(Self {
            monitor: Mutex::new(Some(monitor)),
            runtime,
        }))
    }

    pub(crate) const fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub(crate) fn finish(&self) -> Result<(), Error> {
        self.runtime.stop();
        let monitor = self.monitor.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        let error = monitor.as_ref().and_then(Monitor::last_error);
        drop(monitor);
        error.map_or(Ok(()), |message| Err(Error::new(MonitorFailure::new(message))))
    }

    #[cfg(test)]
    pub(crate) fn from_runtime(runtime: Runtime) -> Self {
        Self {
            monitor: Mutex::new(None),
            runtime,
        }
    }

    #[cfg(test)]
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
    use std::num::NonZeroUsize;

    use seismograph_protocol::io::{IoOperation, MonitorIo};

    use super::*;
    use crate::runtime::config::RuntimeName;
    use crate::runtime::policy::{BlockingPoolPolicy, MonitorPolicy, WorkersPolicy};

    fn config(name: &str) -> RuntimeConfig {
        RuntimeConfig {
            name: RuntimeName::new(name),
            workers_policy: WorkersPolicy::exactly(1),
            stack_size: NonZeroUsize::new(1).unwrap(),
            blocking_pool_policy: BlockingPoolPolicy::shared(),
            monitor_policy: MonitorPolicy::enabled(),
            hook_registrations: std::sync::Mutex::new(Vec::new()),
        }
    }

    #[test]
    fn monitor_start_returns_the_directory_error_without_panicking() {
        let (io, control) = MonitorIo::new_mocked();
        control.fail_next(IoOperation::CreateDirectory, std::io::Error::other("monitor directory failed"));

        let error = Monitoring::with_test_io(io, || Monitoring::from_config(&config(""), 1)).unwrap_err();

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
        let monitoring = Monitoring::with_test_io(io, || Monitoring::from_config(&config(""), 1))
            .unwrap()
            .unwrap();
        while monitoring.monitor.lock().unwrap().as_ref().unwrap().last_error().is_none() {
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
            let monitoring = Monitoring::from_config(&config(name), 1).unwrap().unwrap();

            assert_eq!(
                monitoring.monitor.lock().unwrap().as_ref().unwrap().descriptor().name,
                connection_name
            );
            monitoring.finish().unwrap();
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn an_unrepresentable_worker_count_returns_an_error() {
        Monitoring::from_config(&config(""), usize::MAX).unwrap_err();
    }
}
