// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Internal message types exchanged between the log processor front-end and
//! the background writer thread.

use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLogRecord;
use performables::sync::channel::OneshotSender;

#[derive(Debug)]
pub(super) struct QueuedLog {
    pub record: SdkLogRecord,
    pub scope: InstrumentationScope,
}

#[derive(Debug)]
pub(super) enum ControlMessage {
    Flush(OneshotSender<Result<(), String>>),
    Shutdown(OneshotSender<Result<(), String>>),
    SetResource(Resource),
}
