// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Temporary compatibility: remove as this crate migrates to Anvil defaults.
#![allow(
    clippy::empty_structs_with_brackets,
    reason = "Temporary compatibility with existing empty structs during migration to Anvil defaults"
)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! File-based log destinations with a background writer thread.
//!
//! This crate provides two destination flavors:
//!
//! * [`file_logs`] — append-only logging to a single file.
//! * `rolling_file_logs` — logging with automatic file rotation by size,
//!   with configurable retention (age or count) and optional compression.
//!   Requires the `rolling_file` feature.
//!
//! Internally, log records are forwarded to a background thread through a
//! bounded [`performables`](https://docs.rs/performables/) channel, avoiding
//! mutex contention on the hot (emit) path. When the channel is full, the newest
//! records are dropped rather than blocking the caller, and drops are periodically
//! reported in the destination output.
//!
//! # Behavior and quirks
//!
//! * [`file_logs`] opens the target file only when the destination is
//!   registered (when `FileLogDestination::register`
//!   is called). By default, it appends to existing contents; call
//!   [`FileLogDestinationBuilder::overwrite`] to truncate the file on open.
//! * `rolling_file_logs` also opens on its
//!   `RollingFileLogDestination::register` and rotates when the active file
//!   size exceeds the configured byte limit. Rotation retention can be based on
//!   file count (`FileLimit::MaxFiles`) or age (`FileLimit::Age`), and rotated
//!   files can be compressed when the `rolling_file` feature is enabled.
//!   Rotation is checked as buffered writes reach the rotating writer; a file
//!   can exceed the limit by a write batch or an oversized write.
//! * Logging is asynchronous. The emit path clones the SDK log record and
//!   attempts a non-blocking send to a bounded channel. If that channel is full
//!   or disconnected, the newest record is dropped and the caller is not
//!   blocked. Dropped records are counted and periodically written by the
//!   worker thread to the destination output as a warning like
//!   `dropped 42 events since 2026-05-18T09:45:00.000000000Z`.
//! * The background worker writes records in bounded batches. After receiving a
//!   record it drains a limited portion of already queued records before
//!   returning to control messages, so `force_flush`, `shutdown`, and resource
//!   updates stay responsive under sustained logging.
//! * Both destinations buffer formatted bytes on the worker thread to coalesce
//!   small writes. Use the `buffer_size` builder option to change it,
//!   or zero to disable buffering. Pending
//!   bytes reach the file when the buffer fills, at `flush_interval`,
//!   on `force_flush`, or on shutdown. The worker wakes to flush even
//!   when logging stops; continued logging does not postpone the deadline.
//!   Automatic flushes skip empty buffers and do not add work to the emit path.
//!   Timing depends on worker scheduling and I/O; flushing makes bytes visible
//!   to the operating system but does not guarantee durable storage.
//! * [`LogProcessor::force_flush`](opentelemetry_sdk::logs::LogProcessor::force_flush)
//!   sends a control message to the worker and waits for an acknowledgement.
//!   The wait is bounded by the `flush_ack_timeout` builder option.
//!   A timeout, stopped worker, or write/flush failure is returned
//!   as an `InternalFailure`.
//! * Write errors and automatic flush errors cannot be returned to the
//!   original sink. The worker remembers the first such error and reports it
//!   from the next `force_flush` or shutdown. Errors that happen while draining
//!   for that flush are returned by the same operation. The stored error is
//!   reported once.
//!   A failed write may have committed an unknown prefix. Its remaining buffered
//!   batch is discarded rather than replayed, avoiding duplicate log bytes at the
//!   cost of losing unwritten records in that batch. Later records can still be
//!   written. A downstream flush-only failure does not replay written bytes.
//! * Shutdown requests a final drain and flush, waits for acknowledgement up to
//!   the supplied timeout, then joins the writer thread. A stopped or panicked
//!   worker, poisoned worker mutex, or write/flush failure is reported as an
//!   `InternalFailure`. If the wait expires, shutdown returns `Timeout` without
//!   joining; the worker thread and file handle may remain alive until I/O
//!   completes. Subsequent shutdown and flush calls are no-ops, even after a
//!   timeout or failure, and new log records are ignored.
//! * The resource header is printed before the first log record. If the SDK
//!   resource changes later, the header is printed again before the next record.
//!
//! # Maintainer guide
//!
//! See the design and implementation guide in [`docs/design.md`](https://o365exchange.visualstudio.com/O365%20Core/_git/ox-sdk?path=/crates/observed_destination_file/docs/design.md)
//! for ownership, failure policy, and testing boundaries.

mod buffered_writer;
mod file_dest;
mod file_writer;
mod messages;
mod processor;
#[cfg(feature = "rolling_file")]
mod rolling_file_dest;
#[cfg(test)]
mod tests;
mod worker;

pub use file_dest::{FileLogDestination, FileLogDestinationBuilder, file_logs};
#[cfg(feature = "rolling_file")]
pub use rolling_file_dest::{FileLimit, RollingFileLogDestination, RollingFileLogDestinationBuilder, rolling_file_logs};
pub(crate) use worker::{DEFAULT_BUFFER_SIZE, DEFAULT_FLUSH_ACK_TIMEOUT, DEFAULT_FLUSH_INTERVAL, DEFAULT_QUEUE_CAPACITY};

/// Invalid configuration or output-file opening error for a file log destination.
#[ohno::error]
pub struct FileLogInitError;
