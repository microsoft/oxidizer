// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use routerama_build::Route;
use routerama_build::trie::{Leaf, Trie, build_trie};
use smallvec::SmallVec;

use crate::captures::materialize;
use crate::codegen_helpers::{ScannedPath, scan_path, split_verb};
use crate::raw_match::{INLINE_CAPTURES, RawMatch};
use crate::rt_node::RtNode;
use crate::walk::Walk;

/// Segment offsets retained on the stack for each resolution.
///
/// Sixteen covers typical HTTP route depths without allocating. The paired
/// 16/17-segment benchmarks track this boundary: increasing it grows the two
/// `usize` scratch arrays in every resolver call, while decreasing it makes
/// shallower requests spill their offsets to the heap.
const INLINE_SEGMENTS: usize = 16;

const fn uses_heap_offsets(segment_count: usize) -> bool {
    segment_count > INLINE_SEGMENTS
}

/// A resolver for routes registered at run time.
pub struct RawResolver<T = ()> {
    root: RtNode,
    max_segments: usize,
    any_verb: bool,
    /// Values indexed by [`Leaf::route_index`].
    payloads: Box<[T]>,
}

impl RawResolver<()> {
    /// Builds a runtime resolver from a route set, with no per-route value.
    #[must_use]
    pub fn new(routes: impl IntoIterator<Item = Route>) -> Self {
        let routes: Vec<Route> = routes.into_iter().collect();
        let payloads = vec![(); routes.len()].into_boxed_slice();
        Self::build(&routes, payloads)
    }
}

impl<T> RawResolver<T> {
    /// Builds a resolver from `(route, value)` pairs.
    #[must_use]
    pub fn with_values(entries: impl IntoIterator<Item = (Route, T)>) -> Self {
        let (routes, payloads): (Vec<Route>, Vec<T>) = entries.into_iter().unzip();
        Self::build(&routes, payloads.into_boxed_slice())
    }

    /// Compiles an already-built trie with its route-aligned payload table.
    pub(crate) fn with_trie(payloads: Box<[T]>, trie: Trie) -> Self {
        Self::compile(trie, payloads)
    }

    fn build(routes: &[Route], payloads: Box<[T]>) -> Self {
        let trie = build_trie(routes);
        Self::compile(trie, payloads)
    }

    fn compile(trie: Trie, payloads: Box<[T]>) -> Self {
        Self {
            root: RtNode::compile(trie.root),
            max_segments: trie.max_segments,
            any_verb: trie.any_verb,
            payloads,
        }
    }

    /// Whether request paths are split at a trailing `:verb`.
    #[must_use]
    pub fn splits_verbs(&self) -> bool {
        self.any_verb
    }

    /// Enables verb splitting when another route source requires it.
    pub fn force_verb_split(&mut self, force: bool) {
        self.any_verb = self.any_verb || force;
    }
}

impl<T> fmt::Debug for RawResolver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawResolver")
            .field("routes", &self.payloads.len())
            .field("max_segments", &self.max_segments)
            .field("any_verb", &self.any_verb)
            .finish_non_exhaustive()
    }
}

impl<T> RawResolver<T> {
    /// Resolves `method` + `path`, returning the matched route or [`None`].
    ///
    /// `method` is matched by exact, case-sensitive token (e.g. `"GET"`), so pass
    /// the request method verbatim. [`HttpMethod`](crate::HttpMethod) may be
    /// passed directly, as it implements [`AsRef<str>`].
    ///
    /// Resolution is linear in the request-path length and does not use
    /// request-dependent recursion. Scratch storage is bounded by the lesser of
    /// the request segment count and configured route depth; deep paths or many
    /// captures may allocate.
    #[inline]
    pub fn resolve<'p, P>(&'p self, method: impl AsRef<str>, path: &'p P) -> Option<RawMatch<'p, T>>
    where
        P: AsRef<str> + ?Sized,
    {
        self.resolve_checked(method, path).ok().flatten()
    }

