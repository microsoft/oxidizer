// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod transport;

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use transport::{Channel, PendingListener};
pub(crate) use transport::{Connection, Listener};

use super::{IoOperation, MonitorIo};
use crate::monitor::{InstanceId, MonitorOwner};

#[derive(Clone, Debug)]
pub(crate) struct Runtime {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Debug)]
struct State {
    owner: MonitorOwner,
    process_id: u32,
    administrator: bool,
    root: PathBuf,
    executable: PathBuf,
    epoch: Instant,
    elapsed: Duration,
    next_instance: u64,
    operations: Vec<IoOperation>,
    failures: VecDeque<(IoOperation, io::Error)>,
    entries: BTreeMap<PathBuf, Entry>,
    listeners: BTreeMap<PathBuf, PendingListener>,
    channels: Vec<Weak<Channel>>,
    max_chunk: usize,
}

#[derive(Clone, Debug)]
struct Entry {
    owner: MonitorOwner,
    secure: bool,
    data: Option<Vec<u8>>,
}

impl Runtime {
    pub(crate) fn new() -> Self {
        #[cfg(unix)]
        let (owner, root) = (MonitorOwner::UnixUser(1001), PathBuf::from("/seismograph-mock"));
        #[cfg(windows)]
        let (owner, root) = (
            MonitorOwner::WindowsSid("S-1-5-21-1-2-3-1001".into()),
            PathBuf::from(r"C:\seismograph-mock"),
        );
        let mut entries = BTreeMap::new();
        entries.insert(
            root.clone(),
            Entry {
                owner: owner.clone(),
                secure: true,
                data: None,
            },
        );
        entries.insert(
            PathBuf::from("."),
            Entry {
                owner: owner.clone(),
                secure: true,
                data: None,
            },
        );
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    owner,
                    process_id: 42,
                    administrator: false,
                    executable: root.join("worker"),
                    root,
                    epoch: Instant::now(),
                    elapsed: Duration::ZERO,
                    next_instance: 1,
                    operations: Vec::new(),
                    failures: VecDeque::new(),
                    entries,
                    listeners: BTreeMap::new(),
                    channels: Vec::new(),
                    max_chunk: usize::MAX,
                }),
                changed: Condvar::new(),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    pub(crate) fn checkpoint(&self, operation: IoOperation) -> io::Result<()> {
        let mut state = self.state();
        state.operations.push(operation);
        if let Some(index) = state.failures.iter().position(|(queued, _)| *queued == operation) {
            let (_, error) = state
                .failures
                .remove(index)
                .expect("index was obtained from this failure queue while locked");
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn record(&self, operation: IoOperation) {
        self.state().operations.push(operation);
    }

    pub(crate) fn owner(&self) -> MonitorOwner {
        self.state().owner.clone()
    }

    pub(crate) fn process_id(&self) -> u32 {
        self.state().process_id
    }

    pub(crate) fn executable(&self) -> PathBuf {
        self.state().executable.clone()
    }

    pub(crate) fn instance_id(&self) -> InstanceId {
        let mut state = self.state();
        let id = state.next_instance;
        state.next_instance = id
            .checked_add(1)
            .expect("recording u64::MAX instance operations would exhaust memory first");
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&id.to_le_bytes());
        InstanceId::from_bytes(bytes)
    }

    pub(crate) fn instant(&self) -> Instant {
        let state = self.state();
        state
            .epoch
            .checked_add(state.elapsed)
            .expect("mock clock advancement is checked by the controller")
    }

    pub(crate) fn system_time(&self) -> SystemTime {
        UNIX_EPOCH
            .checked_add(self.state().elapsed)
            .expect("mock clock advancement is checked by the controller")
    }

    #[cfg_attr(test, mutants::skip)] // A scheduler handoff does not advance mocked time or change observable I/O.
    pub(crate) fn wait(&self) {
        // This is only a scheduler hand-off, not advancement of observable time.
        // Activity wakes the waiter; the short real timeout keeps shutdown
        // responsive when a caller sets a stop flag outside the I/O capability.
        let state = self.state();
        drop(
            self.shared
                .changed
                .wait_timeout(state, Duration::from_millis(1))
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }

    fn administrative(state: &State) -> bool {
        state.administrator || matches!(state.owner, MonitorOwner::UnixUser(0))
    }

    fn directory<'a>(state: &'a State, path: &Path, owner: Option<&MonitorOwner>) -> io::Result<&'a Entry> {
        let entry = state.entries.get(path).ok_or(io::ErrorKind::NotFound)?;
        if entry.data.is_some() {
            return Err(io::ErrorKind::NotADirectory.into());
        }
        if !entry.secure || owner.is_some_and(|owner| owner != &entry.owner) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "monitor directory has unsafe ownership or permissions",
            ));
        }
        if owner.is_some() && state.owner != entry.owner && !Self::administrative(state) {
            return Err(access_denied());
        }
        Ok(entry)
    }

    pub(crate) fn open_directory(&self, path: &Path, owner: Option<&MonitorOwner>) -> io::Result<()> {
        Self::directory(&self.state(), path, owner).map(|_| ())
    }

    pub(crate) fn entries(&self, path: &Path) -> io::Result<Vec<io::Result<PathBuf>>> {
        let state = self.state();
        let entry = Self::directory(&state, path, None)?;
        if path != state.root && state.owner != entry.owner && !Self::administrative(&state) {
            return Err(access_denied());
        }
        Ok(state
            .entries
            .keys()
            .filter(|entry| entry.parent() == Some(path))
            .cloned()
            .map(Ok)
            .collect())
    }

    pub(crate) fn current_directory(&self, owner: &MonitorOwner) -> io::Result<PathBuf> {
        let mut state = self.state();
        if owner != &state.owner {
            return Err(access_denied());
        }
        Self::directory(&state, &state.root, None)?;
        let path = state.root.join(owner.to_string());
        if let std::collections::btree_map::Entry::Vacant(entry) = state.entries.entry(path.clone()) {
            entry.insert(Entry {
                owner: owner.clone(),
                secure: true,
                data: None,
            });
        }
        Self::directory(&state, &path, Some(owner))?;
        Ok(path)
    }

    pub(crate) fn registry_directories(&self) -> crate::platform::DirectoryEntries {
        let state = self.state();
        state
            .entries
            .keys()
            .filter(|path| path.parent() == Some(state.root.as_path()))
            .map(|path| {
                let owner = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or(io::ErrorKind::InvalidData)?
                    .parse::<MonitorOwner>()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                Ok(crate::platform::DirectoryEntry { path: path.clone(), owner })
            })
            .collect()
    }

    pub(crate) fn create_file(&self, path: &Path, owner: &MonitorOwner) -> io::Result<DescriptorFile> {
        let mut state = self.state();
        let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        Self::directory(&state, parent, Some(owner))?;
        if state.entries.contains_key(path) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        state.entries.insert(
            path.to_owned(),
            Entry {
                owner: owner.clone(),
                secure: true,
                data: Some(Vec::new()),
            },
        );
        Ok(DescriptorFile {
            runtime: self.clone(),
            path: path.to_owned(),
            position: 0,
        })
    }

    pub(crate) fn read_file(&self, path: &Path) -> io::Result<(DescriptorFile, MonitorOwner)> {
        let state = self.state();
        let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let entry = state.entries.get(path).ok_or(io::ErrorKind::NotFound)?;
        Self::directory(&state, parent, Some(&entry.owner))?;
        if !entry.secure || entry.data.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "monitor descriptor must be a private regular file",
            ));
        }
        Ok((
            DescriptorFile {
                runtime: self.clone(),
                path: path.to_owned(),
                position: 0,
            },
            entry.owner.clone(),
        ))
    }

    pub(crate) fn rename_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        let mut state = self.state();
        for path in [from, to] {
            let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
            let directory = Self::directory(&state, parent, None)?;
            if state.owner != directory.owner && !Self::administrative(&state) {
                return Err(access_denied());
            }
        }
        if state.entries.get(to).is_some_and(|entry| entry.data.is_none()) {
            return Err(io::ErrorKind::IsADirectory.into());
        }
        let entry = state.entries.remove(from).ok_or(io::ErrorKind::NotFound)?;
        state.entries.insert(to.to_owned(), entry);
        Ok(())
    }

    pub(crate) fn remove_file(&self, path: &Path) -> io::Result<()> {
        let mut state = self.state();
        let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let directory = Self::directory(&state, parent, None)?;
        if state.owner != directory.owner && !Self::administrative(&state) {
            return Err(access_denied());
        }
        let entry = state.entries.get(path).ok_or(io::ErrorKind::NotFound)?;
        if entry.data.is_none() {
            return Err(io::ErrorKind::IsADirectory.into());
        }
        state.entries.remove(path);
        Ok(())
    }

    pub(crate) fn write_snapshot(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut state = self.state();
        if state.entries.get(path).is_some_and(|entry| entry.data.is_none()) {
            return Err(io::ErrorKind::IsADirectory.into());
        }
        let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
        let directory = state.entries.get(parent).ok_or(io::ErrorKind::NotFound)?;
        if directory.data.is_some() {
            return Err(io::ErrorKind::NotADirectory.into());
        }
        if (directory.owner != state.owner || state.entries.get(path).is_some_and(|entry| entry.owner != state.owner))
            && !Self::administrative(&state)
        {
            return Err(access_denied());
        }
        let owner = state.owner.clone();
        state.entries.insert(
            path.to_owned(),
            Entry {
                owner,
                secure: true,
                data: Some(bytes.to_vec()),
            },
        );
        Ok(())
    }
}

