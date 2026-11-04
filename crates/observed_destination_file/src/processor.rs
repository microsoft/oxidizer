// Copyright (c) Microsoft Corporation.

//! [`LogProcessor`] implementation that forwards records to the background
//! writer thread via a bounded channel.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::logs::{LogProcessor, SdkLogRecord};
use performables::Error as ChannelError;
use performables::arc::Arc;
use performables::sync::channel::{Sender, oneshot};
use performables::sync::mutex::Mutex;

use crate::messages::{ControlMessage, QueuedLog};

pub(super) const DROPPED_EVENT_WARN_INTERVAL: Duration = Duration::from_mins(1);

/// Frontend for the background writer thread, implementing `LogProcessor`.
#[derive(Debug, Clone)]
pub(super) struct BackgroundLogProcessor {
    pub(super) inner: Arc<BackgroundWriterInner>,
}

impl LogProcessor for BackgroundLogProcessor {
    fn emit(&self, data: &mut SdkLogRecord, instrumentation: &InstrumentationScope) {
        if self.inner.is_shutdown.load(Ordering::Acquire) {
            return;
        }

        let payload = QueuedLog {
            record: data.clone(),
            scope: instrumentation.clone(),
        };

        // Drop newest on full / disconnected — never block the caller.
        match self.inner.log_tx.try_send(payload) {
            Ok(()) => self.inner.wake_worker(),
            Err(_) => {
                self.inner.dropped_event_count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        if self.inner.is_shutdown.load(Ordering::Acquire) {
            return Ok(());
        }

        let (ack_tx, ack_rx) = oneshot();
        self.inner
            .send_control(ControlMessage::Flush(ack_tx))
            .map_err(|e| OTelSdkError::InternalFailure(format!("flush send failed: {e}")))?;

        flush_ack_result(ack_rx.recv_timeout(self.inner.flush_ack_timeout))
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        let started = Instant::now();
        if self.inner.is_shutdown.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        let (ack_tx, ack_rx) = oneshot();
        let sent = self.inner.send_control(ControlMessage::Shutdown(ack_tx)).is_ok();

        let (handle, worker_poisoned) = match self.inner.worker.lock_result() {
            Ok(mut worker) => (worker.take(), false),
            Err(error) => {
                let mut worker = error.into_inner();
                (worker.take(), true)
            }
        };

        let ack = ack_rx.recv_timeout(timeout.saturating_sub(started.elapsed()));
        if ack.as_ref().is_err_and(ChannelError::is_timeout) {
            // Dropping the handle detaches it; do not block past the deadline.
            return Err(OTelSdkError::Timeout(timeout));
        }
        if let Some(handle) = handle {
            let result = handle
                .join()
                .map_err(|e| OTelSdkError::InternalFailure(format!("writer thread panicked during shutdown: {e:?}")))?;
            result.map_err(OTelSdkError::InternalFailure)?;
        }

        if worker_poisoned {
            return Err(OTelSdkError::InternalFailure("worker mutex poisoned during shutdown".to_owned()));
        }

        if !sent {
            return Err(OTelSdkError::InternalFailure(
                "writer thread stopped before shutdown completed".to_owned(),
            ));
        }
        flush_ack_result(ack)
    }

    fn set_resource(&mut self, resource: &Resource) {
        let _ = self.inner.send_control(ControlMessage::SetResource(resource.clone()));
    }
}

fn flush_ack_result(result: Result<Result<(), String>, ChannelError>) -> OTelSdkResult {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(OTelSdkError::InternalFailure(err)),
        Err(error) if error.is_timeout() => Err(OTelSdkError::InternalFailure(
            "force_flush timed out waiting for writer thread ACK".to_owned(),
        )),
        Err(error) => {
            debug_assert!(error.is_closed());
            Err(OTelSdkError::InternalFailure(
                "writer thread stopped before flush completed".to_owned(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    #[cfg(not(miri))]
    use std::thread;

    use performables::sync::channel::{bounded, unbounded};

    use super::*;

    fn processor_without_worker(flush_ack_timeout: Duration) -> BackgroundLogProcessor {
        let (log_tx, _log_rx) = bounded(1);
        let (control_tx, _control_rx) = unbounded();
        let (wake_tx, _wake_rx) = bounded(1);
        let inner = Arc::new(BackgroundWriterInner {
            log_tx,
            control_tx,
            wake_tx,
            flush_ack_timeout,
            is_shutdown: AtomicBool::new(false),
            worker: Mutex::new(None),
            dropped_event_count: Arc::new(AtomicU64::new(0)),
        });

        BackgroundLogProcessor { inner }
    }

    #[test]
    fn flush_ack_timeout_result_returns_error() {
        let (_sender, receiver) = oneshot::<Result<(), String>>();
        let err = flush_ack_result(receiver.recv_timeout(Duration::ZERO)).expect_err("flush should fail");

        match err {
            OTelSdkError::InternalFailure(message) => {
                assert!(message.contains("timed out"));
            }
            other => panic!("expected timeout InternalFailure, got {other:?}"),
        }
    }

    #[test]
    fn flush_ack_preserves_writer_failure_and_detects_disconnection() {
        let error = flush_ack_result(Ok(Err("disk full".to_owned()))).unwrap_err();
        assert!(matches!(error, OTelSdkError::InternalFailure(message) if message.contains("disk full")));

        let (sender, receiver) = oneshot::<Result<(), String>>();
        drop(sender);
        let error = flush_ack_result(receiver.recv()).unwrap_err();
        assert!(matches!(error, OTelSdkError::InternalFailure(message) if message.contains("stopped")));
    }

    #[test]
    fn disconnected_writer_drops_records_and_rejects_flush() {
        use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _};
        use opentelemetry_sdk::logs::SdkLoggerProvider;

        let processor = processor_without_worker(Duration::ZERO);
        let provider = SdkLoggerProvider::builder()
            .with_resource(Resource::builder_empty().build())
            .build();
        let logger = provider.logger("disconnected-writer");
        let mut record = logger.create_log_record();
        record.set_body("discarded".into());
        processor.emit(&mut record, &InstrumentationScope::builder("test").build());
        // No concurrent worker uses the counter in this fixture.
        assert_eq!(processor.inner.dropped_event_count.load(Ordering::Relaxed), 1);
        assert!(processor.force_flush().is_err());

        assert!(processor.shutdown().is_err());
        processor.shutdown().unwrap();
        processor.force_flush().unwrap();
        processor.emit(&mut record, &InstrumentationScope::builder("test").build());
        assert_eq!(processor.inner.dropped_event_count.load(Ordering::Relaxed), 1);
    }

    #[cfg(not(miri))]
    #[test]
    fn shutdown_reports_worker_panic_after_joining() {
        let processor = processor_without_worker(Duration::ZERO);
        *processor.inner.worker.lock() = Some(thread::spawn(|| panic!("writer failed")));

        let error = processor.shutdown().unwrap_err();

        assert!(matches!(error, OTelSdkError::InternalFailure(message) if message.contains("panicked")));
        assert!(processor.inner.worker.lock().is_none());
    }

    #[test]
    fn shutdown_returns_error_when_worker_mutex_is_poisoned() {
        let processor = processor_without_worker(Duration::from_secs(5));

        let inner = Arc::clone(&processor.inner);
        let _ = catch_unwind(AssertUnwindSafe(move || {
            let _guard = inner.worker.lock();
            panic!("poison worker mutex");
        }));

        let error = processor.shutdown().expect_err("shutdown should report poisoned worker mutex");

        match error {
            OTelSdkError::InternalFailure(message) => {
                assert!(message.contains("poisoned"));
            }
            other => panic!("expected poison InternalFailure, got {other:?}"),
        }
    }
}

#[derive(Debug)]
pub(super) struct BackgroundWriterInner {
    pub(super) log_tx: Sender<QueuedLog>,
    pub(super) control_tx: Sender<ControlMessage>,
    pub(super) wake_tx: Sender<()>,
    pub(super) flush_ack_timeout: Duration,
    pub(super) is_shutdown: AtomicBool,
    pub(super) worker: Mutex<Option<JoinHandle<Result<(), String>>>>,
    pub(super) dropped_event_count: Arc<AtomicU64>,
}

impl BackgroundWriterInner {
    // Removing command delivery prevents shutdown and hangs the joining test thread.
    #[cfg_attr(test, mutants::skip)]
    fn send_control(&self, message: ControlMessage) -> Result<(), ChannelError<ControlMessage>> {
        self.control_tx.send(message)?;
        self.wake_worker();
        Ok(())
    }

    // Removing the wake strands shutdown commands while the caller joins the worker.
    #[cfg_attr(test, mutants::skip)]
    fn wake_worker(&self) {
        let _ = self.wake_tx.try_send(());
    }
}