    #[inline]
    #[doc(hidden)]
    pub fn resolve_checked<'p, P>(
        &'p self,
        method: impl AsRef<str>,
        path: &'p P,
    ) -> Result<Option<RawMatch<'p, T>>, crate::codegen_helpers::InvalidPath>
    where
        P: AsRef<str> + ?Sized,
    {
        self.resolve_scanned_checked(method, path, |leaf, value, path| {
            let mut values: SmallVec<[&'p str; INLINE_CAPTURES]> = capture_values(leaf.vars.len());
            for plan in &leaf.vars {
                values.push(materialize(plan, path));
            }
            RawMatch { leaf, values, value }
        })
    }

    #[inline]
    #[doc(hidden)]
    /// Resolves a path and invokes `finish` before its scan offsets expire.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidPath`](crate::codegen_helpers::InvalidPath) when `path`
    /// contains a query or fragment delimiter.
    pub fn resolve_scanned_checked<'p, P, R>(
        &'p self,
        method: impl AsRef<str>,
        path: &'p P,
        finish: impl for<'s> FnOnce(&'p Leaf, &'p T, &ScannedPath<'p, 's>) -> R,
    ) -> Result<Option<R>, crate::codegen_helpers::InvalidPath>
    where
        P: AsRef<str> + ?Sized,
    {
        let method = method.as_ref();
        let path = path.as_ref();
        let (body, verb) = if self.any_verb { split_verb(path) } else { (path, None) };

        let mut inline_starts = [0_usize; INLINE_SEGMENTS];
        let mut inline_ends = [0_usize; INLINE_SEGMENTS];
        if self.max_segments <= INLINE_SEGMENTS {
            let path = scan_path(body, &mut inline_starts[..self.max_segments], &mut inline_ends[..self.max_segments]);
            if !path.is_valid() {
                return Err(crate::codegen_helpers::InvalidPath);
            }
            let walk = Walk { path: &path, method, verb };
            let leaf = if self.any_verb {
                walk.descend_iterative::<true>(&self.root, 0)
            } else {
                walk.descend_iterative::<false>(&self.root, 0)
            };
            let Some(leaf) = leaf else {
                return Ok(None);
            };
            let value = &self.payloads[leaf.route_index];
            return Ok(Some(finish(leaf, value, &path)));
        }

        let path = scan_path(body, &mut inline_starts, &mut inline_ends);
        if !path.is_valid() {
            return Err(crate::codegen_helpers::InvalidPath);
        }
        if uses_heap_offsets(path.count()) {
            return Ok(self.resolve_with_heap_offsets(method, body, verb, path.count(), finish));
        }

        Ok(self.finish_scanned(method, verb, &path, finish))
    }

    fn resolve_with_heap_offsets<'p, R>(
        &'p self,
        method: &str,
        body: &'p str,
        verb: Option<&str>,
        segment_count: usize,
        finish: impl for<'s> FnOnce(&'p Leaf, &'p T, &ScannedPath<'p, 's>) -> R,
    ) -> Option<R> {
        let capacity = segment_count.min(self.max_segments);
        let mut heap_offsets = vec![0_usize; capacity * 2];
        let (starts, ends) = heap_offsets.split_at_mut(capacity);
        let path = scan_path(body, starts, ends);
        self.finish_scanned(method, verb, &path, finish)
    }

    fn finish_scanned<'p, R>(
        &'p self,
        method: &str,
        verb: Option<&str>,
        path: &ScannedPath<'p, '_>,
        finish: impl for<'s> FnOnce(&'p Leaf, &'p T, &ScannedPath<'p, 's>) -> R,
    ) -> Option<R> {
        let walk = Walk { path, method, verb };
        let leaf = if self.any_verb {
            walk.descend_iterative::<true>(&self.root, 0)
        } else {
            walk.descend_iterative::<false>(&self.root, 0)
        }?;
        let value = &self.payloads[leaf.route_index];
        Some(finish(leaf, value, path))
    }
}

#[inline]
fn capture_values<'p>(count: usize) -> SmallVec<[&'p str; INLINE_CAPTURES]> {
    if count <= INLINE_CAPTURES {
        SmallVec::new()
    } else {
        SmallVec::with_capacity(count)
    }
}

#[cfg(test)]
mod tests {
    use alloc::borrow::ToOwned;
    use alloc::format;
    use alloc::string::String;
    use core::fmt::Write as _;
    use std::process::Command;

    use http_path_template::{Grammar, PathTemplate};
    use routerama_build::trie::VarPlan;

    use super::*;
    use crate::route_match::RouteMatch;

    #[test]
    fn materialize_rest_captures_the_remainder_or_nothing() {
        let plan = VarPlan::Rest {
            field: "path".to_owned(),
            key: "path".to_owned(),
            a: 1,
        };
        let (mut starts, mut ends) = ([0; 3], [0; 3]);
        let populated = scan_path("/files/a/b", &mut starts, &mut ends);
        assert_eq!(materialize(&plan, &populated), "a/b");

        let (mut starts, mut ends) = ([0; 1], [0; 1]);
        let empty = scan_path("/files", &mut starts, &mut ends);
        assert_eq!(materialize(&plan, &empty), "");
    }

    #[test]
    fn materialize_affix_strips_the_prefix_and_suffix() {
        let plan = VarPlan::Affix {
            field: "id".to_owned(),
            key: "id".to_owned(),
            a: 0,
            prefix_len: 4,
            suffix_len: 4,
        };
        let (mut starts, mut ends) = ([0; 1], [0; 1]);
        let path = scan_path("/img-cat.png", &mut starts, &mut ends);
        assert_eq!(materialize(&plan, &path), "cat");
    }