pub(crate) struct DescriptorFile {
    runtime: Runtime,
    path: PathBuf,
    position: usize,
}

impl Read for DescriptorFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let state = self.runtime.state();
        let bytes = state
            .entries
            .get(&self.path)
            .and_then(|entry| entry.data.as_ref())
            .ok_or(io::ErrorKind::NotFound)?;
        let remaining = bytes.get(self.position..).ok_or(io::ErrorKind::InvalidData)?;
        let count = remaining.len().min(buf.len()).min(state.max_chunk);
        buf[..count].copy_from_slice(&remaining[..count]);
        self.position += count;
        Ok(count)
    }
}

impl Write for DescriptorFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.runtime.checkpoint(IoOperation::WriteDescriptor)?;
        let mut state = self.runtime.state();
        let count = buf.len().min(state.max_chunk);
        let bytes = state
            .entries
            .get_mut(&self.path)
            .and_then(|entry| entry.data.as_mut())
            .ok_or(io::ErrorKind::NotFound)?;
        bytes.extend_from_slice(&buf[..count]);
        self.position += count;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Controller for one isolated mocked monitor I/O instance.
///
/// Available with the **`test-util`** feature. Obtain it from
/// [`MonitorIo::new_mocked`]; cloning it controls the same instance.
/// Identity and permission changes apply only to the in-memory model, never
/// to the host user or filesystem.
#[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
#[derive(Clone, Debug)]
pub struct MonitorIoControl {
    runtime: Runtime,
}

