// Copyright (c) Microsoft Corporation.

//! Background writer thread that drains a bounded log channel and writes to a
//! [`Write`] sink. Shared by both the file and rolling-file destinations.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use observed_destination_core::{DestinationResult, LogFormatConfig, ResourceHeader, Rfc3339Nanos, print_log_record};
use performables::Error as ChannelError;
use performables::arc::Arc;
use performables::sync::channel::{OneshotSender, Receiver, bounded, unbounded};
use performables::sync::mutex::Mutex;
use tick::SimpleClock;

use crate::FileLogInitError;
use crate::buffered_writer::BufferedWriter;
use crate::messages::{ControlMessage, QueuedLog};
use crate::processor::{BackgroundLogProcessor, BackgroundWriterInner, DROPPED_EVENT_WARN_INTERVAL};

pub(crate) type WriterOpener = Arc<dyn Fn(&Path) -> std::io::Result<Box<dyn Write + Send>> + Send + Sync>;

/// Default number of log records buffered before the channel starts dropping.
pub(crate) const DEFAULT_QUEUE_CAPACITY: usize = 4_096;

/// Default capacity of the worker's reusable byte buffer.
/// Initial tunable policy: coalesce small writes at a bounded per-worker memory cost,
/// not a measured optimum.
pub(crate) const DEFAULT_BUFFER_SIZE: usize = 256 * 1024;

/// Default interval between automatic flushes of pending bytes.
/// Initial tunable policy balancing write coalescing against output visibility,
/// not a measured optimum.
pub(crate) const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// Default maximum time `force_flush` waits for the writer thread to acknowledge
/// a flush request.
pub(crate) const DEFAULT_FLUSH_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(1);

const MAX_LOG_DRAIN_PER_TICK: usize = 256;

/// The automatic flush interval must be nonzero.
#[ohno::error]
pub(crate) struct InvalidFlushInterval;

/// Opens the writer eagerly and spawns the background worker thread.
pub(crate) fn background_procesor(
    format_config: LogFormatConfig,
    opener: &WriterOpener,
    path: &Path,
    queue_capacity: usize,
    buffer_size: usize,
    flush_interval: Duration,
    flush_ack_timeout: std::time::Duration,
) -> DestinationResult<BackgroundLogProcessor> {
    if flush_interval.is_zero() {
        return Err(FileLogInitError::caused_by(InvalidFlushInterval::new()).into());
    }
    let writer = opener(path).map_err(FileLogInitError::caused_by)?;
    let writer = BufferedWriter::with_capacity(buffer_size, writer);

    let (log_tx, log_rx) = bounded(queue_capacity);
    let (control_tx, control_rx) = unbounded();
    let (wake_tx, wake_rx) = bounded(1);
    let dropped_event_count = Arc::new(AtomicU64::new(0));

    let worker_dropped_event_count = Arc::clone(&dropped_event_count);
    let worker_handle = thread::spawn(move || {
        let worker = Worker::new(
            writer,
            log_rx,
            control_rx,
            wake_rx,
            format_config,
            worker_dropped_event_count,
            flush_interval,
        );
        worker.run()
    });

    let inner = Arc::new(BackgroundWriterInner {
        log_tx,
        control_tx,
        wake_tx,
        flush_ack_timeout,
        is_shutdown: AtomicBool::new(false),
        worker: Mutex::new(Some(worker_handle)),
        dropped_event_count,
    });

    Ok(BackgroundLogProcessor { inner })
}

struct Worker {
    log_rx: Receiver<QueuedLog>,
    control_rx: Receiver<ControlMessage>,
    wake_rx: Receiver<()>,
    format_config: LogFormatConfig,
    writer: BufferedWriter,
    clock: SimpleClock,
    shutdown_ack: Option<OneshotSender<Result<(), String>>>,
    flush_interval: Duration,
    // Scheduled/explicit flush attempt time, not successful byte transfer time.
    // Capacity-triggered writes do not restart the automatic cadence.
    last_flush: Instant,
    resource_header: ResourceHeader,
    first_write_error: Option<String>,
    dropped_event_count: Arc<AtomicU64>,
    next_dropped_event_check: Instant,
    last_dropped_event_check: SystemTime,
}

impl Worker {
    /// The registration boundary validates that `flush_interval` is nonzero.
    fn new(
        writer: BufferedWriter,
        log_rx: Receiver<QueuedLog>,
        control_rx: Receiver<ControlMessage>,
        wake_rx: Receiver<()>,
        format_config: LogFormatConfig,
        dropped_event_count: Arc<AtomicU64>,
        flush_interval: Duration,
    ) -> Self {
        let clock = SimpleClock::new_system();
        Self {
            log_rx,
            control_rx,
            wake_rx,
            format_config,
            writer,
            flush_interval,
            last_flush: clock.instant(),
            resource_header: ResourceHeader::new(),
            first_write_error: None,
            dropped_event_count,
            next_dropped_event_check: clock.instant() + DROPPED_EVENT_WARN_INTERVAL,
            last_dropped_event_check: clock.system_time(),
            clock,
            shutdown_ack: None,
        }
    }

