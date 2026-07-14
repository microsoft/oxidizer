// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;

use routerama_build::Route;
use routerama_build::trie::{Leaf, Node, VarPlan, affix_edges_in_match_order, build_trie};
use smallvec::SmallVec;

use crate::codegen_helpers::{scan_segments, seg_bytes, split_verb, substr};
use crate::route::{Resolver, RouteMatch};

/// Inline capacity of the per-request segment-offset scratch buffers: route sets
/// no deeper than this scan on the stack; deeper ones spill to the heap. Sized
/// generously so real APIs (rarely past a handful of segments) never allocate.
const INLINE_SEGMENTS: usize = 16;

/// A runtime resolver built from a route set known only at run time.
///
/// Construct it from any iterator of [`Route`]s ([`new`](Self::new)), or from
/// `(route, value)` pairs ([`with_values`](Self::with_values)) to attach a value
/// — typically a handler — to each route. It builds the shared
/// [`routerama_build::trie`] once and walks it per request, resolving identically
/// to the static codegen path for the same route set.
///
/// A match returns the attached value directly via [`DynMatch::value`], so
/// dispatch needs no by-name lookup.
///
/// # Examples
///
/// ```
/// use http_path_template::{Grammar, PathTemplate};
/// use routerama::{DynResolver, HttpMethod, Resolver as _, Route, RouteMatch as _};
///
/// // Attach a handler index to each route; a match hands it straight back.
/// let resolver = DynResolver::with_values([
///     (
///         Route::new(
///             "ListBooks",
///             HttpMethod::Get,
///             PathTemplate::parse("/books", Grammar::default()).expect("valid"),
///         ),
///         0_usize,
///     ),
///     (
///         Route::new(
///             "GetBook",
///             HttpMethod::Get,
///             PathTemplate::parse("/books/{book}", Grammar::default()).expect("valid"),
///         ),
///         1_usize,
///     ),
/// ]);
///
/// let matched = resolver.resolve("GET", "/books/rust").expect("a match");
/// assert_eq!(*matched.value(), 1);
/// assert_eq!(matched.name(), "GetBook");
/// assert_eq!(matched.capture("book"), Some("rust"));
/// assert!(resolver.resolve("POST", "/books/rust").is_none());
/// ```
pub struct DynResolver<T = ()> {
    root: RtNode,
    max_segments: usize,
    any_verb: bool,
    /// One value per input route, indexed by [`Leaf::route_index`]. `()` when no
    /// per-route value was attached (see [`new`](Self::new)).
    payloads: Box<[T]>,
}

impl DynResolver<()> {
    /// Builds a runtime resolver from a route set, with no per-route value.
    ///
    /// A match exposes the route [`name`](RouteMatch::name) and captures; to also
    /// attach a value (e.g. a handler) to each route, use
    /// [`with_values`](DynResolver::with_values).
    #[must_use]
    pub fn new(routes: impl IntoIterator<Item = Route>) -> Self {
        let routes: Vec<Route> = routes.into_iter().collect();
        let payloads = vec![(); routes.len()].into_boxed_slice();
        Self::build(&routes, payloads)
    }
}

impl<T> DynResolver<T> {
    /// Builds a runtime resolver from `(route, value)` pairs, attaching a value —
    /// typically a handler to invoke — to each route.
    ///
    /// A match returns the value of the matched route directly via
    /// [`DynMatch::value`], so dispatch needs no by-name lookup: the value you
    /// registered is handed straight back.
    #[must_use]
    pub fn with_values(entries: impl IntoIterator<Item = (Route, T)>) -> Self {
        let (routes, payloads): (Vec<Route>, Vec<T>) = entries.into_iter().unzip();
        Self::build(&routes, payloads.into_boxed_slice())
    }

    /// Builds the trie and stores the per-route payload table (index-aligned with
    /// the routes, so [`Leaf::route_index`] locates each route's value).
    fn build(routes: &[Route], payloads: Box<[T]>) -> Self {
        let trie = build_trie(routes);
        Self {
            root: RtNode::compile(trie.root),
            max_segments: trie.max_segments,
            any_verb: trie.any_verb,
            payloads,
        }
    }
}

