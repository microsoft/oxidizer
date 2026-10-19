// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod discovery;

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) use discovery::{current_directory, registry_directories};

use crate::monitor::{MonitorDescriptor, MonitorEndpoint, MonitorOwner};

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
const SOCKET_CLOEXEC: libc::c_int = libc::SOCK_CLOEXEC;
#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
const SOCKET_CLOEXEC: libc::c_int = 0;

#[expect(
    clippy::unnecessary_wraps,
    reason = "shared platform API also covers fallible Windows token queries"
)]
pub(crate) fn current_owner() -> io::Result<MonitorOwner> {
    // SAFETY: geteuid has no preconditions or Rust memory access.
    Ok(MonitorOwner::UnixUser(unsafe { libc::geteuid() }))
}

fn directory_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

pub(crate) fn open_directory(path: &Path, owner: &MonitorOwner) -> io::Result<File> {
    let MonitorOwner::UnixUser(uid) = owner else {
        return Err(denied("a Unix monitor directory requires a Unix user ID"));
    };
    let file = directory_file(path)?;
    let metadata = file.metadata()?;
    if metadata.uid() != *uid || metadata.mode() & 0o077 != 0 {
        return Err(denied("monitor directory must be owned by its user with mode 0700"));
    }
    Ok(file)
}

pub(crate) fn create_directory(path: &Path, owner: &MonitorOwner) -> io::Result<()> {
    let MonitorOwner::UnixUser(uid) = owner else {
        return Err(denied("a Unix monitor directory requires a Unix user ID"));
    };
    let result = fs::DirBuilder::new().mode(0o700).create(path);
    if let Err(error) = result {
        if error.kind() == io::ErrorKind::AlreadyExists {
            return open_directory(path, owner).map(drop);
        }
        return Err(error);
    }
    let file = directory_file(path)?;
    secure_directory(&file, *uid)?;
    open_directory(path, owner).map(drop)
}

#[cfg_attr(coverage_nightly, coverage(off))] // Failing fchown/chmod on our own live directory requires external privilege or filesystem changes.
#[cfg_attr(test, mutants::skip)] // Native permission tests exercise the result; kernel error injection is unavailable.
fn secure_directory(file: &File, uid: u32) -> io::Result<()> {
    // SAFETY: file owns a valid directory descriptor; -1 leaves the group unchanged.
    if unsafe { libc::fchown(file.as_raw_fd(), uid, !0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    file.set_permissions(fs::Permissions::from_mode(0o700))
}

fn open_relative(directory: &File, path: &Path, flags: libc::c_int) -> io::Result<File> {
    let name = CString::new(
        path.file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "descriptor needs a file name"))?
            .as_bytes(),
    )
    .map_err(|_error| io::Error::new(io::ErrorKind::InvalidInput, "descriptor file name contains a null byte"))?;
    // SAFETY: directory is open, name is terminated, and the flags include exclusive
    // creation or no-follow reading. The resulting descriptor transfers to File.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a fresh owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(crate) fn create_descriptor(path: &Path, owner: &MonitorOwner) -> io::Result<File> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "descriptor needs a directory"))?;
    let directory = open_directory(parent, owner)?;
    open_relative(&directory, path, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)
}

pub(crate) fn read_descriptor(path: &Path) -> io::Result<(File, MonitorOwner)> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "descriptor needs a directory"))?;
    let directory = directory_file(parent)?;
    let directory_metadata = directory.metadata()?;
    let owner = MonitorOwner::UnixUser(directory_metadata.uid());
    if directory_metadata.mode() & 0o077 != 0 {
        return Err(denied("monitor descriptor parent has unsafe permissions"));
    }
    // Nonblocking open ensures a malicious FIFO cannot stall administrative discovery.
    let file = open_relative(&directory, path, libc::O_RDONLY | libc::O_NONBLOCK)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != directory_metadata.uid() || metadata.mode() & 0o077 != 0 {
        return Err(denied(
            "monitor descriptor is not a private regular file owned by its directory's user",
        ));
    }
    Ok((file, owner))
}

