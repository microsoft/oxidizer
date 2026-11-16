// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Isolated native IPC fixtures; never touch ordinary users' registries.
//!
//! Available with **`test-util`**. These helpers create real temporary files
//! and OS-protected local endpoints for native integration tests. For tests
//! without host I/O, use [`crate::io::MonitorIo::new_mocked`] instead.

#![cfg_attr(coverage_nightly, coverage(off))]
#![expect(
    clippy::unwrap_used,
    reason = "native test fixtures intentionally panic when test setup or assertions fail"
)]

use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::monitor::{InstanceId, MonitorDescriptor, MonitorEndpoint, MonitorOwner};
use crate::transport::{Connection, Listener};
use crate::{discovery, platform};

static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Temporary directory protected like production user entries.
#[derive(Debug)]
pub struct Directory {
    directory: discovery::Directory,
    _temporary: tempfile::TempDir,
}

impl Default for Directory {
    fn default() -> Self {
        Self::new()
    }
}

impl Directory {
    /// Creates an isolated, correctly owned native directory.
    ///
    /// # Panics
    ///
    /// Panics when creating, securing, or opening the native test directory fails.
    #[must_use]
    pub fn new() -> Self {
        // macOS's per-user TMPDIR can exceed sockaddr_un's pathname limit
        // after appending the registry and monitor identity.
        #[cfg(unix)]
        let temporary = tempfile::Builder::new()
            .prefix("sg-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        #[cfg(windows)]
        let temporary = tempfile::tempdir().unwrap();
        let path = fixture_directory_path(temporary.path());
        let owner = MonitorOwner::current().unwrap();
        platform::create_directory(&path, &owner).unwrap();
        let directory = discovery::Directory::open(&crate::io::MonitorIo::new_native(), &path, &owner).unwrap();
        Self {
            directory,
            _temporary: temporary,
        }
    }

    /// Returns the directory path.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    /// Returns an independently opened native directory guard.
    ///
    /// # Panics
    ///
    /// Panics when the fixture directory can no longer be opened or validated.
    #[must_use]
    pub fn open(&self) -> discovery::Directory {
        discovery::Directory::open(&crate::io::MonitorIo::new_native(), self.path(), self.directory.owner()).unwrap()
    }

    /// Creates fresh non-secret instance metadata for a fixture.
    ///
    /// # Panics
    ///
    /// Panics when the OS cannot provide the current user identity.
    #[must_use]
    pub fn descriptor(&self) -> MonitorDescriptor {
        fixture_descriptor(self.path(), &NEXT_INSTANCE, || MonitorOwner::current().unwrap(), std::process::id)
    }
}

fn fixture_directory_path(root: &Path) -> PathBuf {
    root.join("monitor")
}

fn retain_fixture<D>(slot: &mut Option<D>, directory: D) {
    *slot = Some(directory);
}

fn fixture_descriptor(
    path: &Path,
    sequence: &AtomicU64,
    owner: impl FnOnce() -> MonitorOwner,
    process_id: impl FnOnce() -> u32,
) -> MonitorDescriptor {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&sequence.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    let instance_id = InstanceId::from_bytes(bytes);
    let owner = owner();
    let process_id = process_id();
    MonitorDescriptor {
        name: "worker".into(),
        instance: None,
        process_id,
        instance_id,
        endpoint: MonitorEndpoint::for_instance(path, &owner, process_id, instance_id),
        owner,
    }
}

/// Binds a native fixture whose temporary directory lives with the listener.
///
/// # Panics
///
/// Panics when native fixture setup or endpoint binding fails.
#[must_use]
pub fn listener() -> Listener {
    listener_with(
        Directory::new,
        |directory| Listener::bind(&crate::io::MonitorIo::new_native(), &directory.descriptor()),
        |listener| &mut listener.fixture,
    )
    .unwrap()
}

fn listener_with<D, L>(
    directory: impl FnOnce() -> D,
    bind: impl FnOnce(&D) -> io::Result<L>,
    fixture: impl FnOnce(&mut L) -> &mut Option<D>,
) -> io::Result<L> {
    let directory = directory();
    let mut listener = bind(&directory)?;
    retain_fixture(fixture(&mut listener), directory);
    Ok(listener)
}

/// Creates a connected native client/server pair, without a TCP fallback.
///
/// # Panics
///
/// Panics when native fixture setup, connection, or acceptance fails.
#[must_use]
pub fn connected_pair() -> (Connection, Connection) {
    let listener = listener();
    let client = Connection::connect(&crate::io::MonitorIo::new_native(), listener.descriptor(), CONNECT_TIMEOUT).unwrap();
    let server = listener.accept().unwrap();
    (client, server)
}

/// Reports whether a native read is waiting for data on this connection or a clone.
///
/// Windows reports an operation registered as pending with the kernel. Unix
/// reports a receive that would block and has entered its readiness wait.
///
/// # Errors
///
/// Returns `Unsupported` for mocked connections.
pub fn read_is_pending(connection: &Connection) -> std::io::Result<bool> {
    connection.read_is_pending()
}

/// Returns the total bytes consumed by reads on this connection and its clones.
///
/// Peeking does not contribute to this count.
#[must_use]
pub fn bytes_read(connection: &Connection) -> usize {
    connection.bytes_read()
}

/// Returns the number of explicit close requests on this connection and its clones.
///
/// Dropping a handle or reaching a read deadline does not count as a close.
#[must_use]
pub fn close_calls(connection: &Connection) -> usize {
    connection.close_calls()
}

#[cfg(test)]
mod gamma_round2_tests {
    use super::*;

