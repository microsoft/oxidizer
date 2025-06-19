// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{Debug, Display};

/// A reference to an I/O primitive registered with the operating system (e.g. file handle,
/// socket or similar).
///
/// Instances do not implicitly control ownership/lifetime of the underlying primitive and may be
/// freely cloned if necessary. For automatic lifetime management, use `OwnedPrimitive`.
///
/// Must implement `Send` because closing the primitive may take place on a different thread.
pub trait Primitive: Clone + Debug + Display + Send {
    /// Unregister the I/O primitive from the operating system.
    ///
    /// After this call, no further I/O operations can be issued on the primitive, though already
    /// issued elementary operations will still complete (potentially with errors).
    ///
    /// NB! This is a blocking call that may occupy the current thread for several seconds! To
    /// avoid disrupting latency-sensitive worker threads with blocking I/O, use instead the
    /// `OwnedPrimitive` wrapper type, which releases resources on background threads.
    fn close(&self);
}