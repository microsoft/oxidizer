// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime (dynamic) routing (feature `dynamic`).
//!
//! Where the [`routes!`](crate::routes) macro and `routerama_build` lower a
//! *compile-time-known* route set into a static `match`, a [`DynRouter`] resolves
//! a route set that is only known at **runtime** — read from config, a database,
//! plugins, or per-tenant registration. It does so by walking the *same*
//! [`routerama_build::trie`] the static path lowers, so a given route set resolves
//! **identically** whether static or dynamic (verified by differential tests).
//!
//! Both paths are expressed through two small traits:
//!
//! - [`Router`] — the resolver. A static generated enum's ZST router and a
//!   [`DynRouter`] both implement it.
//! - [`RouteMatch`] — the result. The generated enum and [`DynMatch`] both
//!   implement it, exposing the matched route [`name`](RouteMatch::name) and its
//!   [`capture`](RouteMatch::capture)d path variables by name.
//!
//! [`EitherRouter`] composes two routers into one, trying the primary first and
//! falling back to the secondary — e.g. a fast static core with a dynamic
//! plugin/tenant overlay.
//!
//! See the `dynamic_routing` example for a standalone runtime router, and the
//! `hybrid_routing` example for combining a static core with a dynamic overlay.

use routerama_build::RouteRule;
use routerama_build::trie::{Leaf, Node, VarPlan, affix_edges_in_match_order, build_trie};
use smallvec::SmallVec;

use crate::codegen_helpers::{scan_segments, split_verb};
use crate::route::{RouteMatch, Router};

/// Inline capacity of the per-request segment-offset scratch buffers: route sets
/// no deeper than this scan on the stack; deeper ones spill to the heap. Sized
/// generously so real APIs (rarely past a handful of segments) never allocate.
const INLINE_SEGMENTS: usize = 16;

/// A runtime router built from a route set known only at run time.
///
/// Construct it from any iterator of [`RouteRule`]s; it builds the shared
/// [`routerama_build::trie`] once and walks it per request, resolving identically
/// to the static codegen path for the same route set.
///
/// # Examples
///
/// ```
/// use routerama::{DynRouter, HttpMethod, RouteMatch as _, RouteRule, Router as _};
///
/// let router = DynRouter::new([
///     RouteRule::new(
///         "ListBooks",
///         HttpMethod::Get,
///         "/books".parse().expect("valid"),
///     ),
///     RouteRule::new(
///         "GetBook",
///         HttpMethod::Get,
///         "/books/{book}".parse().expect("valid"),
///     ),
/// ]);
///
/// let matched = router.resolve("GET", "/books/rust").expect("a match");
/// assert_eq!(matched.name(), "GetBook");
/// assert_eq!(matched.capture("book"), Some("rust"));
/// assert!(router.resolve("POST", "/books/rust").is_none());
/// ```
pub struct DynRouter {
    root: RtNode,
    max_segments: usize,
    any_verb: bool,
}

impl DynRouter {
    /// Builds a runtime router from a route set.
    #[must_use]
    pub fn new(routes: impl IntoIterator<Item = RouteRule>) -> Self {
        let routes: Vec<RouteRule> = routes.into_iter().collect();
        let trie = build_trie(&routes);
        Self {
            root: RtNode::compile(trie.root),
            max_segments: trie.max_segments,
            any_verb: trie.any_verb,
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
        // Capture the affix edges' match order as owned keys. This collect ends
        // the borrow of `node` taken by `affix_edges_in_match_order` (which yields
        // references into it) so we can then move `node.affix` out below.
        let affix_order: Vec<(String, String)> = affix_edges_in_match_order(&node).into_iter().map(|(key, _)| key.clone()).collect();
        let mut affix_map = node.affix;
        let affix = affix_order
            .into_iter()
            .map(|key| {
                let child = affix_map.remove(&key).expect("affix key from the same map");
                (key.0.into_boxed_str(), key.1.into_boxed_str(), Self::compile(child))
            })
            .collect();
        // Literal edges are matched by a linear scan in `descend`, so their order
        // is irrelevant.
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

impl core::fmt::Debug for DynRouter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DynRouter")
            .field("max_segments", &self.max_segments)
            .field("any_verb", &self.any_verb)
            .finish_non_exhaustive()
    }
}

impl Router for DynRouter {
    type Match<'p> = DynMatch<'p>;

    fn resolve<'p>(&'p self, method: &str, path: &'p str) -> Option<DynMatch<'p>> {
        // Split a trailing `:verb` only when some route declares one (matching the
        // static path, which omits the split entirely otherwise).
        let (body, verb) = if self.any_verb { split_verb(path) } else { (path, None) };

        // Segment-offset scratch buffers sized to the deepest route. A fixed stack
        // array covers the overwhelmingly common shallow case; only a
        // pathologically deep route set spills to the heap.
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

        // The walk finds the matching leaf; captures are materialized once here
        // rather than inside the recursion. The invariant request context is
        // bundled into `Walk` so descent passes only the node and depth.
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
        Some(DynMatch { leaf, values })
    }
}

/// A dynamic match: the matched route and its captured path-variable values.
///
/// The matched leaf is held by reference (so [`name`](RouteMatch::name) and the
/// capture *keys* need no copy), and only the captured *values* — which borrow
/// the request path — are stored, inline for up to four variables.
#[derive(Debug, Clone)]
pub struct DynMatch<'p> {
    leaf: &'p Leaf,
    values: SmallVec<[&'p str; 4]>,
}

impl<'p> RouteMatch<'p> for DynMatch<'p> {
    fn name(&self) -> &str {
        &self.leaf.name
    }

