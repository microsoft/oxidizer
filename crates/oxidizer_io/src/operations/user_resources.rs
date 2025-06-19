// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

/// Marker trait to indicate that a type is suitable for attaching to an I/O operation
/// as a user resource, to be dropped once the operation has completed.
pub trait UserResource: Send + Sync + Debug + 'static {}

impl<T> UserResource for T where T: Send + Sync + Debug + 'static {}