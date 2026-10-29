// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::thread::ThreadId;
use std::{fmt, io};

use arty_io_core::{DriverError, DriverRole};
use data_privacy::classified;
use observed::{Sink, emit, event};

static SYSTEM_METADATA: data_privacy::DataClass = data_privacy::DataClass::new("arty", "SystemMetadata");
static IO_DRIVER_ERROR_MESSAGE: data_privacy::DataClass = data_privacy::DataClass::new("arty", "IoDriverErrorMessage");

#[classified(SYSTEM_METADATA)]
#[derive(Clone)]
struct WorkerId(String);

#[classified(SYSTEM_METADATA)]
#[derive(Clone, Copy)]
struct OperationLabel(&'static str);

#[classified(SYSTEM_METADATA)]
#[derive(Clone, Copy)]
struct OutcomeLabel(&'static str);

#[classified(SYSTEM_METADATA)]
#[derive(Clone)]
struct ErrorKindLabel(String);

#[classified(IO_DRIVER_ERROR_MESSAGE)]
#[derive(Clone)]
struct ErrorMessage(String);

impl ErrorMessage {
    fn new(error: &impl fmt::Display) -> Self {
        Self(error.to_string())
    }
}

#[derive(Clone, Copy)]
pub(crate) enum DriverOperation {
    Creation,
    Cycle,
    Shutdown,
}

impl DriverOperation {
    const fn label(self) -> OperationLabel {
        OperationLabel(match self {
            Self::Creation => "creation",
            Self::Cycle => "cycle",
            Self::Shutdown => "shutdown",
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) enum DriverOutcome {
    Succeeded,
    Failed,
}

impl DriverOutcome {
    const fn label(self) -> OutcomeLabel {
        OutcomeLabel(match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SourceOperation {
    Registration,
    Deregistration,
}

impl SourceOperation {
    const fn label(self) -> OperationLabel {
        OperationLabel(match self {
            Self::Registration => "registration",
            Self::Deregistration => "deregistration",
        })
    }
}

pub(crate) fn report_driver_started(sink: &Sink, role: DriverRole, worker_id: ThreadId) {
    emit!(
        sink,
        DriverStarted {
            role,
            worker_id: worker_id_label(worker_id),
        }
    );
}

pub(crate) fn report_driver_failed(sink: &Sink, role: DriverRole, worker_id: ThreadId, operation: DriverOperation, error: &DriverError) {
    emit!(
        sink,
        DriverFailed {
            role,
            worker_id: worker_id_label(worker_id),
            operation: operation.label(),
            error_kind: driver_error_kind(error),
            error: ErrorMessage::new(error),
        }
    );
}

pub(crate) fn report_driver_stopped(sink: &Sink, role: DriverRole, worker_id: ThreadId, outcome: DriverOutcome) {
    emit!(
        sink,
        DriverStopped {
            role,
            worker_id: worker_id_label(worker_id),
            outcome: outcome.label(),
        }
    );
}

pub(crate) fn report_source_operation_failed(sink: &Sink, operation: SourceOperation, error: &io::Error) {
    emit!(
        sink,
        SourceOperationFailed {
            operation: operation.label(),
            error_kind: io_error_kind(error),
            error: ErrorMessage::new(error),
        }
    );
}

pub(crate) fn report_context_attachment_failed(sink: &Sink, error: &io::Error) {
    emit!(
        sink,
        ContextAttachmentFailed {
            error_kind: io_error_kind(error),
            error: ErrorMessage::new(error),
        }
    );
}

fn worker_id_label(worker_id: ThreadId) -> WorkerId {
    WorkerId(format!("{worker_id:?}"))
}

fn io_error_kind(error: &io::Error) -> ErrorKindLabel {
    ErrorKindLabel(format!("{:?}", error.kind()))
}

fn driver_error_kind(error: &DriverError) -> ErrorKindLabel {
    error
        .source()
        .and_then(|source| source.downcast_ref::<io::Error>())
        .map_or_else(|| ErrorKindLabel("Driver".to_owned()), io_error_kind)
}

#[event("arty.mio.driver.started")]
#[debug("Mio driver started")]
#[counter(name = "arty.mio.driver.started")]
struct DriverStarted {
    #[dimension(log = "io.driver.role", metric = "io.driver.role")]
    #[data_class(SYSTEM_METADATA.clone())]
    role: DriverRole,
    #[dimension(log = "worker.id")]
    worker_id: WorkerId,
}

#[event("arty.mio.driver.failed")]
#[error("Mio driver failed")]
#[counter(name = "arty.mio.driver.failed")]
struct DriverFailed {
    #[dimension(log = "io.driver.role", metric = "io.driver.role")]
    #[data_class(SYSTEM_METADATA.clone())]
    role: DriverRole,
    #[dimension(log = "worker.id")]
    worker_id: WorkerId,
    #[dimension(log = "operation", metric = "operation")]
    operation: OperationLabel,
    #[dimension(log = "error.kind", metric = "error.kind")]
    error_kind: ErrorKindLabel,
    #[dimension(log = "error.message")]
    error: ErrorMessage,
}

#[event("arty.mio.driver.stopped")]
#[debug("Mio driver stopped")]
#[counter(name = "arty.mio.driver.stopped")]
struct DriverStopped {
    #[dimension(log = "io.driver.role", metric = "io.driver.role")]
    #[data_class(SYSTEM_METADATA.clone())]
    role: DriverRole,
    #[dimension(log = "worker.id")]
    worker_id: WorkerId,
    #[dimension(log = "outcome", metric = "outcome")]
    outcome: OutcomeLabel,
}

#[event("arty.mio.source.operation_failed")]
#[error("Mio source operation failed")]
#[counter(name = "arty.mio.source.operation_failed")]
struct SourceOperationFailed {
    #[dimension(log = "operation", metric = "operation")]
    operation: OperationLabel,
    #[dimension(log = "error.kind", metric = "error.kind")]
    error_kind: ErrorKindLabel,
    #[dimension(log = "error.message")]
    error: ErrorMessage,
}

#[event("arty.mio.context.attachment_failed")]
#[error("Mio context is not attached to a driver")]
#[counter(name = "arty.mio.context.attachment_failed")]
struct ContextAttachmentFailed {
    #[dimension(log = "error.kind", metric = "error.kind")]
    error_kind: ErrorKindLabel,
    #[dimension(log = "error.message")]
    error: ErrorMessage,
}