    fn capture(&self, name: &str) -> Option<&'p str> {
        // The captured values are positional with `leaf.vars`, so the field's
        // index locates its value.
        let index = self.leaf.vars.iter().position(|plan| plan.field() == name)?;
        self.values.get(index).copied()
    }
}

impl<'p> DynMatch<'p> {
    /// Iterates the captured `(name, value)` pairs of this match.
    pub fn captures(&self) -> impl Iterator<Item = (&'p str, &'p str)> + '_ {
        self.leaf.vars.iter().map(VarPlan::field).zip(self.values.iter().copied())
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
    /// Descends `node` at `depth`, mirroring the precedence the generated `match`
    /// encodes: literal → affix (most-specific-first) → single-`*` edges (deeper
    /// wins), then exact leaves, then the `**` catch-all. Returns the matched
    /// [`Leaf`]; the caller materializes its captures.
    fn descend(&self, node: &'p RtNode, depth: usize) -> Option<&'p Leaf> {
        if self.count > depth {
            // A segment exists at this depth: try the edges most-specific-first.
            if !node.literals.is_empty() || !node.affix.is_empty() {
                // Slice the segment from the raw bytes, not the `&str`: a `&str`
                // range index runs a char-boundary check, while these offsets are
                // known-valid `/`-delimited boundaries and the comparisons below
                // are on bytes. Capture values are sliced back as `&str`.
                let seg = &self.body.as_bytes()[self.starts[depth]..self.ends[depth]];
                // Linear scan with a length + first-byte pre-filter before the full
                // byte compare: `[u8]` equality on runtime-length slices is a
                // `memcmp` call, so cheap inline rejects keep it to the one edge
                // that can actually match.
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

/// Slices a captured variable's value out of `body`, mirroring the generated
/// `var_slice_expr`.
fn materialize<'p>(plan: &VarPlan, starts: &[usize], ends: &[usize], body: &'p str, count: usize) -> &'p str {
    match plan {
        VarPlan::Span { a, b, .. } => &body[starts[*a]..ends[*b]],
        VarPlan::Rest { a, .. } => {
            if *a < count {
                &body[starts[*a]..]
            } else {
                ""
            }
        }
        VarPlan::Affix {
            a, prefix_len, suffix_len, ..
        } => &body[starts[*a] + *prefix_len..ends[*a] - *suffix_len],
    }
}

/// Composes two routers into one, trying `primary` first and falling back to
/// `secondary` — e.g. a fast static core with a dynamic plugin/tenant overlay.
///
/// The fallback picks a *fixed* winner on any path both could match ("primary
/// wins"), rather than merging the two route sets into one globally
/// precedence-ranked structure. It is therefore correct when the two sets are
/// precedence-independent — disjoint, or one owning a distinct path subtree.
///
/// # Examples
///
/// ```
/// use routerama::{DynRouter, EitherRouter, HttpMethod, RouteMatch as _, RouteRule, Router as _};
///
/// let core = DynRouter::new([RouteRule::new(
///     "Home",
///     HttpMethod::Get,
///     "/".parse().expect("valid"),
/// )]);
/// let plugins = DynRouter::new([RouteRule::new(
///     "Plugin",
///     HttpMethod::Get,
///     "/plugins/{name}".parse().expect("valid"),
/// )]);
/// let router = EitherRouter::new(core, plugins);
///
/// assert_eq!(
///     router
///         .resolve("GET", "/plugins/auth")
///         .expect("match")
///         .name(),
///     "Plugin"
/// );
/// assert_eq!(router.resolve("GET", "/").expect("match").name(), "Home");
/// ```
#[derive(Debug, Clone)]
pub struct EitherRouter<A, B> {
    primary: A,
    secondary: B,
}

impl<A, B> EitherRouter<A, B> {
    /// Composes `primary` (tried first) with `secondary` (the fallback).
    pub const fn new(primary: A, secondary: B) -> Self {
        Self { primary, secondary }
    }
}

/// The match of an [`EitherRouter`]: the primary's match or the secondary's.
#[derive(Debug, Clone)]
pub enum EitherMatch<L, R> {
    /// The primary router matched.
    Left(L),
    /// The secondary (fallback) router matched.
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

impl<A: Router, B: Router> Router for EitherRouter<A, B> {
    type Match<'p>
        = EitherMatch<A::Match<'p>, B::Match<'p>>
    where
        Self: 'p;

    fn resolve<'p>(&'p self, method: &str, path: &'p str) -> Option<Self::Match<'p>> {
        if let Some(matched) = self.primary.resolve(method, path) {
            return Some(EitherMatch::Left(matched));
        }
        self.secondary.resolve(method, path).map(EitherMatch::Right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materialize_rest_captures_the_remainder_or_nothing() {
        // `body = "/files/a/b"`: segments `files`@[1,6], `a`@[7,8], `b`@[9,10].
        let body = "/files/a/b";
        let starts = [1, 7, 9];
        let ends = [6, 8, 10];
        let plan = VarPlan::Rest {
            field: "path".to_owned(),
            a: 1,
        };
        // `a < count`: the `**` spans from segment 1 to the end of the path.
        assert_eq!(materialize(&plan, &starts, &ends, body, 3), "a/b");
        // `a == count`: no remaining segments, so the `**` captures nothing (pins
        // the boundary at `a < count`, distinguishing it from `<=`, `==`, `>`).
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
            a: 0,
            prefix_len: 4,
            suffix_len: 4,
        };
        assert_eq!(materialize(&plan, &starts, &ends, body, 1), "cat");
    }
}