#[derive(Debug)]
pub(crate) struct Listener {
    socket: UnixListener,
    path: PathBuf,
    owner: u32,
    identity: (u64, u64),
    _directory: File,
}

impl Listener {
    pub(crate) fn bind(descriptor: &MonitorDescriptor) -> io::Result<Self> {
        let (MonitorEndpoint::UnixSocket(path), MonitorOwner::UnixUser(owner)) = (&descriptor.endpoint, &descriptor.owner) else {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "Unix requires a pathname socket"));
        };
        let directory = open_directory(path.parent().ok_or_else(|| denied("socket needs a directory"))?, &descriptor.owner)?;
        let listener = UnixListener::bind(path)?;
        let metadata = fs::symlink_metadata(path)?;
        let bound = Self {
            socket: listener,
            path: path.clone(),
            owner: *owner,
            identity: (metadata.dev(), metadata.ino()),
            _directory: directory,
        };
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(bound)
    }

    pub(crate) fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.socket.set_nonblocking(nonblocking)
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // Foreign users are normally rejected by directory/socket permissions before accept can inspect credentials.
    #[cfg_attr(test, mutants::skip)] // Native cross-user tests retain access checks; owner/root peer policy is tested independently.
    pub(crate) fn accept(&self) -> io::Result<Connection> {
        let (stream, _) = self.socket.accept()?;
        stream.set_nonblocking(false)?;
        let (uid, _) = peer_identity(&stream)?;
        if !peer_allowed(self.owner, uid) {
            return Err(denied("monitor connections require the owning user or root"));
        }
        Ok(Connection::new(stream))
    }
}

impl Drop for Listener {
    #[cfg_attr(coverage_nightly, coverage(off))] // Cleanup errors and pathname replacement require external filesystem races.
    #[cfg_attr(test, mutants::skip)] // Native lifecycle tests verify socket cleanup; racing filesystem failures are not deterministic.
    fn drop(&mut self) {
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) if (metadata.dev(), metadata.ino()) == self.identity => {
                if let Err(error) = fs::remove_file(&self.path) {
                    eprintln!("seismograph: failed to remove monitor socket {}: {error}", self.path.display());
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => eprintln!("seismograph: failed to inspect monitor socket {}: {error}", self.path.display()),
        }
    }
}

const fn peer_allowed(owner: u32, peer: u32) -> bool {
    peer == owner || peer == 0
}

#[derive(Debug)]
pub(crate) struct Connection {
    stream: UnixStream,
    reads: Arc<Reads>,
}

#[derive(Debug, Default)]
struct Reads {
    timeout: Mutex<Option<Duration>>,
    #[cfg(any(test, feature = "test-util"))]
    pending: super::PendingReads,
}

