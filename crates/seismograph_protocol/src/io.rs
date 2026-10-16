// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Instance-scoped monitor I/O, including identity, time, and persistence.
//!
//! [`MonitorIo::new_native`] uses the host OS. With `test-util`,
//! `MonitorIo::new_mocked` returns an isolated in-memory implementation and its
//! controller. Clones share that instance, never process-global test overrides.
//! Mocked capabilities cannot be constructed without test support.
//!
//! Pass the same capability to discovery and transport operations. Returned
//! directories, listeners, and connections retain a clone; methods on those
//! objects do not need another I/O argument.
//!
//! ```no_run
//! use seismograph_protocol::discovery::Directory;
//! use seismograph_protocol::io::MonitorIo;
//!
//! let io = MonitorIo::new_native();
//! let directory = Directory::current(&io)?;
//! for descriptor in directory.descriptors()? {
//!     let descriptor = descriptor?;
//!     println!("{}: {}", descriptor.owner, descriptor.endpoint);
//! }
//! # Ok::<(), seismograph_protocol::Error>(())
//! ```
//!
//! # Mocked I/O
//!
//! Enable the public `test-util` feature on a dev-dependency:
//!
//! ```toml
//! [dev-dependencies]
//! seismograph_protocol = { version = "0.2", features = ["test-util"] }
//! ```
//!
//! With `test-util`, identity, entropy, files, byte streams, and both
//! clocks are isolated from the host. Native tests are still needed to verify
//! actual operating-system permissions.
//!
//! ```
//! # #[cfg(feature = "test-util")]
//! # {
//! use seismograph_protocol::io::{IoOperation, MonitorIo};
//!
//! let (io, control) = MonitorIo::new_mocked();
//! control.fail_next(
//!     IoOperation::CurrentExe,
//!     std::io::ErrorKind::PermissionDenied.into(),
//! );
//! assert_eq!(
//!     io.current_exe().unwrap_err().kind(),
//!     std::io::ErrorKind::PermissionDenied
//! );
//! control.advance_time(std::time::Duration::from_secs(3));
//! assert_eq!(
//!     io.system_time(),
//!     std::time::UNIX_EPOCH + std::time::Duration::from_secs(3)
//! );
//! # }
//! ```

#[cfg(any(test, feature = "test-util"))]
pub(crate) mod mock;
mod operation;

use std::fs::{self, File};
use std::io::{self as std_io, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

#[cfg(any(test, feature = "test-util"))]
#[doc(inline)]
pub use mock::MonitorIoControl;
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) use operation::IoOperation;
#[cfg(any(test, feature = "test-util"))]
#[doc(inline)]
pub use operation::IoOperation;

use crate::discovery::Directory;
use crate::monitor::{InstanceId, MonitorDescriptor, MonitorEndpoint, MonitorOwner};
use crate::{Error, platform};

/// Cloneable monitor I/O capability. Construction performs no registry setup.
///
/// This concrete type supplies built-in native and mocked implementations,
/// not a trait for implementing custom backends. The public `test-util` feature
/// enables `new_mocked` and its controller; native I/O is always available.
#[derive(Clone, Debug)]
pub struct MonitorIo {
    core: Core,
}

#[derive(Clone, Debug)]
enum Core {
    Native,
    #[cfg(any(test, feature = "test-util"))]
    Mocked(mock::Runtime),
}

impl MonitorIo {
    /// Creates an I/O capability backed by the host operating system.
    #[must_use]
    pub const fn new_native() -> Self {
        Self { core: Core::Native }
    }