/// A literal edge: an exact segment key and the subtree it leads to.
type LiteralEdge = (Box<str>, RtNode);

/// An affix edge: a `prefix`, a `suffix`, and the subtree matched when a segment
/// both starts with the prefix and ends with the suffix.
type AffixEdge = (Box<str>, Box<str>, RtNode);

/// A runtime node, compiled from the shared [`Node`] trie into a flat,
/// cache-friendly shape: literal edges in a contiguous slice, affix edges in
/// their match order, and leaves moved in. The shape mirrors the trie it is
/// built from, so matching stays identical to the static path.
struct RtNode {
    /// Literal edges, linear-scanned per segment (see [`Walk::descend`]).
    literals: Box<[LiteralEdge]>,
    /// Affix edges, in most-specific-first match order.
    affix: Box<[AffixEdge]>,
    single: Option<Box<Self>>,
    exact: Box<[Leaf]>,
    rest: Box<[Leaf]>,
}

impl RtNode {
    #[expect(
        clippy::needless_collect,
        reason = "the collect ends a borrow of `node` so `node.affix` can be moved"
    )]
    fn compile(node: Node) -> Self {
        // Own the affix match order first; the collect ends the borrow of `node`
        // so `node.affix` can be moved out below.
        let affix_order: Vec<(String, String)> = affix_edges_in_match_order(&node).into_iter().map(|(key, _)| key.clone()).collect();
        let mut affix_map = node.affix;
        let affix = affix_order
            .into_iter()
            .map(|key| {
                let child = affix_map
                    .remove(&key)
                    .expect("affix_edges_in_match_order enumerated this key from node.affix, and each key appears once so it has not been removed by a prior iteration");
                (key.0.into_boxed_str(), key.1.into_boxed_str(), Self::compile(child))
            })
            .collect();
        // Literal edges are linear-scanned in `descend`, so their order is irrelevant.
        let literals = node
            .literals
            .into_iter()
            .map(|(key, child)| (key.into_boxed_str(), Self::compile(child)))
            .collect();
        Self {
            literals,
            affix,
            single: node.single.map(|child| Box::new(Self::compile(*child))),
            exact: node.exact.into_boxed_slice(),
            rest: node.rest.into_boxed_slice(),
        }
    }
}

impl<T> fmt::Debug for DynResolver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DynResolver")
            .field("routes", &self.payloads.len())
            .field("max_segments", &self.max_segments)
            .field("any_verb", &self.any_verb)
            .finish_non_exhaustive()
    }
}

impl<T> Resolver for DynResolver<T> {
    type Match<'p>
        = DynMatch<'p, T>
    where
        Self: 'p;

