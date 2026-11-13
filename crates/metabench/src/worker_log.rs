// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read as _};
#[cfg(unix)]
use std::os::fd::AsRawFd as _;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle as _;
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::ptr;

#[cfg(windows)]
use windows_sys::Win32::Foundation::ERROR_BROKEN_PIPE;
#[cfg(windows)]
use windows_sys::Win32::System::Pipes::PeekNamedPipe;

#[cfg(not(any(unix, windows)))]
use self::prepare_reader_unsupported as prepare_reader;
#[cfg(not(any(unix, windows)))]
use self::read_available_unsupported as read_available;
#[cfg(windows)]
use self::read_available_windows as read_available;
#[cfg(unix)]
use self::{prepare_reader_unix as prepare_reader, read_available_unix as read_available};

// Conservative diagnostic headroom without unbounded per-worker memory or storage.
pub(crate) const LOG_LIMIT: usize = 1_048_576;
const TRUNCATION: &[u8] = b"[metabench: output truncated; only the final bytes are retained]\n";
const TAIL_LIMIT: usize = LOG_LIMIT - TRUNCATION.len();
// Consume at most a quarter of the retained log before checking the worker deadline again.
const DRAIN_BUDGET: usize = 262_144;

/// Owns a merged output pipe and a bounded raw tail, without a reader thread.
pub(crate) struct WorkerLog {
    pub(crate) file: LogFile,
    reader: LogReader,
    output: OutputTail,
}

pub(crate) enum LogFile {
    Native(File),
    #[cfg(test)]
    Memory(std::rc::Rc<std::cell::RefCell<io::Cursor<Vec<u8>>>>),
    #[cfg(test)]
    Scripted(Box<dyn LogStorage>),
}

#[cfg(test)]
pub(crate) trait LogStorage: io::Write + io::Seek {}
#[cfg(test)]
impl<T: io::Write + io::Seek> LogStorage for T {}

enum LogReader {
    Native(io::PipeReader),
    #[cfg(test)]
    Memory(io::Cursor<Vec<u8>>),
}

impl io::Write for LogFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Native(file) => file.write(buf),
            #[cfg(test)]
            Self::Memory(file) => file.borrow_mut().write(buf),
            #[cfg(test)]
            Self::Scripted(file) => file.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Native(file) => file.flush(),
            #[cfg(test)]
            Self::Memory(file) => file.borrow_mut().flush(),
            #[cfg(test)]
            Self::Scripted(file) => file.flush(),
        }
    }
}

impl io::Seek for LogFile {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        match self {
            Self::Native(file) => file.seek(pos),
            #[cfg(test)]
            Self::Memory(file) => file.borrow_mut().seek(pos),
            #[cfg(test)]
            Self::Scripted(file) => file.seek(pos),
        }
    }
}

#[cfg(test)]
impl LogFile {
    pub(crate) fn native_file(&self) -> &File {
        match self {
            Self::Native(file) => file,
            Self::Memory(_) | Self::Scripted(_) => panic!("a native fixture must own its native file"),
        }
    }
}

fn prepare_output_pipe<R, W>(
    open: impl FnOnce() -> io::Result<(R, W)>,
    prepare: impl FnOnce(&R) -> io::Result<()>,
    clone: impl FnOnce(&W) -> io::Result<W>,
    attach: impl FnOnce(W, W),
) -> io::Result<R> {
    let (reader, writer) = open()?;
    prepare(&reader)?;
    let stdout = clone(&writer)?;
    attach(stdout, writer);
    Ok(reader)
}

struct OutputTail {
    tail: VecDeque<u8>,
    truncated: bool,
    eof: bool,
}

impl OutputTail {
    fn new() -> Self {
        Self {
            tail: VecDeque::with_capacity(TAIL_LIMIT),
            truncated: false,
            eof: false,
        }
    }