    #[test]
    fn debug_reports_the_router_shape() {
        let router = RawResolver::new([Route::new(
            "Get",
            "GET",
            PathTemplate::parse("/a/{x}", Grammar::default()).expect("valid template"),
        )]);
        let debug = format!("{router:?}");
        assert!(debug.contains("RawResolver"), "{debug}");
        assert!(debug.contains("max_segments"), "{debug}");
        assert!(debug.contains("any_verb"), "{debug}");
    }

    #[test]
    fn capture_storage_reserves_for_large_capture_sets() {
        let count = INLINE_CAPTURES + 1;
        assert!(capture_values(count).capacity() >= count);
    }

    #[test]
    fn deep_route_set_spills_the_offset_buffers_to_the_heap() {
        assert!(!uses_heap_offsets(INLINE_SEGMENTS));
        assert!(uses_heap_offsets(INLINE_SEGMENTS + 1));

        let depth = INLINE_SEGMENTS + 4;
        let mut template = String::new();
        for i in 0..depth {
            let _ = write!(template, "/{{v{i}}}");
        }
        template.push_str(":watch");

        let router = RawResolver::new([Route::new(
            "Deep",
            "GET",
            PathTemplate::parse(&template, Grammar::default()).expect("valid template"),
        )]);

        let mut path = String::new();
        for i in 0..depth {
            let _ = write!(path, "/seg{i}");
        }
        path.push_str(":watch");
        let matched = router.resolve("GET", &path).expect("deep route matches through the heap buffers");
        assert_eq!(matched.name(), "Deep");
        assert_eq!(matched.capture("v0"), Some("seg0"));
        assert_eq!(
            matched.capture(&format!("v{}", depth - 1)),
            Some(format!("seg{}", depth - 1).as_str())
        );
        assert!(router.resolve("GET", "/shallow").is_none());
        assert!(matches!(
            router.resolve_checked("GET", "/shallow?query"),
            Err(crate::codegen_helpers::InvalidPath)
        ));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn deeply_nested_route_builds_resolves_and_drops_without_overflowing() {
        const CHILD_ENV: &str = "ROUTERAMA_DEEP_ROUTE_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            let mut template = String::new();
            for index in 0..10_000 {
                let _ = write!(template, "/s{index}");
            }
            let router = RawResolver::new([Route::new(
                "Deep",
                "GET",
                PathTemplate::parse(&template, Grammar::default()).expect("valid deep template"),
            )]);
            assert!(router.resolve("GET", &template).is_some());
            drop(router);
            return;
        }

        let output = Command::new(std::env::current_exe().expect("current test executable is available"))
            .args([
                "--exact",
                "raw_resolver::tests::deeply_nested_route_builds_resolves_and_drops_without_overflowing",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .expect("deep-route child process starts");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "deep-route child failed:\n{stderr}");
    }

    #[test]
    fn with_values_hands_back_the_registered_value() {
        let mk = |name: &str, pattern: &str| {
            Route::new(
                name,
                "GET",
                PathTemplate::parse(pattern, Grammar::default()).expect("valid template"),
            )
        };
        let resolver = RawResolver::with_values([(mk("ListBooks", "/books"), 10_u32), (mk("GetBook", "/books/{book}"), 20_u32)]);

        let hit = resolver.resolve("GET", "/books/rust").expect("match");
        assert_eq!(*hit.value(), 20);
        assert_eq!(hit.name(), "GetBook");
        assert_eq!(hit.capture("book"), Some("rust"));

        assert_eq!(*resolver.resolve("GET", "/books").expect("match").value(), 10);
        assert!(resolver.resolve("POST", "/books").is_none());
    }

    #[test]
    fn stored_handlers_dispatch_without_a_by_name_lookup() {
        let mk = |name: &str, pattern: &str| {
            Route::new(
                name,
                "GET",
                PathTemplate::parse(pattern, Grammar::default()).expect("valid template"),
            )
        };
        let resolver = RawResolver::with_values([
            (
                mk("ListBooks", "/books"),
                (|_m| "list".to_owned()) as fn(&dyn RouteMatch<'_>) -> String,
            ),
            (
                mk("GetBook", "/books/{book}"),
                (|m| format!("get {}", m.capture("book").unwrap_or("?"))) as fn(&dyn RouteMatch<'_>) -> String,
            ),
        ]);

        let matched = resolver.resolve("GET", "/books/rust").expect("match");
        let handler = *matched.value();
        assert_eq!(handler(&matched), "get rust");

        let listed = resolver.resolve("GET", "/books").expect("match");
        assert_eq!((*listed.value())(&listed), "list");
    }
}