    #[derive(Debug)]
    struct DirectoryProbe {
        identity: u32,
        drops: std::rc::Rc<std::cell::Cell<usize>>,
    }

    impl Drop for DirectoryProbe {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    #[derive(Debug)]
    struct ListenerProbe {
        bound_directory: u32,
        fixture: Option<DirectoryProbe>,
    }

    #[test]
    fn nov6_actual_fixture_orchestration_retains_the_bound_directory_until_listener_drop() {
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));
        let calls = std::cell::RefCell::new(Vec::new());
        let listener = listener_with(
            || {
                calls.borrow_mut().push("directory");
                DirectoryProbe {
                    identity: 37,
                    drops: std::rc::Rc::clone(&drops),
                }
            },
            |directory| {
                assert_eq!(*calls.borrow(), ["directory"]);
                assert_eq!(directory.identity, 37);
                assert_eq!(drops.get(), 0);
                calls.borrow_mut().push("bind");
                Ok(ListenerProbe {
                    bound_directory: directory.identity,
                    fixture: None,
                })
            },
            |listener| {
                assert_eq!(*calls.borrow(), ["directory", "bind"]);
                calls.borrow_mut().push("retention");
                &mut listener.fixture
            },
        )
        .unwrap();
        assert_eq!(*calls.borrow(), ["directory", "bind", "retention"]);
        assert_eq!(listener.bound_directory, 37);
        assert_eq!(listener.fixture.as_ref().unwrap().identity, 37);
        assert_eq!(drops.get(), 0);
        drop(listener);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn nov6_actual_fixture_orchestration_drops_the_directory_once_after_binding_failure() {
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));
        let error = listener_with(
            || DirectoryProbe {
                identity: 37,
                drops: std::rc::Rc::clone(&drops),
            },
            |directory| {
                assert_eq!(directory.identity, 37);
                assert_eq!(drops.get(), 0);
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected fixture bind failure"))
            },
            |_listener: &mut ListenerProbe| panic!("failed binding must not access a listener's storage"),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "injected fixture bind failure");
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn november_native_fixture_policies_retain_the_directory_until_listener_drop_and_preserve_exact_namespace_and_timeout() {
        struct Owned(std::rc::Rc<std::cell::Cell<usize>>);
        impl Drop for Owned {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut fixture = None;
        retain_fixture(&mut fixture, Owned(std::rc::Rc::clone(&drops)));
        assert!(fixture.is_some());
        assert_eq!(drops.get(), 0, "a bound listener must retain its directory owner");
        drop(fixture);
        assert_eq!(drops.get(), 1);
        assert_eq!(
            fixture_directory_path(Path::new("owned-root")),
            Path::new("owned-root").join("monitor")
        );
        assert_eq!(CONNECT_TIMEOUT, Duration::from_secs(2));
    }

    #[test]
    fn gamma_round2_fixture_metadata_uses_exact_sequence_bytes_without_native_fixture_setup() {
        let (io, _) = crate::io::MonitorIo::new_mocked();
        let owner = io.current_owner().unwrap();
        let path = Path::new("in-memory-registry");
        let sequence = AtomicU64::new(17);
        let calls = std::cell::RefCell::new(Vec::new());
        for expected in [17, 18] {
            let descriptor = fixture_descriptor(
                path,
                &sequence,
                || {
                    calls.borrow_mut().push("owner");
                    owner.clone()
                },
                || {
                    calls.borrow_mut().push("pid");
                    31_337
                },
            );
            assert_eq!(descriptor.name, "worker");
            assert_eq!(descriptor.instance, None);
            assert_eq!(descriptor.owner, owner);
            assert_eq!(descriptor.process_id, 31_337);
            let bytes = descriptor.instance_id.as_bytes();
            assert_eq!(bytes[0], expected);
            assert_eq!(&bytes[1..], &[0; 15]);
            assert_eq!(
                descriptor.endpoint,
                MonitorEndpoint::for_instance(path, &owner, 31_337, descriptor.instance_id)
            );
        }
        assert_eq!(sequence.load(Ordering::Relaxed), 19);
        assert_eq!(calls.into_inner(), ["owner", "pid", "owner", "pid"]);
    }
}