impl MonitorIoControl {
    pub(super) const fn new(runtime: Runtime) -> Self {
        Self { runtime }
    }

    /// Fails the next occurrence of a fallible operation in this instance.
    /// Closing still cancels pending I/O before reporting an injected error.
    ///
    /// # Panics
    ///
    /// Panics if the selected operation has no error return.
    pub fn fail_next(&self, operation: IoOperation, error: io::Error) {
        assert!(
            !matches!(
                operation,
                IoOperation::ProcessId | IoOperation::Instant | IoOperation::SystemTime | IoOperation::Wait
            ),
            "only operations with an error return can have injected failures"
        );
        self.runtime.state().failures.push_back((operation, error));
        self.runtime.shared.changed.notify_all();
    }

    /// Advances both clocks and wakes operations waiting on mocked deadlines.
    ///
    /// # Panics
    ///
    /// Panics if the requested time cannot be represented by either clock.
    pub fn advance_time(&self, duration: Duration) {
        let channels = {
            let mut state = self.runtime.state();
            let elapsed = state
                .elapsed
                .checked_add(duration)
                .expect("test clock duration must remain representable");
            assert!(
                state.epoch.checked_add(elapsed).is_some() && UNIX_EPOCH.checked_add(elapsed).is_some(),
                "test clock time must remain representable"
            );
            state.elapsed = elapsed;
            state.channels.iter().filter_map(Weak::upgrade).collect::<Vec<_>>()
        };
        for channel in channels {
            let _state = channel.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            channel.changed.notify_all();
        }
        self.runtime.shared.changed.notify_all();
    }

    /// Returns the operations observed so far, in execution order.
    ///
    /// Transport operations record `Wait` after computing their deadline and
    /// immediately before blocking, so tests can advance time without racing
    /// the start of an operation.
    #[must_use]
    pub fn operations(&self) -> Vec<IoOperation> {
        self.runtime.state().operations.clone()
    }

