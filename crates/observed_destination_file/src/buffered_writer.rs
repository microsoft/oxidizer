// Copyright (c) Microsoft Corporation.

//! Buffering for sinks whose failed writes may have committed an unknown prefix.

use std::io::{self, BufWriter, Write};
use std::mem;

pub(crate) struct BufferedWriter {
    inner: BufWriter<Box<dyn Write + Send>>,
}

impl BufferedWriter {
    pub(crate) fn with_capacity(capacity: usize, writer: Box<dyn Write + Send>) -> Self {
        Self {
            inner: BufWriter::with_capacity(capacity, writer),
        }
    }

    pub(crate) fn buffer(&self) -> &[u8] {
        self.inner.buffer()
    }

    fn discard_failed_batch(&mut self) {
        // FileRotate's BytesSurpassed path uses write_all internally: Err can
        // mean a prefix reached disk without being included in its byte count.
        // Retrying that batch can duplicate bytes. Prefer losing its remainder.
        let capacity = self.inner.capacity();
        let failed = mem::replace(&mut self.inner, BufWriter::with_capacity(0, Box::new(io::sink())));
        let (writer, _) = failed.into_parts();
        self.inner = BufWriter::with_capacity(capacity, writer);
    }
}

impl Write for BufferedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let result = self.inner.write(buf);
        if result.is_err() {
            self.discard_failed_batch();
        }
        result
    }

    fn flush(&mut self) -> io::Result<()> {
        let result = self.inner.flush();
        if result.is_err() {
            // After a downstream flush-only failure the buffer is already empty.
            self.discard_failed_batch();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use performables::arc::Arc;
    use performables::sync::mutex::Mutex;

    use super::*;

    #[derive(Default, Clone, Copy, PartialEq, Eq)]
    enum WriteFailure {
        #[default]
        None,
        Partial,
        PartialAfterShort,
        Zero,
        Interrupted,
    }

    #[derive(Default)]
    struct State {
        bytes: Vec<u8>,
        failure: WriteFailure,
        fail_flush: bool,
        short_write: bool,
        writes: usize,
        flushes: usize,
    }

    struct Writer(Arc<Mutex<State>>);

    // The fixture models both normal short writes and indeterminate partial failures.
    impl Write for Writer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut state = self.0.lock();
            state.writes += 1;
            if state.failure == WriteFailure::Interrupted {
                state.failure = WriteFailure::None;
                return Err(io::ErrorKind::Interrupted.into());
            }
            if state.failure == WriteFailure::Zero {
                return Ok(0);
            }
            if state.failure == WriteFailure::PartialAfterShort {
                state.failure = WriteFailure::Partial;
                state.bytes.extend_from_slice(&buf[..2]);
                return Ok(2);
            }
            if state.failure == WriteFailure::Partial {
                state.failure = WriteFailure::None;
                state.bytes.extend_from_slice(&buf[..2]);
                return Err(io::Error::other("partial batch"));
            }
            let count = if state.short_write { buf.len().min(2) } else { buf.len() };
            state.bytes.extend_from_slice(&buf[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            let mut state = self.0.lock();
            state.flushes += 1;
            if mem::take(&mut state.fail_flush) {
                Err(io::Error::other("flush only"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn indeterminate_failed_batches_are_not_replayed_on_flush_capacity_or_drop() {
        for mode in 0..3 {
            let state = Arc::new(Mutex::new(State {
                failure: WriteFailure::Partial,
                ..State::default()
            }));
            let mut writer = BufferedWriter::with_capacity(8, Box::new(Writer(Arc::clone(&state))));
            writer.write_all(b"abcdef").unwrap();
            match mode {
                0 => assert!(writer.flush().is_err()),
                1 => assert!(writer.write_all(b"next").is_err()),
                _ => {
                    drop(writer);
                    assert_eq!(state.lock().bytes, b"ab");
                    assert_eq!(state.lock().writes, 1);
                    continue;
                }
            }
            assert!(writer.buffer().is_empty());
            writer.write_all(b"later").unwrap();
            writer.flush().unwrap();
            drop(writer);
            assert_eq!(state.lock().bytes, b"ablater");
            assert_eq!(state.lock().writes, 2);
        }
    }

    #[test]
    fn direct_failed_writes_are_not_replayed() {
        for capacity in [0, 4] {
            let state = Arc::new(Mutex::new(State {
                failure: WriteFailure::Partial,
                ..State::default()
            }));
            let mut writer = BufferedWriter::with_capacity(capacity, Box::new(Writer(Arc::clone(&state))));
            assert!(writer.write_all(b"abcdef").is_err());
            writer.flush().unwrap();
            drop(writer);
            assert_eq!(state.lock().bytes, b"ab");
            assert_eq!(state.lock().writes, 1);
        }
    }

    #[test]
    fn short_and_interrupted_writes_preserve_bytes_and_zero_write_discards_batch() {
        let state = Arc::new(Mutex::new(State {
            short_write: true,
            failure: WriteFailure::Interrupted,
            ..State::default()
        }));
        let mut writer = BufferedWriter::with_capacity(8, Box::new(Writer(Arc::clone(&state))));
        writer.write_all(b"abcdef").unwrap();
        writer.flush().unwrap();
        assert_eq!(state.lock().bytes, b"abcdef");
        state.lock().failure = WriteFailure::Zero;
        writer.write_all(b"lost").unwrap();
        assert_eq!(writer.flush().unwrap_err().kind(), io::ErrorKind::WriteZero);
        assert!(writer.buffer().is_empty());
        state.lock().failure = WriteFailure::None;
        writer.flush().unwrap();
        assert_eq!(state.lock().bytes, b"abcdef");
    }

    #[test]
    fn acknowledged_short_prefix_and_indeterminate_prefix_are_not_replayed() {
        let state = Arc::new(Mutex::new(State {
            failure: WriteFailure::PartialAfterShort,
            ..State::default()
        }));
        let mut writer = BufferedWriter::with_capacity(8, Box::new(Writer(Arc::clone(&state))));
        writer.write_all(b"abcdef").unwrap();
        assert!(writer.flush().is_err());
        assert!(writer.buffer().is_empty());
        writer.flush().unwrap();
        drop(writer);
        assert_eq!(state.lock().bytes, b"abcd");
        assert_eq!(state.lock().writes, 2);
    }

    #[test]
    fn flush_only_failure_does_not_replay_written_bytes_or_discard_later_records() {
        let state = Arc::new(Mutex::new(State {
            fail_flush: true,
            ..State::default()
        }));
        let mut writer = BufferedWriter::with_capacity(8, Box::new(Writer(Arc::clone(&state))));
        writer.write_all(b"abcdef").unwrap();
        assert!(writer.flush().is_err());
        assert!(writer.buffer().is_empty());
        writer.write_all(b"later").unwrap();
        writer.flush().unwrap();
        assert_eq!(state.lock().bytes, b"abcdeflater");
        assert_eq!(state.lock().writes, 2);
        assert_eq!(state.lock().flushes, 2);
    }

    struct PanicWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for PanicWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().extend_from_slice(&buf[..2]);
            panic!("indeterminate write panic");
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn unwinding_does_not_retry_a_panicked_write() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let output_clone = Arc::clone(&output);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let mut writer = BufferedWriter::with_capacity(8, Box::new(PanicWriter(output_clone)));
            writer.write_all(b"abcdef").unwrap();
            writer.flush().unwrap();
        }));
        assert!(result.is_err());
        assert_eq!(*output.lock(), b"ab");
    }
}