    /// Creates an isolated in-memory registry, transport, and controlled clock.
    ///
    /// The fixture starts as an ordinary user with a writable virtual current
    /// directory. Monitor startup creates its private registry automatically.
    /// Nothing is created
    /// on the host filesystem.
    ///
    /// Available with the **`test-util`** feature. Clones of the returned
    /// capability share the controller; separate calls create independent
    /// instances. Wall-clock time starts at [`std::time::UNIX_EPOCH`] and only
    /// [`MonitorIoControl::advance_time`] advances either mocked clock.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "test-util")]
    /// # {
    /// use seismograph_protocol::io::{IoOperation, MonitorIo};
    ///
    /// let (io, control) = MonitorIo::new_mocked();
    /// control.fail_next(
    ///     IoOperation::CurrentExe,
    ///     std::io::ErrorKind::PermissionDenied.into(),
    /// );
    /// assert_eq!(
    ///     io.current_exe().unwrap_err().kind(),
    ///     std::io::ErrorKind::PermissionDenied
    /// );
    /// assert!(!io.current_exe()?.as_os_str().is_empty());
    /// # }
    /// # Ok::<(), std::io::Error>(())
    /// ```
    #[cfg(any(test, feature = "test-util"))]
    #[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
    #[must_use]
    pub fn new_mocked() -> (Self, MonitorIoControl) {
        let runtime = mock::Runtime::new();
        let control = MonitorIoControl::new(runtime.clone());
        (
            Self {
                core: Core::Mocked(runtime),
            },
            control,
        )
    }

    /// Returns the current OS user, or the identity of the mocked instance.
    ///
    /// # Errors
    ///
    /// Returns an identity-query failure.
    pub fn current_owner(&self) -> std_io::Result<MonitorOwner> {
        self.checkpoint(IoOperation::CurrentOwner)?;
        match &self.core {
            Core::Native => platform::current_owner(),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => Ok(runtime.owner()),
        }
    }

    /// Returns the current process identifier.
    #[must_use]
    pub fn process_id(&self) -> u32 {
        self.record(IoOperation::ProcessId);
        match &self.core {
            Core::Native => std::process::id(),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => runtime.process_id(),
        }
    }

    /// Returns the executable path used for the default application name.
    ///
    /// # Errors
    ///
    /// Returns an executable-path lookup failure.
    pub fn current_exe(&self) -> std_io::Result<PathBuf> {
        self.checkpoint(IoOperation::CurrentExe)?;
        match &self.core {
            Core::Native => std::env::current_exe(),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => Ok(runtime.executable()),
        }
    }

    /// Generates a process-instance identity. This is not an access credential.
    ///
    /// # Errors
    ///
    /// Returns an entropy-source failure.
    pub fn instance_id(&self) -> Result<InstanceId, Error> {
        self.checkpoint(IoOperation::InstanceId).map_err(Error::Io)?;
        match &self.core {
            Core::Native => {
                let mut bytes = [0; 16];
                getrandom::fill(&mut bytes).map_err(|source| Error::Io(std_io::Error::other(source)))?;
                Ok(InstanceId::from_bytes(bytes))
            }
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => Ok(runtime.instance_id()),
        }
    }

    /// Reads monotonic time from this I/O instance.
    #[must_use]
    pub fn instant(&self) -> Instant {
        self.record(IoOperation::Instant);
        match &self.core {
            Core::Native => Instant::now(),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => runtime.instant(),
        }
    }

    /// Reads wall-clock time from this I/O instance.
    #[must_use]
    pub fn system_time(&self) -> SystemTime {
        self.record(IoOperation::SystemTime);
        match &self.core {
            Core::Native => SystemTime::now(),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => runtime.system_time(),
        }
    }

    /// Waits before retrying. Mocked waits yield execution without advancing the
    /// clock; only the controller advances mocked time.
    pub fn wait(&self, duration: Duration) {
        self.record(IoOperation::Wait);
        match &self.core {
            Core::Native => std::thread::sleep(duration),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => runtime.wait(),
        }
    }

    /// Publishes a descriptor by writing an exclusive temporary file and
    /// atomically renaming it inside the verified directory.
    ///
    /// # Errors
    ///
    /// Returns a descriptor, filesystem, or cleanup failure. A directory from a
    /// different I/O instance is rejected.
    pub fn publish_descriptor(&self, directory: &Directory, descriptor: &MonitorDescriptor) -> Result<PathBuf, Error> {
        if !self.same_instance(directory.io()) || directory.owner() != &descriptor.owner {
            return Err(registry_error(
                directory.path(),
                std_io::Error::new(
                    std_io::ErrorKind::PermissionDenied,
                    "descriptor and directory must belong to the same I/O instance and owner",
                ),
            ));
        }
        if descriptor.endpoint
            != MonitorEndpoint::for_instance(directory.path(), &descriptor.owner, descriptor.process_id, descriptor.instance_id)
        {
            return Err(Error::InvalidDescriptor);
        }
        let path = directory.path().join(descriptor.file_name());
        let temporary = path.with_extension("monitor.tmp");
        descriptor.write_file(self, &temporary)?;
        if let Err(source) = self.rename_descriptor(&temporary, &path) {
            return Err(self.cleanup_descriptor(&temporary, registry_error(&path, source)));
        }
        Ok(path)
    }

