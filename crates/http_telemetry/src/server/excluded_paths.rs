// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The base paths whose requests get no telemetry.

use std::sync::Arc;

use templated_uri::BasePath;

/// The base paths whose requests get no telemetry, each without its final `/`.
#[derive(Clone, Default)]
pub(crate) struct ExcludedPaths(Arc<[Box<str>]>);

impl ExcludedPaths {
    pub(crate) fn new(paths: impl IntoIterator<Item = impl Into<BasePath>>) -> Self {
        let paths = paths.into_iter().map(|path| {
            let path = path.into();
            path.as_str().strip_suffix('/').unwrap_or(path.as_str()).into()
        });
        Self(paths.collect())
    }

    /// Whether `path` is one of the base paths or a path under one, ignoring ASCII case. For example, `/health/`
    /// covers `/health` and `/HEALTH/live`, but not `/healths`.
    pub(crate) fn contains(&self, path: &str) -> bool {
        self.0.iter().any(|base| {
            let Some((start, rest)) = path.as_bytes().split_at_checked(base.len()) else {
                return false;
            };
            start.eq_ignore_ascii_case(base.as_bytes()) && matches!(rest.first(), None | Some(b'/'))
        })
    }
}