    #[inline]
    fn resolve<'p, P>(&'p self, method: impl AsRef<str>, path: &'p P) -> Option<DynMatch<'p, T>>
    where
        P: AsRef<str> + ?Sized,
    {
        let method = method.as_ref();
        let path = path.as_ref();
        // Only split a trailing `:verb` when some route declares one.
        let (body, verb) = if self.any_verb { split_verb(path) } else { (path, None) };

        // Segment-offset scratch buffers sized to the deepest route: a stack array
        // for the common shallow case, spilling to the heap only for a very deep
        // route set.
        let mut inline_starts = [0_usize; INLINE_SEGMENTS];
        let mut inline_ends = [0_usize; INLINE_SEGMENTS];
        let mut heap_starts;
        let mut heap_ends;
        let (starts, ends): (&mut [usize], &mut [usize]) = if self.max_segments <= INLINE_SEGMENTS {
            (&mut inline_starts[..self.max_segments], &mut inline_ends[..self.max_segments])
        } else {
            heap_starts = vec![0_usize; self.max_segments];
            heap_ends = vec![0_usize; self.max_segments];
            (&mut heap_starts, &mut heap_ends)
        };
        let count = scan_segments(body, starts, ends);
        let (starts, ends): (&[usize], &[usize]) = (starts, ends);

        let walk = Walk {
            count,
            starts,
            ends,
            body,
            method,
            verb,
            any_verb: self.any_verb,
        };
        let leaf = walk.descend(&self.root, 0)?;
        let mut values: SmallVec<[&'p str; 4]> = SmallVec::with_capacity(leaf.vars.len());
        for plan in &leaf.vars {
            values.push(materialize(plan, starts, ends, body, count));
        }
        // `route_index` is in-bounds by construction: each leaf carries the index
        // of the route that created it, and `payloads` holds one entry per input
        // route, so the two are always aligned.
        let value = &self.payloads[leaf.route_index];
        Some(DynMatch { leaf, values, value })
    }
}

/// The outcome of resolving one request against a [`DynResolver`].
///
/// This caries the route that matched, everything captured from the path, and the value you
/// attached to that route when creating it.
///
/// You get a `DynMatch` from [`DynResolver::resolve`] (wrapped in `Option`, so
/// `None` means no route matched).
///
/// Once you have one, three things are available:
///
/// - [`value`](Self::value) — the value you registered for this route with
///   [`DynResolver::with_values`], typically a handler to invoke. This is how you
///   dispatch: the matched route hands its handler straight back, with no
///   by-name lookup. (For a resolver built with [`DynResolver::new`], the value
///   is `()`.)
/// - [`name`](RouteMatch::name) — the route's registered name, mostly used for telemetry.
/// - [`capture`](RouteMatch::capture) / [`captures`](Self::captures) /
///   [`captures_ordered`](Self::captures_ordered) — the path variables the route
///   captured (`{book}` → `"rust"`).
pub struct DynMatch<'p, T = ()> {
    leaf: &'p Leaf,
    values: SmallVec<[&'p str; 4]>,
    value: &'p T,
}

impl<T> Clone for DynMatch<'_, T> {
    fn clone(&self) -> Self {
        Self {
            leaf: self.leaf,
            values: self.values.clone(),
            value: self.value,
        }
    }
}

impl<T> fmt::Debug for DynMatch<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DynMatch")
            .field("name", &self.name())
            .field("captures", &self.captures().collect::<SmallVec<[_; 4]>>())
            .finish_non_exhaustive()
    }
}

impl<'p, T> RouteMatch<'p> for DynMatch<'p, T> {
    #[inline]
    fn name(&self) -> &str {
        &self.leaf.name
    }

    #[inline]
    fn capture(&self, name: &str) -> Option<&'p str> {
        // The captured values are positional with `leaf.vars`, so the variable's
        // index locates its value. Captures are keyed by the variable's original
        // name (e.g. `shelf.id`, `type`), not the sanitized field identifier.
        let index = self.leaf.vars.iter().position(|plan| plan.key() == name)?;
        self.values.get(index).copied()
    }
}

impl<'p, T> DynMatch<'p, T> {
    /// The value attached to the matched route at registration via
    /// [`DynResolver::with_values`] — typically a handler to invoke.
    ///
    /// For a resolver built with [`DynResolver::new`] (no attached values) this is
    /// `&()`.
    #[inline]
    #[must_use]
    pub fn value(&self) -> &'p T {
        self.value
    }

    /// Iterates the captured `(name, value)` pairs of this match.
    ///
    /// # Ordering contract
    ///
    /// Pairs are yielded in the matched route's **template variable declaration
    /// order** — the left-to-right order the `{variables}` appear in the route's
    /// path template. This order is stable and matches
    /// [`captures_ordered`](Self::captures_ordered) and the registration-time
    /// order of [`routerama_build::trie::capture_field_names`], so a higher-level
    /// crate can precompute a name-to-slot plan at registration and apply it
    /// positionally at match time.
    #[inline]
    pub fn captures(&self) -> impl Iterator<Item = (&'p str, &'p str)> + '_ {
        self.leaf.vars.iter().map(VarPlan::key).zip(self.values.iter().copied())
    }

    /// The captured values in template variable declaration order.
    ///
    /// This is the zero-lookup accessor for typed extraction: a caller that
    /// resolved variable names to slot indices at registration (see the ordering
    /// contract on [`captures`](Self::captures)) can index straight into this
    /// slice, skipping the per-access name scan that [`capture`](RouteMatch::capture)
    /// performs.
    ///
    /// # Ordering contract
    ///
    /// Element `i` is the value of the `i`-th `{variable}` in the matched route's
    /// path template, in left-to-right declaration order. This order is stable and
    /// aligns with [`captures`](Self::captures) and the registration-time order of
    /// [`routerama_build::trie::capture_field_names`].
    #[inline]
    #[must_use]
    pub fn captures_ordered(&self) -> &[&'p str] {
        &self.values
    }
}

