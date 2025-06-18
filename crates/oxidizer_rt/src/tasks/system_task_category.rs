// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// System calls are made for different reasons. This type categorizes system tasks based on the
/// reason they are scheduled.
///
/// Different categories are subject to different handling by the Oxidizer Runtime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SystemTaskCategory {
    /// The default category, to be used when no special considerations apply for
    /// an operating system API call.
    #[default]
    Default,

    /// The task releases resources held with the operating system (e.g. closes a file or socket).
    ///
    /// These tasks are prioritized (releasing ownership of system resources as soon as possible
    /// allows those resources to be reused) and will always be executed, even if the runtime is
    /// shutting down, to ensure proper cleanup.
    ReleaseResources,
}