// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Arty Mio connector for [`fetch_hyper`].
//!
//! [`MioConnector`] resolves HTTP endpoints and opens nonblocking Mio TCP
//! streams through [`arty_mio`]. [`MioStream`] implements
//! [`fetch_hyper::HyperIo`].
//!
//! Create one connector on each Arty worker. [`arty_mio::MioContext`] provides
//! the thread-aware route for direct source registration. The connector is
//! cloneable for Hyper, but it does not implement `ThreadAware`.
//!
//! Use [`anyspawn::Spawner::new_arty`] with the same worker scheduler when you
//! build a [`fetch_hyper::HyperTransportBuilder`].
//!
//! ```
//! use anyspawn::Spawner;
//! use arty::task::Builtins;
//! use arty_mio::{MioContext, MioOptions};
//! use fetch_mio::{MioConnector, MioConnectorOptions};
//!
//! fn worker_transport_parts(builtins: &Builtins) -> (MioConnector, Spawner) {
//!     let context = builtins.io().context::<MioContext>(&MioOptions::default());
//!     let connector = MioConnector::new(
//!         context,
//!         builtins.scheduler().clone(),
//!         builtins.clock().clone(),
//!         MioConnectorOptions::default(),
//!     );
//!     let spawner = Spawner::new_arty(builtins.scheduler().clone());
//!     (connector, spawner)
//! }
//! ```

mod connector;
mod error;
mod stream;

#[cfg(feature = "benchmark")]
#[doc(hidden)]
pub use connector::benchmark;
pub use connector::{MioConnector, MioConnectorOptions};
pub use stream::MioStream;
