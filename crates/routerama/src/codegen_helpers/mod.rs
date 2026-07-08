// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime plumbing invoked by generated routers.
//!
//! The code emitted by the `routes!` macro and by `routerama_build` references
//! this module by absolute path (`::routerama::codegen_helpers`) for the
//! [`Route`] trait it implements and the [`scan_segments`] / [`split_verb`]
//! primitives its `resolve` calls. These items are an implementation detail of
//! code generation, not a human-facing API, so they are hidden from the rendered
//! documentation; the [`Route`] trait is documented at the crate root.

mod scan;

pub use scan::{scan_segments, split_verb};

pub use crate::route::{Route, RouteMatch, Router};
