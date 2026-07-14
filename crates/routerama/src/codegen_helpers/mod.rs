// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime plumbing invoked by generated resolvers.
//!
//! The code emitted by `#[resolver]` and by `routerama_build` references
//! this module by absolute path (`::routerama::codegen_helpers`) for the
//! [`Resolver`] / [`RouteMatch`] traits it implements and the [`scan_segments`] /
//! [`split_verb`] primitives its `resolve` calls. These items are an
//! implementation detail of code generation, not a human-facing API, so they are
//! hidden from the rendered documentation; the traits are documented at the crate
//! root.

mod scan;

pub use scan::{scan_segments, seg_bytes, split_verb, substr};

pub use crate::route::{Resolver, RouteMatch};
