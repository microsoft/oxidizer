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

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::monitor::{InstanceId, MonitorDescriptor, MonitorEndpoint, MonitorOwner};
use crate::transport::{Connection, Listener};
use crate::{discovery, platform};

static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);

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
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("monitor");
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
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed).to_le_bytes());
        let instance_id = InstanceId::from_bytes(bytes);
        let owner = MonitorOwner::current().unwrap();
        let process_id = std::process::id();
        MonitorDescriptor {
            name: "worker".into(),
            instance: None,
            process_id,
            instance_id,
            endpoint: MonitorEndpoint::for_instance(self.path(), &owner, process_id, instance_id),
            owner,
        }
    }
}

/// Binds a native fixture whose temporary directory lives with the listener.
///
/// # Panics
///
/// Panics when native fixture setup or endpoint binding fails.
#[must_use]
pub fn listener() -> Listener {
    let directory = Directory::new();
    let mut listener = Listener::bind(&crate::io::MonitorIo::new_native(), &directory.descriptor()).unwrap();
    listener.fixture = Some(directory);
    listener
}

/// Creates a connected native client/server pair, without a TCP fallback.
///
/// # Panics
///
/// Panics when native fixture setup, connection, or acceptance fails.
#[must_use]
pub fn connected_pair() -> (Connection, Connection) {
    let listener = listener();
    let client = Connection::connect(&crate::io::MonitorIo::new_native(), listener.descriptor(), Duration::from_secs(2)).unwrap();
    let server = listener.accept().unwrap();
    (client, server)
}