    fn run(mut self) -> Result<(), String> {
        self.serve(Receiver::recv_timeout);
        self.report_dropped_events(SystemTime::now());
        let result = self.flush_pending();
        let ack = self.shutdown_ack.take();
        // Finish all writer I/O, including destruction, before acknowledging.
        drop(self);
        if let Some(ack) = ack {
            let _ = ack.send(result.clone());
        }
        result
    }

    /// Drain logs and handle control messages until shutdown.
    // Mutating loop termination can prevent shutdown and hang the joining test thread.
    #[cfg_attr(test, mutants::skip)]
    fn serve(&mut self, mut receive_wake: impl FnMut(&Receiver<()>, Duration) -> Result<(), ChannelError>) {
        loop {
            if self.process_control_messages() {
                return;
            }

            let processed_log = match self.log_rx.try_recv() {
                Ok(log) => {
                    self.write_log(&log);
                    // Drain a bounded batch before checking control messages again
                    // so control stays responsive under sustained logging.
                    self.drain_ready_logs(MAX_LOG_DRAIN_PER_TICK);
                    true
                }
                Err(error) if error.is_empty() => false,
                Err(error) => {
                    debug_assert!(error.is_closed());
                    return;
                }
            };

            self.report_dropped_events_if_due();
            self.flush_if_due();

            if !processed_log && !self.wait_for_wake(&mut receive_wake) {
                return;
            }
        }
    }

    // Mutating wake/disconnect handling can leave the worker spinning indefinitely.
    #[cfg_attr(test, mutants::skip)]
    fn wait_for_wake(&mut self, receive_wake: &mut impl FnMut(&Receiver<()>, Duration) -> Result<(), ChannelError>) -> bool {
        let timeout = self.wait_timeout(self.clock.instant());
        match receive_wake(&self.wake_rx, timeout) {
            Ok(()) => true,
            Err(error) if error.is_timeout() => {
                self.flush_if_due();
                true
            }
            Err(error) => {
                debug_assert!(error.is_closed());
                false
            }
        }
    }

    fn wait_timeout(&self, now: Instant) -> Duration {
        let mut timeout = self.next_dropped_event_check.saturating_duration_since(now);
        if !self.writer.buffer().is_empty() {
            timeout = timeout.min(self.flush_interval.saturating_sub(now.duration_since(self.last_flush)));
        }
        timeout
    }

    // Mutating shutdown handling can leave the worker or its joining thread blocked.
    #[cfg_attr(test, mutants::skip)]
    fn process_control_messages(&mut self) -> bool {
        loop {
            match self.control_rx.try_recv() {
                Ok(ControlMessage::Shutdown(ack)) => {
                    self.shutdown_ack = Some(ack);
                    return true;
                }
                Ok(ControlMessage::Flush(ack)) => {
                    let _ = ack.send(self.flush_pending());
                }
                Ok(ControlMessage::SetResource(resource)) => {
                    self.resource_header.set_resource(&resource);
                }
                Err(error) if error.is_empty() => return false,
                Err(error) => {
                    debug_assert!(error.is_closed());
                    return true;
                }
            }
        }
    }

    fn report_dropped_events_if_due(&mut self) {
        let now = self.clock.instant();
        if now >= self.next_dropped_event_check {
            self.report_dropped_events(SystemTime::now());
            self.next_dropped_event_check = now + DROPPED_EVENT_WARN_INTERVAL;
        }
    }

    fn flush_if_due(&mut self) {
        let now = self.clock.instant();
        if now.duration_since(self.last_flush) < self.flush_interval {
            return;
        }
        // Advance even after failures or empty ticks, avoiding idle flushes and busy retries.
        self.last_flush = now;
        if !self.writer.buffer().is_empty()
            && let Err(err) = self.writer.flush()
        {
            self.record_write_error(err.to_string());
        }
    }

    fn drain_ready_logs(&mut self, max_logs: usize) {
        for _ in 0..max_logs {
            let Ok(log) = self.log_rx.try_recv() else {
                break;
            };
            self.write_log(&log);
        }
    }

    fn write_log(&mut self, log: &QueuedLog) {
        if let Err(err) = self.write_log_result(log) {
            self.record_write_error(err);
        }
    }

    fn write_log_result(&mut self, log: &QueuedLog) -> Result<(), String> {
        self.resource_header.print_once(&mut self.writer).map_err(|err| err.to_string())?;
        print_log_record(&mut self.writer, &log.record, &log.scope, self.format_config).map_err(|err| err.to_string())
    }

    fn record_write_error(&mut self, err: String) {
        if self.first_write_error.is_none() {
            self.first_write_error = Some(err);
        }
    }

