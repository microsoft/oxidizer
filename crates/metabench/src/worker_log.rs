// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read as _, Seek as _, Write as _};
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
    pub(crate) file: File,
    reader: io::PipeReader,
    output: OutputTail,
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
        let (reader, writer) = io::pipe()?;
        #[cfg(not(windows))]
        prepare_reader(&reader)?;
        command.stdout(writer.try_clone()?);
        command.stderr(writer);
        Ok(Self {
            file,
            reader,
            output: OutputTail::new(),
        })
    }

    pub(crate) fn release_command_writers(command: &mut Command) {
        // Command retains its Stdio handles after spawning; they would prevent EOF.
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    }

    /// Bounds work per poll so continuous output cannot starve the worker deadline.
    ///
    /// Returns whether EOF was reached and whether any bytes were consumed.
    pub(crate) fn drain(&mut self) -> io::Result<(bool, bool)> {
        self.output.drain_with(|bytes| read_available(&mut self.reader, bytes))
    }

    pub(crate) fn persist(&mut self) -> io::Result<()> {
        self.file.rewind()?;
        if self.output.truncated {
            self.file.write_all(TRUNCATION)?;
        }
        let (first, second) = self.output.tail.as_slices();
        self.file.write_all(first)?;
        self.file.write_all(second)?;
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
    let mut available = 0;
    // SAFETY: reader owns the live anonymous pipe handle. The only non-null
    // output pointer addresses a writable u32; the zero-length buffer is null.
    let result = unsafe {
        PeekNamedPipe(
            reader.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            &raw mut available,
            ptr::null_mut(),
        )
    };
    if result == 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_BROKEN_PIPE.cast_signed()) {
            // ERROR_BROKEN_PIPE: all writers have closed.
            Ok(0)
        } else {
            Err(error)
        };
    }
    if available == 0 {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    let count = bytes.len().min(available as usize);
    // This is the sole reader, so the bytes observed above cannot be consumed
    // by another reader between the peek and this bounded read.
    reader.read(&mut bytes[..count])
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