    /// Changes the caller identity for subsequent mocked OS operations.
    pub fn set_owner(&self, owner: MonitorOwner) {
        self.runtime.state().owner = owner;
    }

    /// Controls whether the mocked caller has administrative access.
    pub fn set_administrator(&self, administrator: bool) {
        self.runtime.state().administrator = administrator;
    }

    /// Sets the maximum bytes transferred by one read or write.
    ///
    /// # Panics
    ///
    /// Panics for a zero limit, which would make nonempty writes impossible.
    pub fn set_max_chunk(&self, bytes: usize) {
        assert_ne!(bytes, 0, "mock I/O chunks must make forward progress");
        self.runtime.state().max_chunk = bytes;
    }

    /// Returns bytes stored by descriptor publication or snapshot saving.
    ///
    /// Returns `None` for a missing entry or a directory.
    #[must_use]
    pub fn file_bytes(&self, path: &Path) -> Option<Vec<u8>> {
        self.runtime.state().entries.get(path).and_then(|entry| entry.data.clone())
    }

    /// Replaces fixture file contents without using the host filesystem.
    pub fn put_file(&self, path: &Path, owner: MonitorOwner, bytes: Vec<u8>) {
        self.runtime.state().entries.insert(
            path.to_owned(),
            Entry {
                owner,
                secure: true,
                data: Some(bytes),
            },
        );
    }

    /// Marks an existing fixture entry as safe or unsafe for publication.
    ///
    /// # Panics
    ///
    /// Panics if the fixture entry does not exist.
    pub fn set_secure(&self, path: &Path, secure: bool) {
        self.runtime
            .state()
            .entries
            .get_mut(path)
            .expect("configure an existing mock entry")
            .secure = secure;
    }

    /// Creates a connected pair of in-memory streams in this I/O instance.
    #[must_use]
    pub fn connected_pair(&self) -> (crate::transport::Connection, crate::transport::Connection) {
        let (left, right) = Connection::pair(&self.runtime);
        let io = MonitorIo {
            core: super::Core::Mocked(self.runtime.clone()),
        };
        (
            crate::transport::Connection::from_mocked(io.clone(), left),
            crate::transport::Connection::from_mocked(io, right),
        )
    }
}