    fn report_dropped_events(&mut self, now: SystemTime) {
        if let Some(message) = self.take_dropped_event_warning(now)
            && let Err(err) = self.write_dropped_event_warning(now, &message)
        {
            self.record_write_error(err);
        }
    }

    fn take_dropped_event_warning(&mut self, now: SystemTime) -> Option<String> {
        let count = self.dropped_event_count.swap(0, Ordering::AcqRel);
        let since = self.last_dropped_event_check;
        self.last_dropped_event_check = now;
        (count != 0).then(|| dropped_event_warning(count, since))
    }

    fn write_dropped_event_warning(&mut self, now: SystemTime, message: &str) -> Result<(), String> {
        self.resource_header.print_once(&mut self.writer).map_err(|err| err.to_string())?;
        write_warning_line(&mut self.writer, now, message).map_err(|err| err.to_string())
    }

    /// Drains any buffered records and flushes the underlying writer.
    fn flush_pending(&mut self) -> Result<(), String> {
        let mut first_error = self.first_write_error.take();
        while let Ok(log) = self.log_rx.try_recv() {
            match self.write_log_result(&log) {
                Err(err) if first_error.is_none() => first_error = Some(err),
                _ => {}
            }
        }
        match self.writer.flush().map_err(|err| err.to_string()) {
            Err(err) if first_error.is_none() => first_error = Some(err),
            _ => {}
        }
        self.last_flush = self.clock.instant();

        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

fn dropped_event_warning(count: u64, since: SystemTime) -> String {
    format!("dropped {count} events since {}", Rfc3339Nanos::from(since))
}

fn write_warning_line(writer: &mut (impl Write + ?Sized), timestamp: SystemTime, message: &str) -> std::io::Result<()> {
    // Fixed nanosecond precision keeps the timestamp width consistent across
    // every line (see `Rfc3339Nanos`).
    writeln!(writer, "[{}] WARN: {message}", Rfc3339Nanos::from(timestamp))
}

#[cfg(test)]
#[cfg(not(miri))]
mod tests {
    use std::{io, str};

    use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _};
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::logs::{LogProcessor as _, SdkLoggerProvider};

    use super::*;

    struct FailWriter;

    impl Write for FailWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("write failed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failed"))
        }
    }

    #[derive(Clone)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.lock().flush()
        }
    }

    fn queued_log(body: &'static str) -> QueuedLog {
        let scope = opentelemetry::InstrumentationScope::builder("test-lib").build();
        let provider = SdkLoggerProvider::builder().build();
        let logger = provider.logger("test-lib");
        let mut record = logger.create_log_record();
        record.set_body(opentelemetry::logs::AnyValue::String(body.into()));

        QueuedLog { record, scope }
    }

    fn worker_with_writer(writer: Box<dyn Write + Send>, log_rx: Receiver<QueuedLog>) -> Worker {
        worker_with_buffer(writer, log_rx, 0)
    }

    fn worker_with_buffer(writer: Box<dyn Write + Send>, log_rx: Receiver<QueuedLog>, buffer_size: usize) -> Worker {
        let (_control_tx, control_rx) = unbounded();
        let (_wake_tx, wake_rx) = bounded(1);
        Worker::new(
            BufferedWriter::with_capacity(buffer_size, writer),
            log_rx,
            control_rx,
            wake_rx,
            LogFormatConfig::minimal(),
            Arc::new(AtomicU64::new(0)),
            DEFAULT_FLUSH_INTERVAL,
        )
    }

    #[derive(Default)]
    struct WriteStats {
        bytes: Vec<u8>,
        writes: usize,
        flushes: usize,
    }

    struct CountingWriter(Arc<Mutex<WriteStats>>);

    impl Write for CountingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut stats = self.0.lock();
            stats.writes += 1;
            stats.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.lock().flushes += 1;
            Ok(())
        }
    }

    #[test]
    fn idle_worker_flushes_without_another_record() {
        let stats = Arc::new(Mutex::new(WriteStats::default()));
        let (log_tx, log_rx) = bounded(1);
        let (control_tx, control_rx) = unbounded();
        let (wake_tx, wake_rx) = bounded(1);
        let mut worker = worker_with_buffer(Box::new(CountingWriter(Arc::clone(&stats))), log_rx, DEFAULT_BUFFER_SIZE);
        worker.resource_header.set_resource(&Resource::builder_empty().build());
        worker.control_rx = control_rx;
        worker.wake_rx = wake_rx;
        let control = tick::ClockControl::new();
        worker.clock = control.to_simple_clock();
        worker.last_flush = worker.clock.instant();
        worker.next_dropped_event_check = worker.last_flush + DROPPED_EVENT_WARN_INTERVAL;
        let mut processor = Some(BackgroundLogProcessor {
            inner: Arc::new(BackgroundWriterInner {
                log_tx,
                control_tx,
                wake_tx,
                flush_ack_timeout: DEFAULT_FLUSH_ACK_TIMEOUT,
                is_shutdown: AtomicBool::new(false),
                worker: Mutex::new(None),
                dropped_event_count: Arc::clone(&worker.dropped_event_count),
            }),
        });
        let mut waits = 0;

        // Drive the real serve loop synchronously. Only the blocking receive is
        // scripted; no explicit flush, shutdown, or later record can expose bytes.
        worker.serve(|receiver, timeout| {
            waits += 1;
            match waits {
                1 => {
                    assert_eq!(timeout, DROPPED_EVENT_WARN_INTERVAL);
                    let QueuedLog { mut record, scope } = queued_log("idle record");
                    processor.as_ref().unwrap().emit(&mut record, &scope);
                    let wake = receiver.try_recv();
                    assert!(wake.is_ok(), "emitting a record must wake the worker");
                    wake
                }
                2 => {
                    let stats = stats.lock();
                    assert!(stats.bytes.is_empty());
                    assert_eq!(stats.flushes, 0);
                    assert_eq!(timeout, DEFAULT_FLUSH_INTERVAL);
                    control.advance(timeout.checked_sub(Duration::from_nanos(1)).unwrap());
                    receiver.recv_timeout(Duration::ZERO)
                }
                3 => {
                    // An early timeout must neither flush early nor lose the deadline.
                    let stats = stats.lock();
                    assert!(stats.bytes.is_empty());
                    assert_eq!(stats.flushes, 0);
                    assert_eq!(timeout, Duration::from_nanos(1));
                    control.advance(timeout);
                    receiver.recv_timeout(Duration::ZERO)
                }
                4 => {
                    let stats = stats.lock();
                    assert_eq!(str::from_utf8(&stats.bytes).unwrap().matches("idle record").count(), 1);
                    assert_eq!(stats.writes, 1);
                    assert_eq!(stats.flushes, 1);
                    assert_eq!(timeout, DROPPED_EVENT_WARN_INTERVAL.checked_sub(DEFAULT_FLUSH_INTERVAL).unwrap());
                    drop(processor.take());
                    receiver.recv_timeout(Duration::ZERO)
                }
                _ => panic!("worker failed to return to a clean idle wait"),
            }
        });
        assert_eq!(waits, 4);
    }

    #[test]
    fn idle_wait_deadlines_and_timeout_flush_use_the_controlled_clock() {
        let (_log_tx, log_rx) = unbounded();
        let stats = Arc::new(Mutex::new(WriteStats::default()));
        let mut worker = worker_with_buffer(Box::new(CountingWriter(Arc::clone(&stats))), log_rx, DEFAULT_BUFFER_SIZE);
        let (_wake_tx, wake_rx) = bounded(1);
        worker.wake_rx = wake_rx;
        let control = tick::ClockControl::new();
        worker.clock = control.to_simple_clock();
        worker.last_flush = worker.clock.instant();
        worker.next_dropped_event_check = worker.last_flush + worker.flush_interval * 3;

        assert_eq!(worker.wait_timeout(worker.clock.instant()), worker.flush_interval * 3);
        worker.write_log(&queued_log("idle record"));
        assert_eq!(worker.wait_timeout(worker.clock.instant()), worker.flush_interval);
        control.advance(worker.flush_interval.checked_sub(Duration::from_nanos(1)).unwrap());
        assert_eq!(worker.wait_timeout(worker.clock.instant()), Duration::from_nanos(1));
        worker.flush_if_due();
        assert_eq!(stats.lock().flushes, 0);
        control.advance(Duration::from_nanos(1));
        assert_eq!(worker.wait_timeout(worker.clock.instant()), Duration::ZERO);
        // A zero-duration receive exercises the actual idle timeout path without sleeping.
        assert!(worker.wait_for_wake(&mut Receiver::recv_timeout));
        assert_eq!(stats.lock().flushes, 1);
        assert_eq!(worker.last_flush, worker.clock.instant());
        assert!(String::from_utf8(stats.lock().bytes.clone()).unwrap().contains("idle record"));
        assert_eq!(worker.wait_timeout(worker.clock.instant()), worker.flush_interval * 2);
        worker.write_log(&queued_log("overdue record"));
        control.advance(worker.flush_interval + Duration::from_nanos(1));
        assert_eq!(worker.wait_timeout(worker.clock.instant()), Duration::ZERO);
        assert!(worker.wait_for_wake(&mut Receiver::recv_timeout));
        assert_eq!(stats.lock().flushes, 2);

        worker.write_log(&queued_log("report deadline is earlier"));
        worker.next_dropped_event_check = worker.clock.instant() + Duration::from_nanos(1);
        assert_eq!(worker.wait_timeout(worker.clock.instant()), Duration::from_nanos(1));
        control.advance(Duration::from_nanos(2));
        assert_eq!(worker.wait_timeout(worker.clock.instant()), Duration::ZERO);
    }

    #[test]
    fn capacity_writes_keep_cadence_and_explicit_flush_restarts_it() {
        let (_log_tx, log_rx) = unbounded();
        let stats = Arc::new(Mutex::new(WriteStats::default()));
        let mut worker = worker_with_buffer(Box::new(CountingWriter(Arc::clone(&stats))), log_rx, 64);
        let control = tick::ClockControl::new();
        worker.clock = control.to_simple_clock();
        worker.last_flush = worker.clock.instant();
        let origin = worker.last_flush;
        control.advance(worker.flush_interval / 2);
        worker.writer.write_all(&[b'x'; 64]).unwrap();
        assert_eq!(stats.lock().writes, 1);
        assert_eq!(worker.last_flush, origin);
        worker.writer.write_all(b"pending").unwrap();
        worker.flush_pending().unwrap();
        assert_eq!(worker.last_flush, worker.clock.instant());
        worker.writer.write_all(b"after explicit flush").unwrap();
        control.advance(worker.flush_interval.checked_sub(Duration::from_nanos(1)).unwrap());
        worker.flush_if_due();
        assert_eq!(stats.lock().flushes, 1);
        control.advance(Duration::from_nanos(1));
        worker.flush_if_due();
        assert_eq!(stats.lock().flushes, 2);
    }

    #[test]
    fn automatic_flush_keeps_deadline_and_skips_empty_buffers() {
        let (_log_tx, log_rx) = unbounded();
        let stats = Arc::new(Mutex::new(WriteStats::default()));
        let mut worker = worker_with_buffer(Box::new(CountingWriter(Arc::clone(&stats))), log_rx, DEFAULT_BUFFER_SIZE);
        let control = tick::ClockControl::new();
        worker.clock = control.to_simple_clock();
        worker.last_flush = worker.clock.instant();
        let origin = worker.last_flush;
        worker.write_log(&queued_log("first record"));
        control.advance(worker.flush_interval.checked_sub(Duration::from_nanos(1)).unwrap());
        worker.flush_if_due();
        assert_eq!(worker.last_flush, origin);
        assert!(stats.lock().bytes.is_empty());

        control.advance(Duration::from_nanos(1));
        worker.write_log(&queued_log("second record"));
        worker.flush_if_due();
        assert_eq!(worker.last_flush, origin + worker.flush_interval);
        let output = String::from_utf8(stats.lock().bytes.clone()).unwrap();
        assert!(output.contains("first record"));
        assert!(output.contains("second record"));
        assert_eq!(stats.lock().flushes, 1);

        // At the next deadline an empty buffer advances the schedule without I/O.
        control.advance(worker.flush_interval);
        worker.flush_if_due();
        assert_eq!(stats.lock().flushes, 1);
        let empty_tick = worker.last_flush;
        worker.write_log(&queued_log("third record"));
        control.advance(Duration::from_nanos(1));
        worker.flush_if_due();
        assert_eq!(worker.last_flush, empty_tick);
        assert_eq!(stats.lock().flushes, 1);
        control.advance(worker.flush_interval);
        worker.flush_if_due();
        assert_eq!(stats.lock().flushes, 2);
        assert!(String::from_utf8(stats.lock().bytes.clone()).unwrap().contains("third record"));
    }

    #[test]
    fn automatic_flush_reports_buffered_write_failure() {
        let (_log_tx, log_rx) = unbounded();
        let mut worker = worker_with_buffer(Box::new(FailWriter), log_rx, DEFAULT_BUFFER_SIZE);
        worker.write_log(&queued_log("pending record"));
        let control = tick::ClockControl::new();
        worker.clock = control.to_simple_clock();
        worker.last_flush = worker.clock.instant();
        control.advance(worker.flush_interval);

        worker.flush_if_due();

        assert!(worker.first_write_error.as_ref().unwrap().contains("write failed"));
        assert_eq!(worker.last_flush, worker.clock.instant());
        assert!(worker.flush_pending().unwrap_err().contains("write failed"));
    }

    struct TransientFlushFailWriter(bool);

    impl Write for TransientFlushFailWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.0 {
                self.0 = false;
                Err(io::Error::other("temporary flush failure"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn automatic_flush_error_is_reported_even_after_writer_recovers() {
        let (_log_tx, log_rx) = unbounded();
        let mut worker = worker_with_buffer(Box::new(TransientFlushFailWriter(true)), log_rx, DEFAULT_BUFFER_SIZE);
        worker.write_log(&queued_log("pending record"));
        let control = tick::ClockControl::new();
        worker.clock = control.to_simple_clock();
        worker.last_flush = worker.clock.instant();
        control.advance(worker.flush_interval);

        worker.flush_if_due();

        assert!(worker.flush_pending().unwrap_err().contains("temporary flush failure"));
        worker.flush_pending().unwrap();
    }

    struct PartialFailWriter {
        output: Arc<Mutex<Vec<u8>>>,
        fail: bool,
    }

    impl Write for PartialFailWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.fail {
                self.fail = false;
                self.output.lock().extend_from_slice(&buf[..2]);
                Err(io::Error::other("indeterminate prefix"))
            } else {
                self.output.lock().extend_from_slice(buf);
                Ok(buf.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn automatic_flush_discards_failed_batch_and_reports_it_once() {
        let (_log_tx, log_rx) = unbounded();
        let output = Arc::new(Mutex::new(Vec::new()));
        let mut worker = worker_with_buffer(
            Box::new(PartialFailWriter {
                output: Arc::clone(&output),
                fail: true,
            }),
            log_rx,
            DEFAULT_BUFFER_SIZE,
        );
        worker.write_log(&queued_log("lost record"));
        worker.last_flush = worker.clock.instant().checked_sub(worker.flush_interval).unwrap();
        worker.flush_if_due();
        assert!(worker.writer.buffer().is_empty());
        assert_eq!(output.lock().len(), 2);
        worker.write_log(&queued_log("later record"));
        assert!(worker.flush_pending().unwrap_err().contains("indeterminate prefix"));
        worker.flush_pending().unwrap();
        let text = String::from_utf8(output.lock().clone()).unwrap();
        assert!(!text.contains("lost record"));
        assert_eq!(text.matches("later record").count(), 1);
    }

    fn processor_with_writer(writer: Box<dyn Write + Send>, buffer_size: usize) -> BackgroundLogProcessor {
        let writer = Arc::new(Mutex::<_>::new(Some(writer)));
        let opener: WriterOpener = Arc::new(move |_: &Path| Ok(writer.lock().take().unwrap()))
            .coerce::<dyn Fn(&Path) -> io::Result<Box<dyn Write + Send>> + Send + Sync>(|arc| arc);
        background_procesor(
            LogFormatConfig::minimal(),
            &opener,
            Path::new("unused"),
            DEFAULT_QUEUE_CAPACITY,
            buffer_size,
            DEFAULT_FLUSH_INTERVAL,
            DEFAULT_FLUSH_ACK_TIMEOUT,
        )
        .unwrap()
    }

    #[test]
    fn shutdown_returns_final_write_flush_and_remembered_errors() {
        use opentelemetry_sdk::logs::LogProcessor as _;
        for (writer, buffer_size, error) in [
            (Box::new(FailWriter) as Box<dyn Write + Send>, DEFAULT_BUFFER_SIZE, "write failed"),
            (Box::new(FlushFailWriter), DEFAULT_BUFFER_SIZE, "flush failed"),
            (Box::new(FailWriter), 0, "write failed"),
        ] {
            let processor = processor_with_writer(writer, buffer_size);
            processor.inner.log_tx.send(queued_log("final record")).unwrap();
            processor.inner.wake_tx.try_send(()).unwrap();
            let result = processor.shutdown_with_timeout(Duration::from_secs(10)).unwrap_err();
            assert!(result.to_string().contains(error));
            processor.shutdown().unwrap();
            processor.force_flush().unwrap();
            assert!(processor.inner.worker.lock().is_none());
        }
    }

    struct BlockingWriter {
        entered: performables::sync::channel::Sender<()>,
        release: Receiver<()>,
    }

    impl Write for BlockingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn shutdown_timeout_does_not_join_blocked_writer_and_repeated_calls_are_noops() {
        use opentelemetry_sdk::error::OTelSdkError;
        use opentelemetry_sdk::logs::LogProcessor as _;
        let (entered_tx, entered_rx) = bounded(1);
        let (release_tx, release_rx) = bounded(1);
        let processor = processor_with_writer(
            Box::new(BlockingWriter {
                entered: entered_tx,
                release: release_rx,
            }),
            DEFAULT_BUFFER_SIZE,
        );
        processor.inner.log_tx.send(queued_log("final record")).unwrap();
        assert!(matches!(
            processor.shutdown_with_timeout(Duration::from_millis(20)),
            Err(OTelSdkError::Timeout(_))
        ));
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        processor.shutdown().unwrap();
        processor.force_flush().unwrap();
        release_tx.send(()).unwrap();
    }

    #[test]
    fn worker_exit_reports_a_remembered_error_even_after_recovery() {
        let (_log_tx, log_rx) = unbounded();
        let mut worker = worker_with_buffer(Box::new(io::sink()), log_rx, DEFAULT_BUFFER_SIZE);
        worker.record_write_error("stored error".to_owned());
        worker.write_log(&queued_log("later record"));
        assert!(worker.run().unwrap_err().contains("stored error"));
    }

    #[test]
    fn buffering_coalesces_writes_without_changing_output() {
        let mut log = queued_log("buffered record");
        log.record.set_timestamp(SystemTime::UNIX_EPOCH);
        let mut expected_output = None;
        // Contrast bypass, small batches, and a buffer that holds the entire workload.
        for buffer_size in [0, 64, DEFAULT_BUFFER_SIZE] {
            let (_log_tx, log_rx) = unbounded();
            let stats = Arc::new(Mutex::new(WriteStats::default()));
            let mut worker = worker_with_buffer(Box::new(CountingWriter(Arc::clone(&stats))), log_rx, buffer_size);
            worker.resource_header.set_resource(&Resource::builder_empty().build());
            for _ in 0..1024 {
                worker.write_log(&log);
            }

            if buffer_size == DEFAULT_BUFFER_SIZE {
                assert!(stats.lock().bytes.is_empty());
            } else {
                assert!(!stats.lock().bytes.is_empty());
            }

            worker.flush_pending().unwrap();
            let stats = stats.lock();
            if buffer_size == DEFAULT_BUFFER_SIZE {
                assert_eq!(stats.writes, 1);
            } else if buffer_size == 0 {
                assert!(stats.writes >= 1024);
            } else {
                assert!(stats.writes > 1);
            }
            if let Some(expected) = &expected_output {
                assert_eq!(&stats.bytes, expected);
            } else {
                expected_output = Some(stats.bytes.clone());
            }
        }
    }

    #[test]
    fn buffered_write_failure_is_reported_by_flush() {
        let (_log_tx, log_rx) = unbounded();
        let mut worker = worker_with_buffer(Box::new(FailWriter), log_rx, DEFAULT_BUFFER_SIZE);

        worker.write_log(&queued_log("buffered record"));

        assert!(worker.first_write_error.is_none());
        assert!(worker.flush_pending().unwrap_err().contains("write failed"));
    }

    #[test]
    fn direct_write_failure_is_recorded() {
        let (_log_tx, log_rx) = unbounded();
        let mut worker = worker_with_buffer(Box::new(FailWriter), log_rx, 0);

        worker.write_log(&queued_log("record"));

        if let Err(err) = worker.flush_pending() {
            assert_eq!(err, "Operation failed: write failed");
        } else {
            panic!("expected a write error to be returned");
        }
    }

    #[test]
    fn closing_any_input_still_drains_pending_logs() {
        for input in 0..3 {
            let (log_tx, log_rx) = unbounded();
            let (control_tx, control_rx) = unbounded();
            let (wake_tx, wake_rx) = bounded(1);
            let output = Arc::new(Mutex::new(Vec::new()));
            let worker = Worker::new(
                BufferedWriter::with_capacity(DEFAULT_BUFFER_SIZE, Box::new(SharedWriter(Arc::clone(&output)))),
                log_rx,
                control_rx,
                wake_rx,
                LogFormatConfig::minimal(),
                Arc::new(AtomicU64::new(0)),
                DEFAULT_FLUSH_INTERVAL,
            );
            log_tx.send(queued_log("pending record")).unwrap();
            let _senders = match input {
                0 => {
                    drop(log_tx);
                    (None, Some(control_tx), Some(wake_tx))
                }
                1 => {
                    drop(control_tx);
                    (Some(log_tx), None, Some(wake_tx))
                }
                _ => {
                    drop(wake_tx);
                    (Some(log_tx), Some(control_tx), None)
                }
            };

            worker.run().unwrap();

            assert!(String::from_utf8(output.lock().clone()).unwrap().contains("pending record"));
        }
    }

    #[test]
    fn due_drop_warning_write_failure_is_reported_by_flush() {
        let (_log_tx, log_rx) = unbounded();
        let mut worker = worker_with_writer(Box::new(FailWriter), log_rx);
        // This fixture has no concurrent producers.
        worker.dropped_event_count.store(1, Ordering::Relaxed);
        worker.next_dropped_event_check = Instant::now();
        worker.report_dropped_events_if_due();

        assert!(worker.flush_pending().unwrap_err().contains("write failed"));
        assert!(worker.next_dropped_event_check > Instant::now());
        worker.dropped_event_count.store(2, Ordering::Relaxed);
        worker.report_dropped_events_if_due();
        assert_eq!(worker.dropped_event_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn wake_timeout_keeps_worker_alive_until_channel_closes() {
        let (_log_tx, log_rx) = unbounded();
        let mut worker = worker_with_writer(Box::new(io::sink()), log_rx);
        let (wake_tx, wake_rx) = bounded(1);
        worker.wake_rx = wake_rx;
        worker.next_dropped_event_check = Instant::now();

        assert!(worker.wait_for_wake(&mut Receiver::recv_timeout));
        wake_tx.send(()).unwrap();
        assert!(worker.wait_for_wake(&mut Receiver::recv_timeout));
        drop(wake_tx);
        assert!(!worker.wait_for_wake(&mut Receiver::recv_timeout));
    }

    #[test]
    fn ready_backlog_does_not_wait_for_another_wake() {
        let (log_tx, log_rx) = unbounded();
        let (_control_tx, control_rx) = unbounded();
        let output = Arc::new(Mutex::new(Vec::new()));
        let mut worker = worker_with_writer(Box::new(SharedWriter(Arc::clone(&output))), log_rx);
        worker.control_rx = control_rx;
        let log_count = MAX_LOG_DRAIN_PER_TICK + 2;
        for _ in 0..log_count {
            log_tx.send(queued_log("backlog record")).unwrap();
        }
        drop(log_tx);

        // The wake input is closed, but already queued logs must not need another wake.
        worker.serve(Receiver::recv_timeout);
        worker.flush_pending().unwrap();

        assert_eq!(
            String::from_utf8(output.lock().clone()).unwrap().matches("backlog record").count(),
            log_count
        );
    }

    #[test]
    fn buffered_body_and_warning_write_failures_are_reported_by_flush() {
        for warning in [false, true] {
            let (log_tx, log_rx) = unbounded();
            let mut worker = worker_with_writer(Box::new(FailWriter), log_rx);
            worker.resource_header.print_once(&mut io::sink()).unwrap();
            if warning {
                worker.dropped_event_count.store(1, Ordering::Relaxed);
                worker.report_dropped_events(SystemTime::now());
            } else {
                log_tx.send(queued_log("buffered record")).unwrap();
            }

            assert!(worker.flush_pending().unwrap_err().contains("write failed"));
        }
    }

    struct FlushFailWriter;

    impl Write for FlushFailWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failed"))
        }
    }

    #[test]
    fn flush_reports_writer_flush_failure() {
        let (_log_tx, log_rx) = unbounded();
        let mut worker = worker_with_writer(Box::new(FlushFailWriter), log_rx);

        assert!(worker.flush_pending().unwrap_err().contains("flush failed"));
    }

    #[test]
    fn drain_ready_logs_stops_at_configured_batch_size() {
        let (log_tx, log_rx) = unbounded();
        for _ in 0..=MAX_LOG_DRAIN_PER_TICK {
            log_tx.send(queued_log("hello")).unwrap();
        }

        let mut worker = worker_with_writer(Box::new(Vec::new()), log_rx);
        worker.drain_ready_logs(MAX_LOG_DRAIN_PER_TICK);

        assert_eq!(worker.log_rx.len(), 1);
    }

    #[test]
    fn flush_pending_reports_previous_async_write_error() {
        let (log_tx, log_rx) = unbounded();
        let mut worker = worker_with_writer(Box::new(FailWriter), log_rx);

        worker.record_write_error("previous async write failed".to_owned());
        log_tx.send(queued_log("pending record")).unwrap();

        let err = worker.flush_pending().expect_err("flush should report write error");
        assert!(err.contains("previous async write failed"));
    }

    #[test]
    fn dropped_event_warning_takes_counter_since_last_check() {
        let (_log_tx, log_rx) = unbounded();
        let dropped_event_count = Arc::new(AtomicU64::new(3));
        let mut worker = Worker::new(
            BufferedWriter::with_capacity(DEFAULT_BUFFER_SIZE, Box::new(Vec::new())),
            log_rx,
            unbounded().1,
            bounded(1).1,
            LogFormatConfig::minimal(),
            Arc::clone(&dropped_event_count),
            DEFAULT_FLUSH_INTERVAL,
        );
        let first_check = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10);
        let second_check = first_check + std::time::Duration::from_mins(1);
        worker.last_dropped_event_check = first_check;

        assert_eq!(
            worker.take_dropped_event_warning(second_check),
            Some("dropped 3 events since 1970-01-01T00:00:10.000000000Z".to_owned())
        );
        assert_eq!(dropped_event_count.load(Ordering::Relaxed), 0);
        assert_eq!(worker.last_dropped_event_check, second_check);
        assert_eq!(worker.take_dropped_event_warning(second_check), None);
    }

    #[test]
    fn dropped_event_warning_is_written_to_log_file() {
        let (_log_tx, log_rx) = unbounded();
        let writer = Arc::new(Mutex::new(Vec::new()));
        let mut worker = Worker::new(
            BufferedWriter::with_capacity(DEFAULT_BUFFER_SIZE, Box::new(SharedWriter(Arc::clone(&writer)))),
            log_rx,
            unbounded().1,
            bounded(1).1,
            LogFormatConfig::minimal(),
            Arc::new(AtomicU64::new(0)),
            DEFAULT_FLUSH_INTERVAL,
        );
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(70);

        worker
            .write_dropped_event_warning(now, "dropped 3 events since 1970-01-01T00:00:10.000000000Z")
            .unwrap();
        worker.flush_pending().unwrap();

        let output = String::from_utf8(writer.lock().clone()).unwrap();
        assert!(output.starts_with("<Resource: "));
        assert!(output.ends_with("[1970-01-01T00:01:10.000000000Z] WARN: dropped 3 events since 1970-01-01T00:00:10.000000000Z\n"));
    }
}