impl Connection {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            reads: Arc::default(),
        }
    }

    pub(crate) fn connect(descriptor: &MonitorDescriptor, timeout: Duration) -> io::Result<Self> {
        let (MonitorEndpoint::UnixSocket(path), MonitorOwner::UnixUser(owner)) = (&descriptor.endpoint, &descriptor.owner) else {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "Unix requires a pathname socket"));
        };
        let stream = connect_timeout(path, timeout)?;
        let (uid, pid) = peer_identity(&stream)?;
        if uid != *owner || pid.is_some_and(|pid| pid != descriptor.process_id) {
            return Err(denied("connected monitor server has a different OS identity"));
        }
        Ok(Self::new(stream))
    }

    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        self.stream.try_clone().map(|stream| Self {
            stream,
            reads: Arc::clone(&self.reads),
        })
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn read_is_pending(&self) -> bool {
        self.reads.pending.active()
    }

    pub(crate) fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        if timeout.is_some_and(|timeout| timeout.is_zero() || Instant::now().checked_add(timeout).is_none()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "monitor read timeout must be positive and bounded",
            ));
        }
        // Read and peek use explicit poll deadlines, not socket timeout options.
        // This also permits configuring deadlines after a peer disconnects.
        *self.reads.timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = timeout;
        Ok(())
    }

    pub(crate) fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_write_timeout(timeout)
    }

    pub(crate) fn peek(&self, bytes: &mut [u8]) -> io::Result<usize> {
        self.receive(bytes, libc::MSG_PEEK)
    }

    pub(crate) fn close(&self) -> io::Result<()> {
        self.stream.shutdown(Shutdown::Both)
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // recv/poll interruption, failure, and deadline races cannot be forced reliably on live sockets.
    #[cfg_attr(test, mutants::skip)] // Native read, peek, timeout, and cancellation tests retain kernel-I/O coverage.
    fn receive(&self, buf: &mut [u8], flags: libc::c_int) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let timeout = *self.reads.timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let deadline = timeout
            .map(|timeout| {
                Instant::now()
                    .checked_add(timeout)
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "monitor read timeout is too large"))
            })
            .transpose()?;
        loop {
            // SAFETY: the socket is live and buf is writable for its length.
            // MSG_DONTWAIT affects this receive only, not concurrent writes.
            let count = unsafe {
                libc::recv(
                    self.stream.as_raw_fd(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    flags | libc::MSG_DONTWAIT,
                )
            };
            if let Ok(count) = usize::try_from(count) {
                return Ok(count);
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() != io::ErrorKind::WouldBlock {
                return Err(error);
            }
            let timeout = match deadline {
                Some(deadline) => {
                    let remaining = deadline
                        .checked_duration_since(Instant::now())
                        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "monitor read timed out"))?;
                    i32::try_from(remaining.as_millis().saturating_add(1)).unwrap_or(i32::MAX)
                }
                None => -1,
            };
            let mut poll = libc::pollfd {
                fd: self.stream.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            #[cfg(any(test, feature = "test-util"))]
            let _pending = self.reads.pending.enter();
            // SAFETY: poll points to one initialized pollfd; shutdown wakes this wait.
            let result = unsafe { libc::poll(&raw mut poll, 1, timeout) };
            if result == 0 {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "monitor read timed out"));
            }
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.receive(buf, 0)
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

#[cfg(target_os = "linux")]
#[cfg_attr(coverage_nightly, coverage(off))] // SO_PEERCRED failures and invalid kernel credentials cannot be injected on a valid connected socket.
#[cfg_attr(test, mutants::skip)] // Native cross-user tests verify authorization using actual kernel credentials.
fn peer_identity(stream: &UnixStream) -> io::Result<(u32, Option<u32>)> {
    // SAFETY: ucred is a C POD initialized before use by getsockopt.
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = libc::socklen_t::try_from(std::mem::size_of_val(&credentials)).expect("fixed Unix credentials fit socklen_t");
    // SAFETY: socket and output pointers are valid for their stated sizes.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::from_mut(&mut credentials).cast(),
            &raw mut length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let pid = u32::try_from(credentials.pid).map_err(|_error| denied("invalid monitor peer process ID"))?;
    Ok((credentials.uid, Some(pid)))
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
#[cfg_attr(coverage_nightly, coverage(off))] // getpeereid failures cannot be injected on a valid connected socket.
#[cfg_attr(test, mutants::skip)] // Native authorization tests retain the peer-identity boundary.
fn peer_identity(stream: &UnixStream) -> io::Result<(u32, Option<u32>)> {
    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: connected socket and writable UID/GID pointers are valid.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &raw mut uid, &raw mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((uid, None))
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
fn peer_identity(_stream: &UnixStream) -> io::Result<(u32, Option<u32>)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "monitor peer credentials are unavailable on this Unix platform",
    ))
}

fn connect_timeout(path: &Path, timeout: Duration) -> io::Result<UnixStream> {
    connect_timeout_with(path, timeout, std::thread::sleep)
}