/// The invariant per-request context threaded through the trie descent, so
/// [`descend`](Walk::descend) passes only the node and depth per level.
struct Walk<'a, 'p> {
    count: usize,
    starts: &'a [usize],
    ends: &'a [usize],
    body: &'p str,
    method: &'a str,
    verb: Option<&'a str>,
    any_verb: bool,
}

impl<'p> Walk<'_, 'p> {
    /// Whether a path segment exists at `depth` (segments are indexed
    /// `0..self.count`).
    //
    // `mutants::skip`: `>` vs `>=`/`==` is equivalent here — overshooting to
    // `depth == self.count` reads a zeroed offset slot, an empty segment that
    // matches no edge, so it is a no-op.
    #[cfg_attr(test, mutants::skip)]
    fn has_segment_at(&self, depth: usize) -> bool {
        self.count > depth
    }

    /// Descends `node` at `depth`, following match precedence: literal → affix
    /// (most-specific-first) → single-`*` edges (deeper wins), then exact leaves,
    /// then the `**` catch-all. Returns the matched [`Leaf`]; the caller
    /// materializes its captures.
    fn descend(&self, node: &'p RtNode, depth: usize) -> Option<&'p Leaf> {
        if self.has_segment_at(depth) {
            if !node.literals.is_empty() || !node.affix.is_empty() {
                // Segment offsets are only read at edge-bearing nodes, and the
                // trie has edges only along atom chains no deeper than
                // `max_segments`, so `depth` is always within the offset buffers.
                debug_assert!(depth < self.starts.len(), "segment read past the offset buffer");
                let seg = seg_bytes(self.body, self.starts[depth], self.ends[depth]);
                // A length + first-byte pre-filter keeps the `memcmp` slice
                // compare to the one literal edge that can actually match.
                if let Some((_, child)) = node.literals.iter().find(|(key, _)| {
                    let key = key.as_bytes();
                    key.len() == seg.len() && key.first() == seg.first() && key == seg
                }) && let Some(matched) = self.descend(child, depth + 1)
                {
                    return Some(matched);
                }
                for (prefix, suffix, child) in &node.affix {
                    if seg.len() > prefix.len() + suffix.len()
                        && seg.starts_with(prefix.as_bytes())
                        && seg.ends_with(suffix.as_bytes())
                        && let Some(matched) = self.descend(child, depth + 1)
                    {
                        return Some(matched);
                    }
                }
            }
            if let Some(child) = &node.single {
                debug_assert!(depth < self.starts.len(), "single-edge read past the offset buffer");
                // A single-segment wildcard matches exactly one non-empty segment.
                if self.ends[depth] > self.starts[depth]
                    && let Some(matched) = self.descend(child, depth + 1)
                {
                    return Some(matched);
                }
            }
        }
        if self.count == depth
            && !node.exact.is_empty()
            && let Some(matched) = self.dispatch(&node.exact)
        {
            return Some(matched);
        }
        if self.count >= depth
            && !node.rest.is_empty()
            && let Some(matched) = self.dispatch(&node.rest)
        {
            return Some(matched);
        }
        None
    }

    /// Selects the first leaf whose method (and, when any route uses verbs, custom
    /// verb) matches.
    fn dispatch(&self, leaves: &'p [Leaf]) -> Option<&'p Leaf> {
        leaves
            .iter()
            .find(|leaf| leaf.method == self.method && (!self.any_verb || leaf.verb.as_deref() == self.verb))
    }
}

/// Slices a captured variable's value out of `body`.
#[inline]
fn materialize<'p>(plan: &VarPlan, starts: &[usize], ends: &[usize], body: &'p str, count: usize) -> &'p str {
    match plan {
        VarPlan::Span { a, b, .. } => substr(body, starts[*a], ends[*b]),
        VarPlan::Rest { a, .. } => {
            if *a < count {
                substr(body, starts[*a], body.len())
            } else {
                ""
            }
        }
        VarPlan::Affix {
            a, prefix_len, suffix_len, ..
        } => substr(body, starts[*a] + *prefix_len, ends[*a] - *suffix_len),
    }
}