fn access_denied() -> io::Error {
    #[cfg(windows)]
    let code = 5;
    #[cfg(unix)]
    let code = libc::EACCES;
    io::Error::from_raw_os_error(code)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn partial_descriptor_transfers_advance_the_cursor_and_preserve_the_chunk_limit() {
        let runtime = Runtime::new();
        let control = MonitorIoControl::new(runtime.clone());
        let owner = runtime.owner();
        let directory = runtime.current_directory(&owner).unwrap();
        let path = directory.join("partial");
        let mut writer = runtime.create_file(&path, &owner).unwrap();
        control.set_max_chunk(3);
        assert_eq!(writer.write(b"abcdef").unwrap(), 3);
        assert_eq!(writer.read(&mut [0; 8]).unwrap(), 0);
        assert_eq!(control.file_bytes(&path).unwrap(), b"abc");
        let (mut reader, _) = runtime.read_file(&path).unwrap();
        control.set_max_chunk(2);
        let mut bytes = [0; 8];
        assert_eq!(reader.read(&mut bytes).unwrap(), 2);
        assert_eq!(&bytes[..2], b"ab");
        assert_eq!(reader.read(&mut bytes).unwrap(), 1);
        assert_eq!(bytes[0], b'c');
        assert_eq!(reader.read(&mut bytes).unwrap(), 0);
    }

    #[test]
    fn snapshot_writes_require_access_to_both_the_parent_and_an_existing_file() {
        let runtime = Runtime::new();
        let owner = runtime.owner();
        let parent = runtime.current_directory(&owner).unwrap();
        let foreign = match &owner {
            MonitorOwner::UnixUser(_) => MonitorOwner::UnixUser(2002),
            MonitorOwner::WindowsSid(_) => MonitorOwner::WindowsSid("S-1-5-21-1-2-3-2002".into()),
        };
        let path = parent.join("snapshot");
        runtime.state().entries.insert(
            path.clone(),
            Entry {
                owner: foreign.clone(),
                secure: true,
                data: Some(b"original".to_vec()),
            },
        );
        assert_eq!(
            runtime.write_snapshot(&path, b"new").unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        runtime.state().entries.get_mut(&path).unwrap().owner = owner;
        runtime.state().entries.get_mut(&parent).unwrap().owner = foreign;
        assert_eq!(
            runtime.write_snapshot(&path, b"new").unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(runtime.state().entries.get(&path).unwrap().data.as_deref(), Some(&b"original"[..]));
    }

    #[test]
    fn filesystem_operations_reject_invalid_parent_and_entry_types() {
        let runtime = Runtime::new();
        let owner = runtime.owner();
        let directory = runtime.current_directory(&owner).unwrap();
        let file = directory.join("file");
        let mut handle = runtime.create_file(&file, &owner).unwrap();
        handle.write_all(b"value").unwrap();
        handle.flush().unwrap();
        assert_eq!(
            runtime.write_snapshot(&directory, b"x").unwrap_err().kind(),
            io::ErrorKind::IsADirectory
        );
        assert_eq!(
            runtime.write_snapshot(&file.join("child"), b"x").unwrap_err().kind(),
            io::ErrorKind::NotADirectory
        );
        assert_eq!(
            runtime.open_directory(&file, None).unwrap_err().kind(),
            io::ErrorKind::NotADirectory
        );
        assert_eq!(runtime.remove_file(&directory).unwrap_err().kind(), io::ErrorKind::IsADirectory);

        let foreign = match owner {
            MonitorOwner::UnixUser(_) => MonitorOwner::UnixUser(2002),
            MonitorOwner::WindowsSid(_) => MonitorOwner::WindowsSid("S-1-5-21-1-2-3-2002".into()),
        };
        assert_eq!(
            runtime.current_directory(&foreign).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        runtime.state().owner = foreign;
        assert_eq!(runtime.entries(&directory).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            runtime.write_snapshot(&file, b"new").unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        runtime.state().administrator = true;
        runtime.write_snapshot(&file, b"new").unwrap();
        assert_eq!(runtime.state().entries.get(&file).unwrap().data.as_deref(), Some(&b"new"[..]));
    }

    #[test]
    fn descriptor_handles_report_removal_and_truncation_instead_of_panicking() {
        let runtime = Runtime::new();
        let owner = runtime.owner();
        let directory = runtime.current_directory(&owner).unwrap();
        let path = directory.join("file");
        let mut writer = runtime.create_file(&path, &owner).unwrap();
        writer.write_all(b"original").unwrap();
        let (mut reader, _) = runtime.read_file(&path).unwrap();
        assert_eq!(reader.read(&mut [0; 8]).unwrap(), 8);
        runtime.write_snapshot(&path, b"short").unwrap();
        assert_eq!(reader.read(&mut [0]).unwrap_err().kind(), io::ErrorKind::InvalidData);
        runtime.remove_file(&path).unwrap();
        assert_eq!(reader.read(&mut [0]).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(writer.write(&[0]).unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn moving_another_users_file_requires_administrative_access() {
        let runtime = Runtime::new();
        let owner = runtime.owner();
        let parent = runtime.current_directory(&owner).unwrap();
        let source = parent.join("source");
        let target = parent.join("target");
        runtime.create_file(&source, &owner).unwrap();
        runtime.state().owner = match &owner {
            MonitorOwner::UnixUser(_) => MonitorOwner::UnixUser(1002),
            MonitorOwner::WindowsSid(_) => MonitorOwner::WindowsSid("S-1-5-21-1-2-3-1002".into()),
        };
        assert_eq!(
            runtime.rename_file(&source, &target).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(runtime.state().entries.contains_key(&source));
        runtime.state().administrator = true;
        runtime.rename_file(&source, &target).unwrap();
        let state = runtime.state();
        assert!(!state.entries.contains_key(&source));
        assert_eq!(state.entries.get(&target).unwrap().owner, owner);
    }

    #[test]
    fn file_publication_does_not_replace_a_directory() {
        let runtime = Runtime::new();
        let owner = runtime.owner();
        let parent = runtime.current_directory(&owner).unwrap();
        let source = parent.join("source");
        let target = parent.join("target");
        runtime.state().entries.insert(
            target.clone(),
            Entry {
                owner: owner.clone(),
                secure: true,
                data: None,
            },
        );
        runtime.create_file(&source, &owner).unwrap();
        assert_eq!(
            runtime.rename_file(&source, &target).unwrap_err().kind(),
            io::ErrorKind::IsADirectory
        );
        assert!(runtime.state().entries.contains_key(&source));
        assert!(runtime.state().entries.get(&target).unwrap().data.is_none());
    }
}
