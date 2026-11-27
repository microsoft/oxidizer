// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Transfers Observed enrichment into Arty async tasks.
//!
//! Enrichment contains context fields that Observed attaches to emitted events.
//! The integration captures these fields from the thread that submits an async
//! task. For example, a request ID attached before submission remains visible
//! on events that the task emits on another worker.
//!
//! Call [`ObservedExt::enable_observed`] with an [`observed::Sink`] before you
//! submit a task. The integration restores the captured enrichment for each
//! task poll and removes it after the poll. Blocking functions submitted
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
//! Arty enables a live registration on the calling worker immediately. Each
//! other worker enables it when that worker processes the queued registration.
//! Until then, tasks submitted by that worker do not inherit enrichment through
//! this integration. Configure the runtime builder when every async task must
//! receive the integration.
//!
//! The integration also captures enrichment when a thread outside the Arty
//! runtime submits a task. Captured enrichment appears before enrichment added
//! inside the task. The integration restores the previous worker enrichment
//! after every poll.

mod extensions;

pub use extensions::{ObservedExt, ObservedOptions};
