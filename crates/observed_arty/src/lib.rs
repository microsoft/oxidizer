// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Integrates an Observed sink with the Arty runtime.
//!
//! Call [`ObservedExt::enable_observed`] with an [`observed::Sink`] before you
//! submit a task. The first integration feature transfers enrichment into Arty
//! async tasks. It captures the enrichment at submission time, restores it for
//! each task poll, and removes it after the poll. Blocking functions submitted
//! through `spawn_blocking` are not covered.
//!
//! Enable the integration on a runtime builder:
//!
//! ```
//! use arty::runtime::Runtime;
//! use observed::Sink;
//! use observed_arty::ObservedExt as _;
//!
//! let sink = Sink::builder("example").build();
//! let runtime = Runtime::builder().enable_observed(sink).build()?;
//! runtime.stop()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! You can also enable the integration on a live runtime:
//!
//! ```
//! use arty::runtime::Runtime;
//! use observed::Sink;
//! use observed_arty::ObservedExt as _;
//!
//! let runtime = Runtime::new()?;
//! let sink = Sink::builder("example").build();
//! runtime
//!     .scheduler()
//!     .block_on(async |cx| cx.control().enable_observed(sink))?;
//! runtime.stop()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Live registration runs immediately on the calling worker and is queued on
//! other workers. An async task submitted on another worker before it processes
//! that queue is not covered. Configure the runtime builder when every async
//! task must receive the integration.
//!
//! Arty runs the task-spawn callback on the submitting worker when one exists.
//! Submissions from outside the runtime use Arty's worker-neutral callback.
//! Arty moves the captured transfer to the target worker, creates the poll
//! observer there, and keeps that observer on the target worker.
//!
//! Enrichment added inside the task stays above the transferred enrichment.
//! The integration restores the previous worker enrichment after every poll.

mod extensions;

pub use extensions::{ObservedExt, ObservedOptions};
