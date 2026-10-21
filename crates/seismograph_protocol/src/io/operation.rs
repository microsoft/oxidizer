// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Monitor operation recorded by the mock controller.
///
/// Available with the **`test-util`** feature. Select an operation for
/// [`super::MonitorIoControl::fail_next`] or inspect the sequence returned by
/// [`super::MonitorIoControl::operations`]. Infallible identity and clock
/// operations are recorded but cannot be configured to fail.
#[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
#[cfg_attr(
    not(any(test, feature = "test-util")),
    expect(
        unreachable_pub,
        reason = "the public re-export exists only with test-util; native builds use this enum internally"
    )
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoOperation {
    /// Resolve the current user.
    CurrentOwner,
    /// Read the process identifier.
    ProcessId,
    /// Resolve the executable path.
    CurrentExe,
    /// Generate an instance identity.
    InstanceId,
    /// Read monotonic time.
    Instant,
    /// Read wall-clock time.
    SystemTime,
    /// Wait between listener or connection attempts.
    Wait,
    /// Create or validate the current user's registry without elevation.
    CreateDirectory,
    /// Locate accessible per-user registries.
    DiscoverDirectories,
    /// Open and validate a private directory.
    OpenDirectory,
    /// Enumerate a discovery directory.
    Enumerate,
    /// Exclusively create a descriptor.
    CreateDescriptor,
    /// Open a descriptor for reading.
    ReadDescriptor,
    /// Write descriptor bytes.
    #[cfg(any(test, feature = "test-util"))]
    WriteDescriptor,
    /// Atomically publish a descriptor.
    PublishDescriptor,
    /// Remove a descriptor.
    RemoveDescriptor,
    /// Save a snapshot.
    WriteSnapshot,
    /// Bind a local endpoint.
    Bind,
    /// Configure listener blocking mode.
    ConfigureListener,
    /// Accept a client.
    Accept,
    /// Connect to a monitor.
    Connect,
    /// Duplicate a connection for cancellation.
    Duplicate,
    /// Configure a read deadline.
    ReadTimeout,
    /// Configure a write deadline.
    WriteTimeout,
    /// Read connection bytes.
    Read,
    /// Write connection bytes.
    Write,
    /// Inspect bytes without consuming them.
    Peek,
    /// Flush connection writes.
    Flush,
    /// Close and cancel a connection.
    Close,
}