fn connect_timeout_with(path: &Path, timeout: Duration, mut wait: impl FnMut(Duration)) -> io::Result<UnixStream> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .filter(|_| !timeout.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "connection timeout must be positive and bounded"))?;
    // SAFETY: sockaddr_un is C POD; fields are initialized below.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::sa_family_t::try_from(libc::AF_UNIX).expect("AF_UNIX fits the native address-family field");
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unix monitor socket path is too long or contains a null byte",
        ));
    }
    for (target, byte) in address.sun_path.iter_mut().zip(bytes) {
        *target = libc::c_char::from_ne_bytes([*byte]);
    }
    let length = libc::socklen_t::try_from(std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1)
        .expect("the pathname was bounded above by sockaddr_un's fixed buffer");
    let fd = connecting_socket()?;
    connect_socket(&fd, &address, length, deadline, &mut wait)?;
    let stream = UnixStream::from(fd);
    stream.set_nonblocking(false)?;
    Ok(stream)
}

#[cfg_attr(coverage_nightly, coverage(off))] // Pending-connect and signal-interruption outcomes depend on kernel scheduling.
#[cfg_attr(test, mutants::skip)] // Native connection and saturated-backlog tests retain timeout/backoff assertions.
fn connect_socket(
    fd: &OwnedFd,
    address: &libc::sockaddr_un,
    length: libc::socklen_t,
    deadline: Instant,
    wait: &mut impl FnMut(Duration),
) -> io::Result<()> {
    loop {
        // SAFETY: address is initialized and length describes its populated bytes.
        if unsafe { libc::connect(fd.as_raw_fd(), std::ptr::from_ref(address).cast(), length) } == 0 {
            break;
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EINPROGRESS) => {
                wait_connected(fd, deadline)?;
                break;
            }
            Some(libc::EAGAIN | libc::EINTR) if Instant::now() < deadline => {
                wait(deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(2)));
            }
            Some(libc::EAGAIN | libc::EINTR) => return Err(io::Error::new(io::ErrorKind::TimedOut, "monitor connection timed out")),
            _ => return Err(error),
        }
    }
    Ok(())
}

#[cfg_attr(coverage_nightly, coverage(off))] // Socket/fcntl failures require exhausting or invalidating process-owned descriptors.
#[cfg_attr(test, mutants::skip)] // Native tests assert the resulting close-on-exec and nonblocking flags.
fn connecting_socket() -> io::Result<OwnedFd> {
    // SAFETY: creating a stream socket has no pointer preconditions. Supported
    // kernels install close-on-exec atomically, before another thread can fork.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | SOCKET_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: socket returned a fresh owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if SOCKET_CLOEXEC == 0 {
        // Apple and other kernels without SOCK_CLOEXEC require this non-atomic fallback.
        // SAFETY: fd is valid and fcntl uses integer close-on-exec flags.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    // SAFETY: fd is valid and fcntl uses integer socket status flags.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

#[cfg_attr(coverage_nightly, coverage(off))] // Unix-domain connects normally complete immediately; pending-connect poll/error paths are kernel-dependent.
#[cfg_attr(test, mutants::skip)] // Native deadline tests exercise connection behavior without fabricating kernel outcomes.
fn wait_connected(fd: &OwnedFd, deadline: Instant) -> io::Result<()> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "monitor connection timed out"))?;
        let timeout = i32::try_from(remaining.as_millis().saturating_add(1)).unwrap_or(i32::MAX);
        let mut poll = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: poll points to one initialized pollfd.
        let result = unsafe { libc::poll(&raw mut poll, 1, timeout) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if result == 0 {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "monitor connection timed out"));
        }
        let mut error: libc::c_int = 0;
        let mut length = libc::socklen_t::try_from(std::mem::size_of_val(&error)).expect("native socket error integer fits socklen_t");
        // SAFETY: fd and output pointers are valid.
        if unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                std::ptr::from_mut(&mut error).cast(),
                &raw mut length,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        return if error == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(error))
        };
    }
}