    pub(crate) fn cleanup_descriptor(&self, path: &Path, failure: Error) -> Error {
        match self.remove_descriptor(path) {
            Ok(()) => failure,
            Err(Error::Registry { source, .. }) if source.kind() == std_io::ErrorKind::NotFound => failure,
            Err(cleanup) => registry_error(path, std_io::Error::other(DescriptorCleanup { failure, cleanup })),
        }
    }

    /// Removes one descriptor without recursively removing its directory.
    ///
    /// # Errors
    ///
    /// Returns a removal failure, including `NotFound` for a missing entry.
    pub fn remove_descriptor(&self, path: &Path) -> Result<(), Error> {
        self.checkpoint(IoOperation::RemoveDescriptor)
            .map_err(|source| registry_error(path, source))?;
        match &self.core {
            Core::Native => fs::remove_file(path),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => runtime.remove_file(path),
        }
        .map_err(|source| registry_error(path, source))
    }

    /// Saves encoded snapshot bytes.
    ///
    /// # Errors
    ///
    /// Returns an opening or writing failure.
    pub fn write_snapshot(&self, path: &Path, bytes: &[u8]) -> std_io::Result<()> {
        self.checkpoint(IoOperation::WriteSnapshot)?;
        match &self.core {
            Core::Native => fs::write(path, bytes),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => runtime.write_snapshot(path, bytes),
        }
    }

    #[cfg_attr(
        not(any(test, feature = "test-util")),
        expect(
            clippy::unnecessary_wraps,
            reason = "native checkpoints are no-ops; mocked checkpoints inject I/O errors"
        )
    )]
    pub(crate) fn checkpoint(&self, operation: IoOperation) -> std_io::Result<()> {
        match (&self.core, operation) {
            (Core::Native, _) => Ok(()),
            #[cfg(any(test, feature = "test-util"))]
            (Core::Mocked(runtime), operation) => runtime.checkpoint(operation),
        }
    }

    fn record(&self, operation: IoOperation) {
        match (&self.core, operation) {
            (Core::Native, _) => {}
            #[cfg(any(test, feature = "test-util"))]
            (Core::Mocked(runtime), operation) => runtime.record(operation),
        }
    }

    pub(crate) fn same_instance(&self, other: &Self) -> bool {
        match (&self.core, &other.core) {
            (Core::Native, Core::Native) => true,
            #[cfg(any(test, feature = "test-util"))]
            (Core::Mocked(left), Core::Mocked(right)) => left.same_instance(right),
            #[cfg(any(test, feature = "test-util"))]
            _ => false,
        }
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn mocked(&self) -> Option<&mock::Runtime> {
        match &self.core {
            Core::Native => None,
            Core::Mocked(runtime) => Some(runtime),
        }
    }

    pub(crate) fn current_directory(&self, owner: &MonitorOwner) -> std_io::Result<(PathBuf, DirectoryGuard)> {
        self.checkpoint(IoOperation::CreateDirectory)?;
        match &self.core {
            Core::Native => {
                let directory = platform::current_directory(owner)?;
                Ok((
                    directory.path,
                    DirectoryGuard::Native {
                        _file: directory.file,
                        _temporary: TemporaryDirectory(directory.temporary),
                    },
                ))
            }
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => runtime.current_directory(owner).map(|path| (path, DirectoryGuard::Mocked)),
        }
    }

    pub(crate) fn registry_directories(&self) -> std_io::Result<platform::DirectoryEntries> {
        self.checkpoint(IoOperation::DiscoverDirectories)?;
        match &self.core {
            Core::Native => platform::registry_directories(),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => Ok(runtime.registry_directories()),
        }
    }

    pub(crate) fn open_directory(&self, path: &Path, owner: &MonitorOwner) -> std_io::Result<DirectoryGuard> {
        self.checkpoint(IoOperation::OpenDirectory)?;
        match &self.core {
            Core::Native => platform::open_directory(path, owner).map(|file| DirectoryGuard::Native {
                _file: file,
                _temporary: TemporaryDirectory(None),
            }),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => runtime.open_directory(path, Some(owner)).map(|()| DirectoryGuard::Mocked),
        }
    }

    pub(crate) fn entries(&self, path: &Path) -> std_io::Result<Vec<std_io::Result<PathBuf>>> {
        self.checkpoint(IoOperation::Enumerate)?;
        match &self.core {
            Core::Native => Ok(fs::read_dir(path)?.map(|entry| entry.map(|entry| entry.path())).collect()),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => runtime.entries(path),
        }
    }

    pub(crate) fn create_descriptor(&self, path: &Path, owner: &MonitorOwner) -> std_io::Result<DescriptorFile> {
        self.checkpoint(IoOperation::CreateDescriptor)?;
        match &self.core {
            Core::Native => platform::create_descriptor(path, owner).map(DescriptorFile::Native),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => runtime.create_file(path, owner).map(DescriptorFile::Mocked),
        }
    }

    pub(crate) fn read_descriptor(&self, path: &Path) -> std_io::Result<(DescriptorFile, MonitorOwner)> {
        self.checkpoint(IoOperation::ReadDescriptor)?;
        match &self.core {
            Core::Native => platform::read_descriptor(path).map(|(file, owner)| (DescriptorFile::Native(file), owner)),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => runtime.read_file(path).map(|(file, owner)| (DescriptorFile::Mocked(file), owner)),
        }
    }

    fn rename_descriptor(&self, from: &Path, to: &Path) -> std_io::Result<()> {
        self.checkpoint(IoOperation::PublishDescriptor)?;
        match &self.core {
            Core::Native => fs::rename(from, to),
            #[cfg(any(test, feature = "test-util"))]
            Core::Mocked(runtime) => runtime.rename_file(from, to),
        }
    }
}