/// Composes two resolvers into one, trying `primary` first and falling back to
/// `secondary`.
///
/// The fallback picks a *fixed* winner on any path both could match ("primary
/// wins"), rather than merging the two route sets into one globally
/// precedence-ranked structure. It is therefore correct when the two sets are
/// precedence-independent — disjoint, or one owning a distinct path subtree.
///
/// # Examples
///
/// ```
/// use http_path_template::{Grammar, PathTemplate};
/// use routerama::{
///     DynResolver, EitherResolver, HttpMethod, Resolver as _, Route, RouteMatch as _,
/// };
///
/// let core = DynResolver::new([Route::new(
///     "Home",
///     HttpMethod::Get,
///     PathTemplate::parse("/", Grammar::default()).expect("valid"),
/// )]);
/// let plugins = DynResolver::new([Route::new(
///     "Plugin",
///     HttpMethod::Get,
///     PathTemplate::parse("/plugins/{name}", Grammar::default()).expect("valid"),
/// )]);
/// let resolver = EitherResolver::new(core, plugins);
///
/// assert_eq!(
///     resolver
///         .resolve("GET", "/plugins/auth")
///         .expect("match")
///         .name(),
///     "Plugin"
/// );
/// assert_eq!(resolver.resolve("GET", "/").expect("match").name(), "Home");
/// ```
#[derive(Debug, Clone)]
pub struct EitherResolver<A, B> {
    primary: A,
    secondary: B,
}

impl<A, B> EitherResolver<A, B> {
    /// Composes `primary` (tried first) with `secondary` (the fallback).
    pub const fn new(primary: A, secondary: B) -> Self {
        Self { primary, secondary }
    }
}

/// The match of an [`EitherResolver`]: the primary's match or the secondary's.
#[derive(Debug, Clone)]
pub enum EitherMatch<L, R> {
    /// The primary resolver matched.
    Left(L),

    /// The secondary (fallback) resolver matched.
    Right(R),
}

impl<'p, L: RouteMatch<'p>, R: RouteMatch<'p>> RouteMatch<'p> for EitherMatch<L, R> {
    fn name(&self) -> &str {
        match self {
            Self::Left(left) => left.name(),
            Self::Right(right) => right.name(),
        }
    }

    fn capture(&self, name: &str) -> Option<&'p str> {
        match self {
            Self::Left(left) => left.capture(name),
            Self::Right(right) => right.capture(name),
        }
    }
}

impl<A: Resolver, B: Resolver> Resolver for EitherResolver<A, B> {
    type Match<'p>
        = EitherMatch<A::Match<'p>, B::Match<'p>>
    where
        Self: 'p;

