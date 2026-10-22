// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt;

#[ohno::error]
#[from(std::io::Error)]
#[display("could not build the shared Tokio runtime")]
struct RuntimeBuildError;

#[ohno::error]
#[display("Tokio compatibility requires a multi-thread Tokio runtime")]
struct UnsupportedRuntimeError;

#[ohno::error]
#[display("an Arty worker already has a Tokio runtime context")]
struct ContextConflictError;

#[ohno::error]
#[from(arty::task::JoinError)]
#[display("Arty shut down during Tokio compatibility initialization")]
struct ArtyShutdownError;

#[ohno::error]
#[from(arty::task::JoinError)]
#[display("Arty rejected a Tokio installer's worker destination")]
struct InvalidDestinationError;

#[ohno::error]
#[from(arty::task::JoinError)]
#[display("a Tokio installer panicked")]
struct InstallerPanicError;

#[ohno::error]
#[display("Tokio compatibility initialization stopped before commit")]
struct InitializationAbortedError;

/// An error returned when a worker cannot enter the Tokio runtime context.
#[ohno::error]
#[from(RuntimeBuildError)]
#[from(UnsupportedRuntimeError)]
#[from(ContextConflictError)]
#[from(ArtyShutdownError)]
#[from(InvalidDestinationError)]
#[from(InstallerPanicError)]
#[from(InitializationAbortedError)]
struct InitErrorInner;

/// An error returned when a worker cannot enter the Tokio runtime context.
pub struct InitError {
    inner: InitErrorInner,
}

macro_rules! impl_from_condition {
    ($($condition:ty),+ $(,)?) => {
        $(
            impl From<$condition> for InitError {
                fn from(condition: $condition) -> Self {
                    Self {
                        inner: condition.into(),
                    }
                }
            }
        )+
    };
}

impl_from_condition!(
    RuntimeBuildError,
    UnsupportedRuntimeError,
    ContextConflictError,
    ArtyShutdownError,
    InvalidDestinationError,
    InstallerPanicError,
    InitializationAbortedError,
);

impl fmt::Debug for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>()).finish_non_exhaustive()
    }
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl Error for InitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.inner.source()
    }
}

pub(crate) fn runtime_build(error: std::io::Error) -> InitError {
    RuntimeBuildError::from(error).into()
}

pub(crate) fn unsupported_runtime() -> InitError {
    UnsupportedRuntimeError::new().into()
}

pub(crate) fn context_conflict() -> InitError {
    ContextConflictError::new().into()
}

pub(crate) fn initialization_aborted() -> InitError {
    InitializationAbortedError::new().into()
}

pub(crate) fn classify_join(error: arty::task::JoinError) -> InitError {
    if error.is_invalid_destination() {
        InvalidDestinationError::from(error).into()
    } else if error.is_panic() {
        InstallerPanicError::from(error).into()
    } else {
        ArtyShutdownError::from(error).into()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use arty::runtime::{Runtime, WorkersPolicy};

    use super::*;

    fn has_source<T>(error: &InitError) -> bool
    where
        T: std::error::Error + 'static,
    {
        error.source().is_some_and(<dyn std::error::Error>::is::<T>)
    }

    #[test]
    fn direct_failures_keep_private_source_types() {
        let runtime_build = runtime_build(std::io::Error::other("build failed"));
        let unsupported = unsupported_runtime();
        let conflict = context_conflict();
        let aborted = initialization_aborted();

        assert!(has_source::<RuntimeBuildError>(&runtime_build));
        assert!(has_source::<UnsupportedRuntimeError>(&unsupported));
        assert!(has_source::<ContextConflictError>(&conflict));
        assert!(has_source::<InitializationAbortedError>(&aborted));
    }

    #[test]
    fn join_failures_keep_private_source_types() {
        let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
        let builtins = runtime.scheduler().block_on(async |cx| cx).unwrap();
        let foreign_runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
        let foreign = foreign_runtime.scheduler().block_on(async |cx| cx.thread().clone()).unwrap();

        let invalid = futures::executor::block_on(builtins.scheduler().spawn_at(&foreign, (), |()| async {})).unwrap_err();
        let panic =
            futures::executor::block_on(builtins.scheduler().spawn(async |_| panic!("installer panic classification"))).unwrap_err();
        builtins.operations().request_stop();
        let shutdown = futures::executor::block_on(builtins.scheduler().spawn(async |_| ())).unwrap_err();

        assert!(has_source::<InvalidDestinationError>(&classify_join(invalid)));
        assert!(has_source::<InstallerPanicError>(&classify_join(panic)));
        assert!(has_source::<ArtyShutdownError>(&classify_join(shutdown)));

        runtime.stop().unwrap();
        foreign_runtime.stop().unwrap();
    }
}
