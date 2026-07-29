// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;
use alloc::string::{String, ToString as _};
use alloc::vec::Vec;
use core::fmt;

use http_path_template::{Grammar, PathTemplate};
use routerama_build::Route;
use routerama_build::trie::{build_trie, capture_field_names, conflicts};

use crate::HttpMethod;
use crate::build_error_entry::BuildErrorEntry;
use crate::configuration_error::ConfigurationError;
use crate::dyn_route::DynRoute;
use crate::raw_resolver::RawResolver;
use crate::rt_node::RtNode;

/// The runtime behind a generated route builder's dynamic routes.
pub struct DynBuilder<X> {
    entries: Vec<(Route, DynRoute<X>)>,
    errors: Vec<BuildErrorEntry>,
}

impl<X> DynBuilder<X> {
    /// Creates an empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Registers `path` for `method` under the dynamic `variant`.
    pub fn add(&mut self, method: HttpMethod, path: &str, expected: &[&'static str], variant: &'static str, extractor: X) {
        let template = match PathTemplate::parse(path, Grammar::default().with_segment_affixes()) {
            Ok(template) => template,
            Err(error) => {
                self.errors.push(BuildErrorEntry::InvalidTemplate {
                    variant,
                    path: path.to_string(),
                    source: error,
                });
                return;
            }
        };

        let found: Vec<String> = capture_field_names(template.segments()).iter().map(|name| name.join(".")).collect();
        let capture_order: Option<Box<[usize]>> = expected
            .iter()
            .map(|field| found.iter().position(|found| found == field))
            .collect::<Option<Vec<_>>>()
            .map(Vec::into_boxed_slice);
        let mut found_sorted = found;
        found_sorted.sort();
        let mut wanted: Vec<String> = expected.iter().map(|field| (*field).to_string()).collect();
        wanted.sort();
        let Some(capture_order) = capture_order else {
            self.errors.push(BuildErrorEntry::CaptureMismatch {
                variant,
                path: path.to_string(),
                expected: fmt_set(&wanted),
                found: fmt_set(&found_sorted),
            });
            return;
        };
        if found_sorted != wanted {
            self.errors.push(BuildErrorEntry::CaptureMismatch {
                variant,
                path: path.to_string(),
                expected: fmt_set(&wanted),
                found: fmt_set(&found_sorted),
            });
            return;
        }

        self.entries
            .push((Route::new(variant, method, template), DynRoute::new(extractor, capture_order)));
    }

    /// Records a missing dynamic route registration.
    pub fn require(&mut self, seen: bool, add_method: &'static str, variant: &'static str) {
        if !seen {
            self.errors.push(BuildErrorEntry::MissingRoute { add_method, variant });
        }
    }

    /// Finishes into a [`RawResolver`] or the accumulated [`ConfigurationError`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError`] if any dynamic route failed to register.
    pub fn finish(self) -> Result<RawResolver<DynRoute<X>>, ConfigurationError> {
        let Self { entries, mut errors } = self;
        let (routes, payloads): (Vec<Route>, Vec<DynRoute<X>>) = entries.into_iter().unzip();
        let trie = build_trie(&routes);
        errors.extend(
            conflicts(&trie.root)
                .into_iter()
                .map(|message| BuildErrorEntry::Conflict { message }),
        );
        if errors.is_empty() {
            Ok(RawResolver::with_trie(payloads.into_boxed_slice(), trie))
        } else {
            RtNode::discard_source(trie.root);
            Err(ConfigurationError::resolver(errors))
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<X> Default for DynBuilder<X> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<X> fmt::Debug for DynBuilder<X> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DynBuilder")
            .field("routes", &self.entries.len())
            .field("errors", &self.errors.len())
            .finish_non_exhaustive()
    }
}

fn fmt_set(items: &[String]) -> String {
    if items.is_empty() {
        return "{}".to_string();
    }
    let mut out = String::from("{");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(item);
    }
    out.push('}');
    out
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use super::*;

    #[test]
    fn failed_build_discards_deep_source_trie_iteratively() {
        std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(|| {
                let mut path = String::new();
                for index in 0..4_096 {
                    let _ = write!(path, "/segment{index}");
                }

                let mut builder = DynBuilder::new();
                builder.add(HttpMethod::GET, &path, &[], "Deep", ());
                builder.add(HttpMethod::GET, "/{broken", &[], "Invalid", ());
                builder.finish().expect_err("invalid template must fail the build");
            })
            .expect("test thread starts")
            .join()
            .expect("failed build must not overflow its stack");
    }
}