    fn resolve<'p, P>(&'p self, method: impl AsRef<str>, path: &'p P) -> Option<Self::Match<'p>>
    where
        P: AsRef<str> + ?Sized,
    {
        let method = method.as_ref();
        let path = path.as_ref();
        if let Some(matched) = self.primary.resolve(method, path) {
            return Some(EitherMatch::Left(matched));
        }
        self.secondary.resolve(method, path).map(EitherMatch::Right)
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use http_path_template::{Grammar, PathTemplate};

    use super::*;
    use crate::HttpMethod;

    #[test]
    fn materialize_rest_captures_the_remainder_or_nothing() {
        // `body = "/files/a/b"`: segments `files`@[1,6], `a`@[7,8], `b`@[9,10].
        let body = "/files/a/b";
        let starts = [1, 7, 9];
        let ends = [6, 8, 10];
        let plan = VarPlan::Rest {
            field: "path".to_owned(),
            key: "path".to_owned(),
            a: 1,
        };
        // `a < count`: the `**` spans from segment 1 to the end of the path.
        assert_eq!(materialize(&plan, &starts, &ends, body, 3), "a/b");
        // `a == count`: no remaining segments, so the `**` captures nothing.
        assert_eq!(materialize(&plan, &starts, &ends, body, 1), "");
    }

    #[test]
    fn materialize_affix_strips_the_prefix_and_suffix() {
        // `body = "/img-cat.png"`: one segment `img-cat.png`@[1,12], with a 4-byte
        // prefix (`img-`) and 4-byte suffix (`.png`) wrapping the `cat` capture.
        let body = "/img-cat.png";
        let starts = [1];
        let ends = [12];
        let plan = VarPlan::Affix {
            field: "id".to_owned(),
            key: "id".to_owned(),
            a: 0,
            prefix_len: 4,
            suffix_len: 4,
        };
        assert_eq!(materialize(&plan, &starts, &ends, body, 1), "cat");
    }

    #[test]
    fn deep_paths_beyond_max_segments_return_none_without_panicking() {
        // Each resolver's `max_segments` is small; every lookup path is far deeper.
        // A too-deep path must find no edge and return `None` (or match a `**`
        // leaf) rather than index past the fixed offset buffers.
        let mk = |name: &str, pattern: &str| {
            Route::new(
                name,
                HttpMethod::Get,
                PathTemplate::parse(pattern, Grammar::default().with_segment_affixes()).expect("valid template"),
            )
        };
        let route_sets: &[&[(&str, &str)]] = &[
            &[("Lit", "/a/b/c")],
            &[("Single", "/{a}/{b}/{c}")],
            &[("Short", "/a/{x}"), ("Long", "/a/{x}/{y}/{z}")],
            &[("Rest", "/a/**"), ("Var", "/a/{x}")],
            &[("Affix", "/img-{id}.png"), ("Var", "/a/{x}")],
            &[("SingleRest", "/{a}/{b}/**")],
        ];
        for routes in route_sets {
            let router = DynResolver::new(routes.iter().map(|(name, pattern)| mk(name, pattern)));
            for depth in 0..40 {
                let mut path = String::from("/a");
                for i in 0..depth {
                    let _ = write!(path, "/s{i}");
                }
                // None of these may panic, regardless of how deep the path is.
                let _ = router.resolve("GET", &path);
                let _ = router.resolve("GET", &format!("{path}/"));
                let _ = router.resolve("POST", &format!("{path}:verb"));
            }
            // A pathologically deep path is still handled gracefully.
            let mut very_deep = String::new();
            for i in 0..500 {
                let _ = write!(very_deep, "/{i}");
            }
            let _ = router.resolve("GET", &very_deep);
        }
    }

    #[test]
    fn compile_builds_affix_edges_and_resolves_them() {
        // A route set with affix segments (`img-{id}.png`, `thumb-{id}.jpg`).
        let router = DynResolver::new([
            Route::new(
                "Image",
                HttpMethod::Get,
                PathTemplate::parse("/img-{id}.png", Grammar::default().with_segment_affixes()).expect("valid template"),
            ),
            Route::new(
                "Thumb",
                HttpMethod::Get,
                PathTemplate::parse("/thumb-{id}.jpg", Grammar::default().with_segment_affixes()).expect("valid template"),
            ),
        ]);
        let matched = router.resolve("GET", "/img-42.png").expect("affix route matches");
        assert_eq!(matched.name(), "Image");
        assert_eq!(matched.capture("id"), Some("42"));
        // The empty-middle case does not match (the capture must be non-empty).
        assert!(router.resolve("GET", "/img-.png").is_none());
    }

    #[test]
    fn debug_reports_the_router_shape() {
        let router = DynResolver::new([Route::new(
            "Get",
            HttpMethod::Get,
            PathTemplate::parse("/a/{x}", Grammar::default()).expect("valid template"),
        )]);
        let debug = format!("{router:?}");
        assert!(debug.contains("DynResolver"), "{debug}");
        assert!(debug.contains("max_segments"), "{debug}");
        assert!(debug.contains("any_verb"), "{debug}");
    }

    #[test]
    fn deep_route_set_spills_the_offset_buffers_to_the_heap() {
        // A route deeper than `INLINE_SEGMENTS` spills the offset buffers to the
        // heap; it must still resolve correctly.
        let depth = INLINE_SEGMENTS + 4;
        let mut template = String::new();
        for i in 0..depth {
            let _ = write!(template, "/{{v{i}}}");
        }
        let router = DynResolver::new([Route::new(
            "Deep",
            HttpMethod::Get,
            PathTemplate::parse(&template, Grammar::default()).expect("valid template"),
        )]);

        let mut path = String::new();
        for i in 0..depth {
            let _ = write!(path, "/seg{i}");
        }
        let matched = router.resolve("GET", &path).expect("deep route matches through the heap buffers");
        assert_eq!(matched.name(), "Deep");
        // A capture from the heap-backed offsets is materialized correctly.
        assert_eq!(matched.capture("v0"), Some("seg0"));
        assert_eq!(
            matched.capture(&format!("v{}", depth - 1)),
            Some(format!("seg{}", depth - 1).as_str())
        );
    }

    #[test]
    fn descend_matches_literal_then_affix_edges_at_the_same_node() {
        // Under `/files`, one node carries both a literal edge (`data`) and an
        // affix edge (`img-{id}.png`).
        let mk = |name: &str, pattern: &str| {
            Route::new(
                name,
                HttpMethod::Get,
                PathTemplate::parse(pattern, Grammar::default().with_segment_affixes()).expect("valid template"),
            )
        };
        let router = DynResolver::new([mk("Data", "/files/data"), mk("Image", "/files/img-{id}.png")]);

        // Literal edge wins.
        let data = router.resolve("GET", "/files/data").expect("literal route matches");
        assert_eq!(data.name(), "Data");
        assert_eq!(data.captures().count(), 0);

        // The literal is rejected by the pre-filter, so the affix edge matches.
        let image = router.resolve("GET", "/files/img-7.png").expect("affix route matches");
        assert_eq!(image.name(), "Image");
        assert_eq!(image.capture("id"), Some("7"));

        // A segment matching neither the literal nor the affix misses.
        assert!(router.resolve("GET", "/files/other").is_none());
    }

    #[test]
    fn affix_length_guard_and_descent_depth_are_pinned() {
        // An affix route under `/api` with a 3-byte prefix (`ab-`) and 2-byte
        // suffix (`.x`) wrapping the capture.
        let router = DynResolver::new([Route::new(
            "Tagged",
            HttpMethod::Get,
            PathTemplate::parse("/api/ab-{id}.x", Grammar::default().with_segment_affixes()).expect("valid template"),
        )]);

        // A non-empty middle matches and is captured.
        let matched = router
            .resolve("GET", "/api/ab-7.x")
            .expect("affix route matches a non-empty middle");
        assert_eq!(matched.name(), "Tagged");
        assert_eq!(matched.capture("id"), Some("7"));

        // An empty middle is rejected: the capture must be non-empty.
        assert!(router.resolve("GET", "/api/ab-.x").is_none());
    }

    #[test]
    fn literal_prefilter_still_verifies_full_segment_content() {
        // Two root literals with the same length and first byte but different
        // content (`car`, `cat`). The pre-filter must still compare the whole
        // segment: otherwise `/cat/x` picks `car`, descends, and misses without
        // backtracking.
        let mk = |name: &str, pattern: &str| {
            Route::new(
                name,
                HttpMethod::Get,
                PathTemplate::parse(pattern, Grammar::default()).expect("valid template"),
            )
        };
        let router = DynResolver::new([mk("Car", "/car/y"), mk("Cat", "/cat/x")]);

        let cat = router.resolve("GET", "/cat/x").expect("cat route matches");
        assert_eq!(cat.name(), "Cat");

        let car = router.resolve("GET", "/car/y").expect("car route matches");
        assert_eq!(car.name(), "Car");
    }

    #[test]
    fn captures_yields_every_name_value_pair_in_order() {
        // `captures()` yields each `(name, value)` pair in order.
        let router = DynResolver::new([Route::new(
            "Review",
            HttpMethod::Get,
            PathTemplate::parse("/books/{book}/reviews/{review}", Grammar::default()).expect("valid template"),
        )]);
        let matched = router.resolve("GET", "/books/rust/reviews/42").expect("match");
        let pairs: Vec<(&str, &str)> = matched.captures().collect();
        assert_eq!(pairs, vec![("book", "rust"), ("review", "42")]);
        // `captures_ordered` is the positional counterpart, in the same order.
        assert_eq!(matched.captures_ordered(), &["rust", "42"]);
    }

    #[test]
    fn with_values_hands_back_the_registered_value() {
        // Each route carries a caller-supplied value; a match returns that value
        // directly (indexed by the leaf's `route_index`), with no by-name lookup.
        let mk = |name: &str, pattern: &str| {
            Route::new(
                name,
                HttpMethod::Get,
                PathTemplate::parse(pattern, Grammar::default()).expect("valid template"),
            )
        };
        let resolver = DynResolver::with_values([(mk("ListBooks", "/books"), 10_u32), (mk("GetBook", "/books/{book}"), 20_u32)]);

        let hit = resolver.resolve("GET", "/books/rust").expect("match");
        assert_eq!(*hit.value(), 20);
        assert_eq!(hit.name(), "GetBook");
        assert_eq!(hit.capture("book"), Some("rust"));

        assert_eq!(*resolver.resolve("GET", "/books").expect("match").value(), 10);
        assert!(resolver.resolve("POST", "/books").is_none());
    }

    #[test]
    fn stored_handlers_dispatch_without_a_by_name_lookup() {
        // The value can be a handler; the match hands it back so the caller invokes
        // it directly. A uniform `fn(&dyn RouteMatch) -> String` lets routes with
        // different captures share one table.
        type Handler = fn(&dyn RouteMatch<'_>) -> String;
        let mk = |name: &str, pattern: &str| {
            Route::new(
                name,
                HttpMethod::Get,
                PathTemplate::parse(pattern, Grammar::default()).expect("valid template"),
            )
        };
        let resolver = DynResolver::with_values([
            (mk("ListBooks", "/books"), (|_m| "list".to_owned()) as Handler),
            (
                mk("GetBook", "/books/{book}"),
                (|m| format!("get {}", m.capture("book").unwrap_or("?"))) as Handler,
            ),
        ]);

        let matched = resolver.resolve("GET", "/books/rust").expect("match");
        let handler = *matched.value();
        assert_eq!(handler(&matched), "get rust");

        // The capture-less route reaches its own handler too.
        let listed = resolver.resolve("GET", "/books").expect("match");
        assert_eq!((*listed.value())(&listed), "list");
    }

    #[test]
    fn dyn_match_is_cloneable_and_debuggable() {
        // `DynMatch`'s manual `Clone`/`Debug` impls hold regardless of `T` (here a
        // non-`Clone`, non-`Debug` payload), since the match stores only
        // references and an owned `SmallVec` of `&str`.
        struct NoDerives;
        let mk = |name: &str, pattern: &str| {
            Route::new(
                name,
                HttpMethod::Get,
                PathTemplate::parse(pattern, Grammar::default()).expect("valid template"),
            )
        };
        let resolver = DynResolver::with_values([(mk("GetBook", "/books/{book}"), NoDerives)]);

        let matched = resolver.resolve("GET", "/books/rust").expect("match");
        let cloned = matched.clone();
        assert_eq!(cloned.name(), "GetBook");
        assert_eq!(cloned.capture("book"), Some("rust"));

        let debug = format!("{matched:?}");
        assert!(debug.contains("DynMatch"), "{debug}");
        assert!(debug.contains("GetBook"), "{debug}");
        assert!(debug.contains("book"), "{debug}");
    }

    #[test]
    fn either_match_name_and_capture_reach_both_arms() {
        let mk = |name: &str, pattern: &str| {
            Route::new(
                name,
                HttpMethod::Get,
                PathTemplate::parse(pattern, Grammar::default()).expect("valid template"),
            )
        };
        // Disjoint primary/secondary routers so each side wins for a distinct path.
        let primary = DynResolver::new([mk("Book", "/books/{book}")]);
        let secondary = DynResolver::new([mk("Plugin", "/plugins/{name}")]);
        let router = EitherResolver::new(primary, secondary);

        // Primary wins -> `EitherMatch::Left`.
        let left = router.resolve("GET", "/books/rust").expect("primary match");
        assert_eq!(left.name(), "Book");
        assert_eq!(left.capture("book"), Some("rust"));

        // Secondary wins -> `EitherMatch::Right`.
        let right = router.resolve("GET", "/plugins/auth").expect("secondary match");
        assert_eq!(right.name(), "Plugin");
        assert_eq!(right.capture("name"), Some("auth"));
        assert_eq!(right.capture("missing"), None);
    }
}