#[derive(Debug)]
pub(crate) enum DirectoryGuard {
    Native {
        _file: File,
        _temporary: TemporaryDirectory,
    },
    #[cfg(any(test, feature = "test-util"))]
    Mocked,
}

#[derive(Debug)]
pub(crate) struct TemporaryDirectory(Option<tempfile::TempDir>);

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        if let Some(directory) = self.0.take()
            && let Err(error) = directory.close()
        {
            eprintln!("seismograph: failed to remove temporary monitor registry: {error}");
        }
    }
}

pub(crate) enum DescriptorFile {
    Native(File),
    #[cfg(any(test, feature = "test-util"))]
    Mocked(mock::DescriptorFile),
}

impl Read for DescriptorFile {
    fn read(&mut self, buf: &mut [u8]) -> std_io::Result<usize> {
        match self {
            Self::Native(file) => file.read(buf),
            #[cfg(any(test, feature = "test-util"))]
            Self::Mocked(file) => file.read(buf),
        }
    }
}

impl Write for DescriptorFile {
    fn write(&mut self, buf: &[u8]) -> std_io::Result<usize> {
        match self {
            Self::Native(file) => file.write(buf),
            #[cfg(any(test, feature = "test-util"))]
            Self::Mocked(file) => file.write(buf),
        }
    }

    fn flush(&mut self) -> std_io::Result<()> {
        match self {
            Self::Native(file) => file.flush(),
            #[cfg(any(test, feature = "test-util"))]
            Self::Mocked(file) => file.flush(),
        }
    }
}

pub(crate) fn registry_error(path: &Path, source: std_io::Error) -> Error {
    Error::Registry {
        path: path.to_owned(),
        source,
    }
}

#[derive(Debug)]
struct DescriptorCleanup {
    failure: Error,
    cleanup: Error,
}

impl std::fmt::Display for DescriptorCleanup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}; descriptor cleanup failed: {}", self.failure, self.cleanup)
    }
}

impl std::error::Error for DescriptorCleanup {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.failure)
    }
}
