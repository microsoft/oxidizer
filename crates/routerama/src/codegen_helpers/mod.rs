// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime support referenced by generated resolvers.

mod scan;
mod scanned_path;

pub use scan::{scan_segments, seg_bytes, split_verb, substr};
pub use scanned_path::{InvalidPath, ScannedPath, scan_path, with_scanned_path};

pub use crate::route_match::RouteMatch;
