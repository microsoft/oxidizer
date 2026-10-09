// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! OS-thread lifecycle and joining.

mod lifecycle;
pub(super) mod waiter;

pub(super) use lifecycle::spawn;