    fn drain_with(&mut self, mut read: impl FnMut(&mut [u8]) -> io::Result<usize>) -> io::Result<(bool, bool)> {
        if self.eof {
            return Ok((true, false));
        }
        // #[gamma::skip(literal.int_increment, reason = "Read initializes exactly the returned prefix; drain_with only retains that prefix and ignores initial buffer contents")]
        let mut bytes = [0; 8_192];
        let mut had_output = false;
        for _ in 0..DRAIN_BUDGET / bytes.len() {
            match read(&mut bytes) {
                Ok(0) => {
                    self.eof = true;
                    return Ok((true, had_output));
                }
                Ok(count) => {
                    had_output = true;
                    self.retain(&bytes[..count]);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok((false, had_output)),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok((false, had_output))
    }

    fn retain(&mut self, bytes: &[u8]) {
        let excess = (self.tail.len() + bytes.len()).saturating_sub(TAIL_LIMIT);
        if excess != 0 {
            self.tail.drain(..excess);
            self.truncated = true;
        }
        self.tail.extend(bytes);
    }

    fn excerpt(&self) -> Vec<u8> {
        self.tail.iter().skip(self.tail.len().saturating_sub(8_192)).copied().collect()
    }
}

impl WorkerLog {
    pub(crate) fn new(file: File, command: &mut Command) -> io::Result<Self> {
        // #[gamma::skip(try.propagate_to_unwrap, reason = "this adapter constructs native pipe ends; prepare_output_pipe error and ownership policy is tested with scripted owned values without pipes or subprocesses")]
        let reader = prepare_output_pipe(
            io::pipe,
            |reader| {
                #[cfg(not(windows))]
                prepare_reader(reader)?;
                #[cfg(windows)]
                let _ = reader;
                Ok(())
            },
            io::PipeWriter::try_clone,
            |stdout, stderr| {
                command.stdout(stdout).stderr(stderr);
            },
        )?;
        Ok(Self {
            file: LogFile::Native(file),
            reader: LogReader::Native(reader),
            output: OutputTail::new(),
        })
    }

    #[cfg(test)]
    pub(crate) fn in_memory(bytes: Vec<u8>, file: std::rc::Rc<std::cell::RefCell<io::Cursor<Vec<u8>>>>) -> Self {
        Self {
            file: LogFile::Memory(file),
            reader: LogReader::Memory(io::Cursor::new(bytes)),
            output: OutputTail::new(),
        }
    }

    pub(crate) fn release_command_writers(command: &mut Command) {
        // Command retains its Stdio handles after spawning; they would prevent EOF.
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    }

    /// Bounds work per poll so continuous output cannot starve the worker deadline.
    ///
    /// Returns whether EOF was reached and whether any bytes were consumed.
    pub(crate) fn drain(&mut self) -> io::Result<(bool, bool)> {
        self.output.drain_with(|bytes| match &mut self.reader {
            LogReader::Native(reader) => read_available(reader, bytes),
            #[cfg(test)]
            LogReader::Memory(reader) => reader.read(bytes),
        })
    }

    pub(crate) fn persist(&mut self) -> io::Result<()> {
        Self::persist_output(&self.output, &mut self.file)
    }

    fn persist_output(output: &OutputTail, writer: &mut (impl io::Write + io::Seek)) -> io::Result<()> {
        writer.rewind()?;
        if output.truncated {
            writer.write_all(TRUNCATION)?;
        }
        let (first, second) = output.tail.as_slices();
        writer.write_all(first)?;
        writer.write_all(second)?;
        Ok(())
    }

    pub(crate) const fn truncated(&self) -> bool {
        self.output.truncated
    }

    pub(crate) fn excerpt(&self) -> Vec<u8> {
        self.output.excerpt()
    }
}

#[cfg(unix)]
fn prepare_reader_unix(reader: &io::PipeReader) -> io::Result<()> {
    prepare_reader_with(|operation, flags| {
        // SAFETY: reader owns a live descriptor; F_GETFL/F_SETFL take no pointer arguments.
        unsafe { libc::fcntl(reader.as_raw_fd(), operation, flags) }
    })
}

#[cfg(unix)]
fn prepare_reader_with(mut fcntl: impl FnMut(libc::c_int, libc::c_int) -> libc::c_int) -> io::Result<()> {
    let flags = fcntl(libc::F_GETFL, 0);
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if fcntl(libc::F_SETFL, flags | libc::O_NONBLOCK) == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn read_available_unix(reader: &mut io::PipeReader, bytes: &mut [u8]) -> io::Result<usize> {
    reader.read(bytes)
}

#[cfg(windows)]
fn read_available_windows(reader: &mut io::PipeReader, bytes: &mut [u8]) -> io::Result<usize> {
    // #[gamma::skip(literal.int_increment, reason = "PeekNamedPipe overwrites available on success; on failure its value is not read")]
    let mut available = 0;
    // SAFETY: reader owns the live anonymous pipe handle. The only non-null
    // output pointer addresses a writable u32; the zero-length buffer is null.
    let result = unsafe {
        PeekNamedPipe(
            reader.as_raw_handle(),
            ptr::null_mut(),
            {
                // #[gamma::skip(literal.int_increment, reason = "PeekNamedPipe documents nBufferSize as ignored when the adjacent lpBuffer argument is NULL")]
                0
            },
            ptr::null_mut(),
            &raw mut available,
            ptr::null_mut(),
        )
    };
    let peek = if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(available)
    };
    read_available_windows_with(bytes, peek, ERROR_BROKEN_PIPE.cast_signed(), |bytes| reader.read(bytes))
}

#[cfg(any(windows, test))]
fn read_available_windows_with(
    bytes: &mut [u8],
    peek: io::Result<u32>,
    broken_pipe: i32,
    read: impl FnOnce(&mut [u8]) -> io::Result<usize>,
) -> io::Result<usize> {
    let available = match peek {
        Ok(available) => available,
        Err(error) => {
            return if error.raw_os_error() == Some(broken_pipe) {
                // ERROR_BROKEN_PIPE: all writers have closed.
                Ok(0)
            } else {
                Err(error)
            };
        }
    };
    if available == 0 {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    let count = bytes.len().min(available as usize);
    // This is the sole reader, so the bytes observed above cannot be consumed
    // by another reader between the peek and this bounded read.
    read(&mut bytes[..count])
}

#[cfg(test)]
mod november_survivor_tests {
    #[test]
    fn november6_log_storage_preserves_flush_errors_and_exact_seek_results() {
        use std::io::{Seek as _, Write as _};

        struct Storage(io::Cursor<Vec<u8>>);
        impl io::Write for Storage {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.write(buf)
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::from_raw_os_error(5))
            }
        }
        impl io::Seek for Storage {
            fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
                self.0.seek(pos)
            }
        }
        let mut file = LogFile::Scripted(Box::new(Storage(io::Cursor::new(vec![2, 3, 5, 7, 11]))));
        assert_eq!(file.flush().unwrap_err().raw_os_error(), Some(5));
        assert_eq!(file.seek(io::SeekFrom::Start(3)).unwrap(), 3);
        assert_eq!(file.seek(io::SeekFrom::Current(1)).unwrap(), 4);
        assert_eq!(file.seek(io::SeekFrom::End(-3)).unwrap(), 2);
        assert_eq!(file.seek(io::SeekFrom::Start(0)).unwrap(), 0);
        assert_eq!(
            file.seek(io::SeekFrom::Current(-1)).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn november_pipe_preparation_preserves_each_native_error_and_releases_owned_ends_on_every_path_without_pipes() {
        #[derive(Debug)]
        struct Owned(&'static str, std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>);
        impl Drop for Owned {
            fn drop(&mut self) {
                self.1.borrow_mut().push(self.0);
            }
        }
        for stage in 0..=3 {
            let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let result = prepare_output_pipe(
                || {
                    events.borrow_mut().push("open");
                    if stage == 0 {
                        Err(io::Error::from_raw_os_error(5))
                    } else {
                        Ok((
                            Owned("drop-reader", std::rc::Rc::clone(&events)),
                            Owned("drop-writer", std::rc::Rc::clone(&events)),
                        ))
                    }
                },
                |reader| {
                    assert_eq!(reader.0, "drop-reader");
                    events.borrow_mut().push("prepare");
                    if stage == 1 { Err(io::Error::from_raw_os_error(6)) } else { Ok(()) }
                },
                |writer| {
                    assert_eq!(writer.0, "drop-writer");
                    events.borrow_mut().push("clone");
                    if stage == 2 {
                        Err(io::Error::from_raw_os_error(7))
                    } else {
                        Ok(Owned("drop-stdout", std::rc::Rc::clone(&events)))
                    }
                },
                |stdout, stderr| {
                    events.borrow_mut().push("attach");
                    assert_eq!(stdout.0, "drop-stdout");
                    assert_eq!(stderr.0, "drop-writer");
                    drop(stdout);
                    drop(stderr);
                },
            );
            if stage < 3 {
                assert_eq!(result.err().unwrap().raw_os_error(), Some(5 + stage));
            } else {
                let reader = result.unwrap();
                assert_eq!(reader.0, "drop-reader");
                drop(reader);
            }
            let expected: &[&str] = match stage {
                0 => &["open"],
                1 => &["open", "prepare", "drop-writer", "drop-reader"],
                2 => &["open", "prepare", "clone", "drop-writer", "drop-reader"],
                _ => &["open", "prepare", "clone", "attach", "drop-stdout", "drop-writer", "drop-reader"],
            };
            assert_eq!(*events.borrow(), expected);
        }
    }

    use super::*;

    #[test]
    fn windows_available_reads_distinguish_eof_would_block_and_errors_without_native_handles() {
        let mut bytes = [0; 7];
        for (available, count) in [(1, 1), (6, 6), (7, 7), (8, 7), (u32::MAX, 7)] {
            assert_eq!(
                read_available_windows_with(&mut bytes, Ok(available), 109, |output| {
                    assert_eq!(output.len(), count);
                    output.fill(0xa5);
                    Ok(count)
                })
                .unwrap(),
                count
            );
            assert_eq!(&bytes[..count], &vec![0xa5; count]);
        }
        let error = read_available_windows_with(&mut bytes, Ok(0), 109, |_| panic!("no available bytes")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(
            read_available_windows_with(&mut bytes, Err(io::Error::from_raw_os_error(109)), 109, |_| panic!("EOF")).unwrap(),
            0
        );
        let error =
            read_available_windows_with(&mut bytes, Err(io::Error::from_raw_os_error(5)), 109, |_| panic!("peek failed")).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(5));
        let error = read_available_windows_with(&mut bytes, Ok(1), 109, |_| {
            Err(io::Error::new(io::ErrorKind::Interrupted, "read interrupted"))
        })
        .unwrap_err();
        assert_eq!(
            (error.kind(), error.to_string()),
            (io::ErrorKind::Interrupted, "read interrupted".into())
        );
    }

    #[test]
    fn persisted_tail_rewinds_and_writes_both_wrapped_slices_and_only_real_truncation_markers() {
        for truncated in [false, true] {
            let mut tail = VecDeque::with_capacity(8);
            tail.extend(b"abcdef");
            tail.drain(..4);
            tail.extend(b"ghijkl");
            assert!(!tail.as_slices().0.is_empty());
            assert!(!tail.as_slices().1.is_empty());
            let output = OutputTail {
                tail,
                truncated,
                eof: false,
            };
            let mut writer = io::Cursor::new(Vec::new());
            writer.set_position(13);
            WorkerLog::persist_output(&output, &mut writer).unwrap();
            let expected = if truncated {
                [TRUNCATION, b"efghijkl"].concat()
            } else {
                b"efghijkl".to_vec()
            };
            assert_eq!(writer.into_inner(), expected);
        }
    }

    #[test]
    fn persisted_tail_propagates_seek_and_each_write_failure() {
        struct FailingWriter {
            operation: usize,
            fail_at: usize,
        }
        impl io::Write for FailingWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.operation += 1;
                if self.operation == self.fail_at {
                    Err(io::Error::new(io::ErrorKind::BrokenPipe, "write refused"))
                } else {
                    Ok(buf.len())
                }
            }

            fn flush(&mut self) -> io::Result<()> {
                panic!("persist does not flush")
            }
        }
        impl io::Seek for FailingWriter {
            fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
                assert_eq!(pos, io::SeekFrom::Start(0));
                self.operation += 1;
                if self.operation == self.fail_at {
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, "seek refused"))
                } else {
                    Ok(0)
                }
            }
        }
        let mut tail = VecDeque::with_capacity(4);
        tail.extend(b"abcd");
        tail.pop_front();
        tail.push_back(b'e');
        assert!(!tail.as_slices().1.is_empty());
        let output = OutputTail {
            tail,
            truncated: true,
            eof: false,
        };
        for fail_at in 1..=4 {
            let mut writer = FailingWriter { operation: 0, fail_at };
            let error = WorkerLog::persist_output(&output, &mut writer).unwrap_err();
            assert_eq!(writer.operation, fail_at);
            assert_eq!(error.to_string(), if fail_at == 1 { "seek refused" } else { "write refused" });
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn prepare_reader_unsupported(_reader: &io::PipeReader) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "worker output capture requires Unix or Windows",
    ))
}