fn denied(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "native directory/socket input validation and peer credentials")]
    fn native_inputs_and_server_identity_are_checked_before_use() {
        let directory = crate::testing::Directory::new();
        let descriptor = directory.descriptor();
        let other_platform = MonitorOwner::WindowsSid("S-1-5-18".into());
        assert_eq!(
            open_directory(directory.path(), &other_platform).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            create_directory(directory.path(), &other_platform).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        create_directory(directory.path(), &descriptor.owner).unwrap();
        assert_eq!(
            create_directory(&directory.path().join("missing/child"), &descriptor.owner)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            create_descriptor(Path::new("/"), &descriptor.owner).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(read_descriptor(Path::new("/")).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        let mut unsupported = descriptor.clone();
        unsupported.endpoint = MonitorEndpoint::NamedPipe("unsupported".into());
        assert_eq!(Listener::bind(&unsupported).unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert_eq!(
            Connection::connect(&unsupported, Duration::from_secs(1)).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        let _listener = Listener::bind(&descriptor).unwrap();
        let mut wrong = descriptor.clone();
        wrong.owner = MonitorOwner::UnixUser(!0);
        assert_eq!(
            Connection::connect(&wrong, Duration::from_secs(1)).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        let connection = Connection::connect(&descriptor, Duration::from_secs(1)).unwrap();
        for timeout in [Duration::ZERO, Duration::MAX] {
            assert_eq!(
                connection.set_read_timeout(Some(timeout)).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
            assert_eq!(
                connect_timeout(descriptor.endpoint.path(), timeout).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        for path in [PathBuf::from("/".repeat(200)), PathBuf::from("bad\0socket")] {
            assert_eq!(
                connect_timeout(&path, Duration::from_secs(1)).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    #[cfg_attr(miri, ignore = "native backlog behavior is exercised by native and careful tests")]
    fn saturated_backlog_waits_without_busy_spinning() {
        let temporary = tempfile::tempdir_in("/tmp").unwrap();
        let path = temporary.path().join("saturated.sock");
        let listener = UnixListener::bind(&path).unwrap();
        // SAFETY: the bound listener is live; reducing the backlog needs no pointers.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
        let _queued = UnixStream::connect(&path).unwrap();
        let start = Instant::now();
        let mut waits = Vec::new();
        let error = connect_timeout_with(&path, Duration::from_millis(40), |delay| {
            waits.push(delay);
            std::thread::sleep(delay);
        })
        .unwrap_err();
        let elapsed = start.elapsed();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(elapsed < Duration::from_secs(1), "connection deadline exceeded: {elapsed:?}");
        assert!(!waits.is_empty(), "full-backlog retries must wait instead of spinning");
        assert!(waits.iter().all(|wait| *wait <= Duration::from_millis(2)));
        assert!(waits.into_iter().sum::<Duration>() > Duration::ZERO);
    }

    #[test]
    fn peer_policy_allows_only_owner_and_root() {
        assert_eq!(
            [
                peer_allowed(1001, 1001),
                peer_allowed(1001, 0),
                peer_allowed(1001, 1002),
                peer_allowed(0, 1001)
            ],
            [true, true, false, false]
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "native and careful suites cover OS directory and socket permissions")]
    fn native_directory_and_socket_permissions_are_private() {
        let socket = connecting_socket().unwrap();
        // SAFETY: the fresh socket is live and F_GETFD takes no pointer argument.
        let descriptor_flags = unsafe { libc::fcntl(socket.as_raw_fd(), libc::F_GETFD) };
        // SAFETY: the fresh socket is live and F_GETFL takes no pointer argument.
        let status_flags = unsafe { libc::fcntl(socket.as_raw_fd(), libc::F_GETFL) };
        assert!(descriptor_flags >= 0 && status_flags >= 0);
        assert_eq!(descriptor_flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
        assert_ne!(status_flags & libc::O_NONBLOCK, 0);
        let listener = crate::testing::listener();
        let descriptor = listener.descriptor();
        assert_eq!(fs::metadata(descriptor.endpoint.path()).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(descriptor.endpoint.path().parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "native and careful suites cover descriptor redirection")]
    fn descriptor_symlinks_and_fifos_are_rejected_without_blocking() {
        let directory = crate::testing::Directory::new();
        let target = directory.path().join("target");
        fs::write(&target, []).unwrap();
        let link = directory.path().join("link.monitor");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        read_descriptor(&link).unwrap_err();
        let fifo = directory.path().join("fifo.monitor");
        let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a valid terminated pathname in the test fixture.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(read_descriptor(&fifo).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(read_descriptor(&target).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }
}