#[cfg(not(any(unix, windows)))]
fn read_available_unsupported(_reader: &mut io::PipeReader, _bytes: &mut [u8]) -> io::Result<usize> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "worker output capture requires Unix or Windows",
    ))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io::Write as _;

    #[test]
    fn gamma_round2_eof_is_latched_and_idle_poll_reports_no_output() {
        let mut tail = super::OutputTail::new();
        let mut calls = 0;
        assert_eq!(
            tail.drain_with(|_| {
                calls += 1;
                Ok(0)
            })
            .unwrap(),
            (true, false)
        );
        assert_eq!(
            tail.drain_with(|_| {
                calls += 1;
                Err(std::io::Error::other("EOF must not be read again"))
            })
            .unwrap(),
            (true, false)
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn gamma_survivor_diagnostic_excerpt_boundary() {
        use super::OutputTail;
        for length in [0_usize, 1, 8_191, 8_192, 8_193, 16_384] {
            let bytes = (0..length).map(|index| u8::try_from(index % 251).unwrap()).collect::<Vec<_>>();
            let mut tail = OutputTail::new();
            tail.retain(&bytes);
            assert_eq!(tail.excerpt(), bytes[length.saturating_sub(8_192)..]);
        }
    }

    use super::*;

    fn log() -> OutputTail {
        OutputTail::new()
    }

    #[test]
    fn drain_limits_continuous_output_to_one_poll_budget() {
        let mut log = log();
        let mut consumed = 0;
        let state = log
            .drain_with(|bytes| {
                consumed += bytes.len();
                assert!(consumed <= DRAIN_BUDGET, "a poll must not starve the worker deadline");
                bytes.fill(b'x');
                Ok(bytes.len())
            })
            .unwrap();
        assert_eq!(state, (false, true));
        assert_eq!(consumed, DRAIN_BUDGET);
        assert_eq!(log.tail.len(), DRAIN_BUDGET);
    }

    #[test]
    fn drain_retries_interrupts_preserves_output_and_stops_on_would_block() {
        let mut log = log();
        let mut calls = 0;
        let state = log
            .drain_with(|bytes| {
                calls += 1;
                match calls {
                    1 => Err(io::ErrorKind::Interrupted.into()),
                    2 => {
                        bytes[..3].copy_from_slice(b"raw");
                        Ok(3)
                    }
                    3 => Err(io::ErrorKind::WouldBlock.into()),
                    _ => panic!("WouldBlock must end the poll"),
                }
            })
            .unwrap();
        assert_eq!(calls, 3);
        assert_eq!(state, (false, true));
        assert_eq!(log.excerpt(), b"raw");
        assert!(!log.eof);
    }

    #[test]
    fn drain_propagates_fatal_errors_instead_of_retrying_or_suppressing_them() {
        let mut log = log();
        let mut calls = 0;
        let error = log
            .drain_with(|_| {
                calls += 1;
                assert_eq!(calls, 1, "fatal errors must not be retried");
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "read denied"))
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "read denied");
        assert!(!log.eof);
    }

    #[test]
    fn drain_records_eof_once_and_keeps_the_bytes_before_it() {
        let mut log = log();
        let mut calls = 0;
        assert_eq!(
            log.drain_with(|bytes| {
                calls += 1;
                if calls == 1 {
                    bytes[0] = 0xff;
                    Ok(1)
                } else {
                    Ok(0)
                }
            })
            .unwrap(),
            (true, true)
        );
        assert_eq!(calls, 2);
        assert_eq!(log.excerpt(), [0xff]);
        assert_eq!(log.drain_with(|_| panic!("EOF is permanent")).unwrap(), (true, false));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn available_pipe_reads_distinguish_pending_data_and_eof() {
        #[cfg(windows)]
        {
            const FIXTURE: &str = "METABENCH_TEST_WINDOWS_PIPE";
            if std::env::var_os(FIXTURE).is_none() {
                let mut child = Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "worker_log::tests::available_pipe_reads_distinguish_pending_data_and_eof",
                    ])
                    .env(FIXTURE, "1")
                    .spawn()
                    .unwrap();
                let started = std::time::Instant::now();
                loop {
                    if let Some(status) = child.try_wait().unwrap() {
                        assert!(status.success());
                        return;
                    }
                    if started.elapsed() >= std::time::Duration::from_secs(10) {
                        child.kill().unwrap();
                        child.wait().unwrap();
                        panic!("reading an empty pipe must not block");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
            }
        }
        let (mut reader, mut writer) = io::pipe().unwrap();
        #[cfg(unix)]
        {
            prepare_reader(&reader).unwrap();
            prepare_reader(&reader).unwrap();
            // SAFETY: reader owns a live descriptor and F_GETFL takes no pointers.
            let flags = unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_GETFL) };
            assert_ne!(flags & libc::O_NONBLOCK, 0);
        }
        let mut bytes = [0; 4];
        assert_eq!(
            read_available(&mut reader, &mut bytes).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        writer.write_all(b"raw").unwrap();
        assert_eq!(read_available(&mut reader, &mut bytes).unwrap(), 3);
        assert_eq!(&bytes[..3], b"raw");
        drop(writer);
        assert_eq!(read_available(&mut reader, &mut bytes).unwrap(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn nonblocking_setup_preserves_flags_and_propagates_each_syscall_failure() {
        for flags in [0, libc::O_NONBLOCK, libc::O_APPEND | libc::O_NONBLOCK] {
            let mut calls = 0;
            prepare_reader_with(|operation, value| {
                calls += 1;
                match calls {
                    1 => {
                        assert_eq!((operation, value), (libc::F_GETFL, 0));
                        flags
                    }
                    2 => {
                        assert_eq!((operation, value), (libc::F_SETFL, flags | libc::O_NONBLOCK));
                        0
                    }
                    _ => panic!("only flag lookup and update are needed"),
                }
            })
            .unwrap();
            assert_eq!(calls, 2);
        }
        for failing_call in [1, 2] {
            let mut calls = 0;
            assert!(
                prepare_reader_with(|_, _| {
                    calls += 1;
                    assert!(calls <= failing_call, "setup must stop at the failed syscall");
                    if calls == failing_call { -1 } else { 0 }
                })
                .is_err()
            );
            assert_eq!(calls, failing_call);
        }
    }
}
