// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Framework-neutral routing trie shared by static code generation and runtime
//! resolution.
//!
//! Edge ordering implements literal, affix, single-segment, and catch-all
//! precedence without depending on code-generation crates.

use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::{format, vec};
use core::fmt::Write as _;

use http_path_template::{PathTemplate, Segment};

use crate::route::Route;

/// A flattened path-template element, with variables expanded into their
/// constituent atoms.
enum Atom {
    Literal(String),
    Single,
    Rest,
    /// An intra-segment parameter: a single segment shaped `prefix{var}suffix`.
    Affix {
        prefix: String,
        suffix: String,
    },
}

/// A variable's capture span over the flattened atom sequence.
struct VarSpan {
    name: Vec<String>,
    first: usize,
    last: usize,
    /// For an affix parameter, the byte lengths of the literal prefix/suffix
    /// wrapping the capture within its single segment.
    affix: Option<(usize, usize)>,
}

/// A plan for materializing one captured path variable at a leaf.
///
/// Each variant carries two pre-computed names so neither backend re-derives
/// them per match: `field` is the sanitized Rust identifier the static codegen
/// uses for the variable's struct-variant field (see [`route_field_name`]), and
/// `key` is the variable's original (unmangled) name, which both backends use as
/// the runtime capture key.
#[derive(Debug)]
pub enum VarPlan {
    /// A value spanning atoms `a..=b`, sliced from the path as
    /// `&body[starts[a]..ends[b]]`.
    Span { field: String, key: String, a: usize, b: usize },
    /// A `**` capture from atom `a` to the end of the path.
    Rest { field: String, key: String, a: usize },
    /// An intra-segment capture: the middle of segment `a`, sliced as
    /// `&body[starts[a] + prefix_len .. ends[a] - suffix_len]`.
    Affix {
        field: String,
        key: String,
        a: usize,
        prefix_len: usize,
        suffix_len: usize,
    },
}

impl VarPlan {
    /// The sanitized Rust identifier the static codegen uses for this variable's
    /// struct-variant field.
    #[must_use]
    #[inline]
    pub fn field(&self) -> &str {
        match self {
            Self::Span { field, .. } | Self::Rest { field, .. } | Self::Affix { field, .. } => field,
        }
    }

    /// This variable's original (unmangled) name, used as the runtime `capture`
    /// key by both backends.
    #[must_use]
    #[inline]
    pub fn key(&self) -> &str {
        match self {
            Self::Span { key, .. } | Self::Rest { key, .. } | Self::Affix { key, .. } => key,
        }
    }
}

/// A route that terminates at a trie node: how to dispatch it and what to bind.
#[derive(Debug)]
pub struct Leaf {
    pub method: String,
    pub verb: Option<String>,
    pub name: String,
    pub vars: Vec<VarPlan>,
    /// The position of the route that produced this leaf in the input route set,
    /// so a runtime backend can index a parallel per-route payload table. The
    /// static (codegen) backend ignores it.
    pub route_index: usize,
}

/// A node of the routing trie.
#[derive(Debug, Default)]
pub struct Node {
    /// Literal-segment edges, keyed by the literal text (ordered for
    /// deterministic codegen).
    pub literals: BTreeMap<String, Self>,
    /// Intra-segment affix edges, keyed by `(prefix, suffix)` (ordered for
    /// deterministic codegen; matched most-specific-first — see
    /// [`affix_edges_in_match_order`]).
    pub affix: BTreeMap<(String, String), Self>,
    /// The single-segment wildcard edge (`*` / `{var}`), if any route uses one
    /// at this depth.
    pub single: Option<Box<Self>>,
    /// Routes that end exactly at this node (matched when the path has exactly
    /// this many segments).
    pub exact: Vec<Leaf>,
    /// Routes whose `**` catch-all begins at this node (matched by any, possibly
    /// empty, remainder).
    pub rest: Vec<Leaf>,
}

/// A built routing trie plus the metadata both backends need.
#[derive(Debug)]
pub struct Trie {
    /// The root node.
    pub root: Node,
    /// The largest number of path segments any route in the set has — the size
    /// of the segment-offset scratch buffers the interpreter (or generated
    /// `match`) needs.
    pub max_segments: usize,
    /// Whether any route declares a custom `:verb`; when `false` the path is not
    /// verb-split at all.
    pub any_verb: bool,
}

/// Builds the routing trie for a set of [`Route`]s.
#[must_use]
pub fn build_trie(routes: &[Route]) -> Trie {
    let templates: Vec<PathTemplate<'_>> = routes.iter().map(Route::template).collect();
    build_trie_with_templates(routes, &templates)
}

/// The most path segments one route template may declare.
///
/// Every code-generation walk over the trie — `emit_node`, `is_literal_only`,
/// `fused_literal_chain`, `collect_route_groups` — and `Node`'s derived drop
/// glue recurse once per segment, so an unbounded template overflows the
/// compiler's stack instead of producing a diagnostic. The bound is well above
/// any real URL template (they have far fewer than a hundred segments) and far
/// below the depth at which those recursions become a risk.
pub const MAX_TEMPLATE_SEGMENTS: usize = 256;

/// The number of trie levels a template occupies: its segments with each
/// variable expanded into its constituent atoms.
#[must_use]
pub fn template_depth(segments: &[Segment]) -> usize {
    flatten(segments).0.len()
}

/// Reports a template that declares more segments than [`MAX_TEMPLATE_SEGMENTS`].
///
/// Returns the diagnostic to attribute to the offending template, or `None` when
/// the template is within the bound.
#[must_use]
pub fn depth_limit_error(segments: &[Segment]) -> Option<String> {
    let depth = template_depth(segments);
    (depth > MAX_TEMPLATE_SEGMENTS).then(|| {
        format!("path template declares {depth} segments, but a route may declare at most {MAX_TEMPLATE_SEGMENTS}; shorten the template")
    })
}

pub(crate) fn build_trie_with_templates(routes: &[Route], templates: &[PathTemplate<'_>]) -> Trie {
    assert_eq!(routes.len(), templates.len(), "each route must have one parsed template");
    let mut root = Node::default();
    let mut max_segments = 0_usize;
    let mut any_verb = false;
    for (route_index, (route, template)) in routes.iter().zip(templates).enumerate() {
        any_verb |= template.verb().is_some();
        max_segments = max_segments.max(insert_route(&mut root, route, template, route_index));
    }
    Trie {
        root,
        max_segments,
        any_verb,
    }
}

/// Reports route sets whose routes are not all reachable and unambiguous.
///
/// Two kinds of problem are reported: several routes terminating at the same
/// trie node with the same HTTP method and custom verb, which makes all but the
/// first unreachable; and intra-segment (affix) patterns that are equally
/// specific yet can both match the same request, which makes the winner
/// arbitrary.
#[must_use]
pub fn conflicts(root: &Node) -> Vec<String> {
    let mut conflicts = Vec::new();
    let mut prefix = String::new();
    let mut pending = vec![ConflictAction::Visit(root, String::new())];
    while let Some(action) = pending.pop() {
        match action {
            ConflictAction::Visit(node, component) => {
                let parent_len = prefix.len();
                prefix.push_str(&component);
                check_bucket(&node.exact, &prefix, false, &mut conflicts);
                check_bucket(&node.rest, &prefix, true, &mut conflicts);
                check_affix_siblings(node, &prefix, &mut conflicts);

                pending.push(ConflictAction::Truncate(parent_len));
                if let Some(single) = &node.single {
                    pending.push(ConflictAction::Visit(single, "/*".to_owned()));
                }
                for ((affix_prefix, affix_suffix), child) in node.affix.iter().rev() {
                    pending.push(ConflictAction::Visit(child, format!("/{affix_prefix}{{}}{affix_suffix}")));
                }
                for (literal, child) in node.literals.iter().rev() {
                    pending.push(ConflictAction::Visit(child, format!("/{literal}")));
                }
            }
            ConflictAction::Truncate(len) => prefix.truncate(len),
        }
    }
    conflicts
}

enum ConflictAction<'a> {
    Visit(&'a Node, String),
    Truncate(usize),
}

fn check_bucket(leaves: &[Leaf], prefix: &str, is_rest: bool, out: &mut Vec<String>) {
    let mut groups: BTreeMap<(&str, Option<&str>), Vec<&str>> = BTreeMap::new();
    for leaf in leaves {
        groups
            .entry((leaf.method.as_str(), leaf.verb.as_deref()))
            .or_default()
            .push(leaf.name.as_str());
    }
    for ((method, verb), names) in groups {
        if names.len() > 1 {
            let path = if is_rest { format!("{prefix}/**") } else { prefix.to_owned() };
            let path = if path.is_empty() { "/".to_owned() } else { path };
            let verb = verb.map(|verb| format!(":{verb}")).unwrap_or_default();
            out.push(format!(
                "conflicting routes: `{method} {path}{verb}` maps to multiple names ({}); each HTTP method and path may resolve to only one route",
                names.join(", ")
            ));
        }
    }
}

/// Reports affix siblings of `node` that are equally specific yet can both match
/// the same request, so the match-order tie-break between them is arbitrary.
///
/// Affix edges of *different* specificity are deliberately ordered longest
/// literal prefix+suffix first (see [`affix_edges_in_match_order`]), so those
/// overlaps have a documented winner and are not reported.
fn check_affix_siblings(node: &Node, prefix: &str, out: &mut Vec<String>) {
    let edges: Vec<(&(String, String), &Node)> = node.affix.iter().collect();
    for (index, (left_key, left_child)) in edges.iter().enumerate() {
        for (right_key, right_child) in edges.iter().skip(index + 1) {
            let (left_prefix, left_suffix) = (left_key.0.as_str(), left_key.1.as_str());
            let (right_prefix, right_suffix) = (right_key.0.as_str(), right_key.1.as_str());
            if affix_specificity(left_prefix, left_suffix) != affix_specificity(right_prefix, right_suffix)
                || !affix_edges_overlap((left_prefix, left_suffix), (right_prefix, right_suffix))
            {
                continue;
            }

            let left_shapes = leaf_shapes(left_child);
            let right_shapes = leaf_shapes(right_child);
            let ambiguous = left_shapes.iter().find_map(|left| {
                right_shapes
                    .iter()
                    .find(|right| shapes_can_match_same_request(left, right))
                    .map(|right| (left, right))
            });
            let Some((left, right)) = ambiguous else {
                continue;
            };

            let left_edge = EdgeShape::Affix {
                prefix: left_prefix,
                suffix: left_suffix,
            };
            let right_edge = EdgeShape::Affix {
                prefix: right_prefix,
                suffix: right_suffix,
            };
            let method = &left.leaf.method;
            let verb = left.leaf.verb.as_ref().map(|verb| format!(":{verb}")).unwrap_or_default();
            out.push(format!(
                "conflicting routes: `{method} {left_path}{verb}` ({left_name}) and `{method} {right_path}{verb}` ({right_name}) are equally specific and can both match the same request, so which one wins is arbitrary; give one of the two intra-segment patterns a longer literal prefix or suffix",
                left_path = render_shape(prefix, left_edge, left),
                left_name = left.leaf.name,
                right_path = render_shape(prefix, right_edge, right),
                right_name = right.leaf.name,
            ));
        }
    }
}

/// Whether two affix edges can both match the same path segment.
///
/// A segment `t` matches an affix edge `(p, s)` when it starts with `p`, ends
/// with `s`, and is strictly longer than `p` and `s` together — the capture is
/// never empty and the prefix and suffix never overlap each other. Both edges
/// therefore match `t` exactly when `t` starts with both prefixes and ends with
/// both suffixes, which requires one prefix to be a prefix of the other and one
/// suffix to be a suffix of the other. When that holds, the segment built from
/// the longer prefix, one filler byte, and the longer suffix matches both, so
/// the condition is sufficient as well as necessary: no length ever makes such a
/// segment impossible, because the capture may be padded to any length.
///
/// Both comparisons are byte-wise, matching the runtime matcher, so affixes that
/// differ only in case are disjoint.
fn affix_edges_overlap((left_prefix, left_suffix): (&str, &str), (right_prefix, right_suffix): (&str, &str)) -> bool {
    let prefixes_agree = left_prefix.starts_with(right_prefix) || right_prefix.starts_with(left_prefix);
    let suffixes_agree = left_suffix.ends_with(right_suffix) || right_suffix.ends_with(left_suffix);
    prefixes_agree && suffixes_agree
}

/// A trie edge, as the constraint it places on one path segment.
#[derive(Clone, Copy)]
enum EdgeShape<'a> {
    Literal(&'a str),
    Affix { prefix: &'a str, suffix: &'a str },
    Single,
}

/// One route of a subtree: the edges walked from the root of that subtree to
/// reach it, the leaf itself, and whether it terminates in a `**` catch-all.
struct LeafShape<'a> {
    edges: Vec<EdgeShape<'a>>,
    leaf: &'a Leaf,
    is_rest: bool,
}

/// Every route in `node`'s subtree, each with the edges leading to it.
fn leaf_shapes(node: &Node) -> Vec<LeafShape<'_>> {
    let mut shapes = Vec::new();
    let mut pending = vec![(node, Vec::new())];
    while let Some((node, edges)) = pending.pop() {
        for (leaves, is_rest) in [(&node.exact, false), (&node.rest, true)] {
            shapes.extend(leaves.iter().map(|leaf| LeafShape {
                edges: edges.clone(),
                leaf,
                is_rest,
            }));
        }

        let mut descend = |edge, child| {
            let mut edges = edges.clone();
            edges.push(edge);
            pending.push((child, edges));
        };
        if let Some(single) = &node.single {
            descend(EdgeShape::Single, single);
        }
        for ((prefix, suffix), child) in &node.affix {
            descend(EdgeShape::Affix { prefix, suffix }, child);
        }
        for (literal, child) in &node.literals {
            descend(EdgeShape::Literal(literal), child);
        }
    }
    shapes
}

/// Whether some request matches both routes: same HTTP method and custom verb,
/// and a path every edge pair can agree on.
fn shapes_can_match_same_request(left: &LeafShape<'_>, right: &LeafShape<'_>) -> bool {
    if left.leaf.method != right.leaf.method || left.leaf.verb != right.leaf.verb {
        return false;
    }
    if !left.edges.iter().zip(&right.edges).all(|(l, r)| segments_can_agree(*l, *r)) {
        return false;
    }
    // A `**` leaf matches any remainder, so it only needs the other route to be
    // at least as deep; two exact leaves must be exactly as deep as each other.
    match (left.is_rest, right.is_rest) {
        (true, true) => true,
        (true, false) => right.edges.len() >= left.edges.len(),
        (false, true) => left.edges.len() >= right.edges.len(),
        (false, false) => left.edges.len() == right.edges.len(),
    }
}

/// Whether some path segment satisfies both edges' constraints.
fn segments_can_agree(left: EdgeShape<'_>, right: EdgeShape<'_>) -> bool {
    match (left, right) {
        (EdgeShape::Literal(left), EdgeShape::Literal(right)) => left == right,
        (EdgeShape::Literal(literal), EdgeShape::Affix { prefix, suffix })
        | (EdgeShape::Affix { prefix, suffix }, EdgeShape::Literal(literal)) => {
            literal.len() > prefix.len() + suffix.len() && literal.starts_with(prefix) && literal.ends_with(suffix)
        }
        (
            EdgeShape::Affix {
                prefix: left_prefix,
                suffix: left_suffix,
            },
            EdgeShape::Affix {
                prefix: right_prefix,
                suffix: right_suffix,
            },
        ) => affix_edges_overlap((left_prefix, left_suffix), (right_prefix, right_suffix)),
        // A single-segment wildcard matches any non-empty segment, and an affix
        // segment is always non-empty.
        (EdgeShape::Literal(literal), EdgeShape::Single) | (EdgeShape::Single, EdgeShape::Literal(literal)) => !literal.is_empty(),
        (EdgeShape::Affix { .. } | EdgeShape::Single, EdgeShape::Single) | (EdgeShape::Single, EdgeShape::Affix { .. }) => true,
    }
}

/// Renders a route's path: the node's own path, the affix edge reaching the
/// subtree, and the edges leading to the leaf within it.
fn render_shape(prefix: &str, edge: EdgeShape<'_>, shape: &LeafShape<'_>) -> String {
    let mut path = String::from(prefix);
    for edge in core::iter::once(edge).chain(shape.edges.iter().copied()) {
        match edge {
            EdgeShape::Literal(literal) => {
                path.push('/');
                path.push_str(literal);
            }
            EdgeShape::Affix {
                prefix: affix_prefix,
                suffix: affix_suffix,
            } => {
                let _ = write!(path, "/{affix_prefix}{{}}{affix_suffix}");
            }
            EdgeShape::Single => path.push_str("/*"),
        }
    }
    if shape.is_rest {
        path.push_str("/**");
    }
    path
}

/// Inserts one route into the trie, returning its path-segment (atom) count.
/// `route_index` is the route's position in the input set, recorded on the leaf.
fn insert_route(root: &mut Node, route: &Route, template: &PathTemplate<'_>, route_index: usize) -> usize {
    let (atoms, vars) = flatten(template.segments());
    let has_rest = matches!(atoms.last(), Some(Atom::Rest));
    let atom_count = atoms.len();

    // A trailing `**` is stored as a leaf on the node it starts from, never as an
    // edge, so descending it is a no-op: walking every atom lets the `Atom::Rest`
    // arm fall through without a separate edge count.
    let mut node = root;
    for atom in atoms {
        node = match atom {
            Atom::Literal(lit) => node.literals.entry(lit).or_default(),
            Atom::Single => {
                if node.single.is_none() {
                    node.single = Some(Box::new(Node::default()));
                }
                node.single.as_deref_mut().expect("single edge was just inserted")
            }
            Atom::Affix { prefix, suffix } => node.affix.entry((prefix, suffix)).or_default(),
            // `**` is always the last atom and is handled as a leaf below.
            Atom::Rest => node,
        };
    }

    let plans = vars
        .into_iter()
        .map(|v| {
            let key = v.name.join(".");
            let field = route_field_name(&key);
            if let Some((prefix_len, suffix_len)) = v.affix {
                VarPlan::Affix {
                    field,
                    key,
                    a: v.first,
                    prefix_len,
                    suffix_len,
                }
            } else if has_rest && v.last == atom_count - 1 {
                VarPlan::Rest { field, key, a: v.first }
            } else {
                VarPlan::Span {
                    field,
                    key,
                    a: v.first,
                    b: v.last,
                }
            }
        })
        .collect();

    let leaf = Leaf {
        method: route.method().to_owned(),
        verb: template.verb().map(str::to_owned),
        name: route.name().to_owned(),
        vars: plans,
        route_index,
    };

    if has_rest {
        node.rest.push(leaf);
    } else {
        node.exact.push(leaf);
    }

    atom_count
}

/// The affix edges of `node` in the order both backends must try them: longer
/// literal prefix+suffix first, so the more specific edge wins.
///
/// Equally specific edges are ordered by key, which is only a determinism
/// tie-break: [`conflicts`] rejects equally specific edges that can both match a
/// segment and reach the same route, so their relative order never decides a
/// match.
#[must_use]
pub fn affix_edges_in_match_order(node: &Node) -> Vec<(&(String, String), &Node)> {
    let mut affixes: Vec<_> = node.affix.iter().collect();
    affixes.sort_by(|((p1, s1), _), ((p2, s2), _)| {
        affix_specificity(p2, s2)
            .cmp(&affix_specificity(p1, s1))
            .then_with(|| (p1, s1).cmp(&(p2, s2)))
    });
    affixes
}

/// The specificity of an affix edge: longer literal prefix+suffix wins.
fn affix_specificity(prefix: &str, suffix: &str) -> usize {
    prefix.len() + suffix.len()
}

/// The ordered field-name segment lists of the variables a template captures.
///
/// Each is the variable's dotted path, e.g. `["shelf"]` or `["shelf", "id"]`.
/// Shared with `rest_over_grpc::build`-style callers that group routes by capture
/// signature.
///
/// # Ordering contract
///
/// The variables are returned in **template declaration order** — the
/// left-to-right order the `{variables}` appear in the path template. This is the
/// same order the runtime backend materializes captured *values* in, so it aligns
/// element-for-element with the pair order of `RawMatch::captures`.
#[must_use]
pub fn capture_field_names(segments: &[Segment]) -> Vec<Vec<String>> {
    flatten(segments).1.into_iter().map(|var| var.name).collect()
}

/// Maps a route template variable's name to the sanitized Rust *field*
/// identifier the generated `Route` enum uses for it.
///
/// This is only the codegen field identifier; it is **not** the runtime
/// `capture` key. Both backends key `capture` on the variable's original
/// (unmangled) name (see [`VarPlan::key`]).
///
/// The name is the variable's (possibly dotted) source name, e.g. `"shelf"` or
/// `"shelf.id"`; dotted path separators become `_` (so `"shelf.id"` yields
/// `"shelf_id"`), and a name that is not a valid Rust identifier — most commonly
/// one that collides with a keyword, e.g. `"type"` — is given a deterministic
/// `_f_` prefix so the output still tokenizes.
///
/// # Examples
///
/// ```
/// use routerama_build::route_field_name;
///
/// assert_eq!(route_field_name("shelf"), "shelf");
/// assert_eq!(route_field_name("shelf.id"), "shelf_id");
/// ```
#[must_use]
pub fn route_field_name(name: impl AsRef<str>) -> String {
    field_name(&name.as_ref().replace('.', "_"))
}

/// Turns a `_`-joined candidate field name into a valid identifier: used as-is
/// when it already is one, otherwise sanitized (non-identifier characters
/// replaced with `_`) and given an `_f_` prefix so it still tokenizes.
pub(crate) fn field_name(joined: &str) -> String {
    if is_valid_variant(joined) {
        joined.to_owned()
    } else {
        let sanitized: String = joined
            .chars()
            .map(|c| if c == '_' || c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        format!("_f_{sanitized}")
    }
}

/// Whether `name` can be used verbatim as a route enum variant: a non-empty,
/// non-keyword Rust identifier.
#[must_use]
pub fn is_valid_variant(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || unicode_ident::is_xid_start(first)) {
        return false;
    }
    if !chars.all(unicode_ident::is_xid_continue) {
        return false;
    }
    // Reserved words (and the bare underscore) cannot name an enum variant.
    !matches!(
        name,
        "_" | "as"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
            | "try"
            | "gen"
    )
}

/// Flattens a template's top-level segments into a linear atom sequence,
/// recording the capture span of each variable.
// Coverage off: the defensive wildcard arms exist only because `Segment` is
// non-exhaustive; every variant that exists today is covered by unit tests.
#[cfg_attr(coverage_nightly, coverage(off))]
fn flatten(segments: &[Segment]) -> (Vec<Atom>, Vec<VarSpan>) {
    let mut atoms = Vec::new();
    let mut vars = Vec::new();

    for seg in segments {
        match seg {
            Segment::Literal(lit) => atoms.push(Atom::Literal((*lit).to_owned())),
            Segment::Single => atoms.push(Atom::Single),
            Segment::Rest => atoms.push(Atom::Rest),
            Segment::Affix { prefix, name, suffix } => {
                let index = atoms.len();
                atoms.push(Atom::Affix {
                    prefix: (*prefix).to_owned(),
                    suffix: (*suffix).to_owned(),
                });
                vars.push(VarSpan {
                    name: name.split('.').map(str::to_owned).collect(),
                    first: index,
                    last: index,
                    affix: Some((prefix.len(), suffix.len())),
                });
            }
            Segment::Variable(var) => {
                let first = atoms.len();
                for sub in var.segments() {
                    match sub {
                        Segment::Literal(lit) => atoms.push(Atom::Literal((*lit).to_owned())),
                        Segment::Single => atoms.push(Atom::Single),
                        Segment::Rest => atoms.push(Atom::Rest),
                        // Nested variables are rejected by the parser; any
                        // future segment kind is conservatively ignored.
                        _ => {}
                    }
                }
                let last = atoms.len() - 1;
                vars.push(VarSpan {
                    name: var.field_path().split('.').map(str::to_owned).collect(),
                    first,
                    last,
                    affix: None,
                });
            }
            // `Segment` is non-exhaustive; ignore any future variant.
            _ => {}
        }
    }

    (atoms, vars)
}

#[cfg(test)]
mod tests {
    use http_path_template::Grammar;

    use super::*;

    fn rule(name: &str, method: &str, template: &str) -> Route {
        Route::new(
            name,
            method,
            PathTemplate::parse(template, Grammar::default()).expect("valid template"),
        )
    }

    fn ext_rule(name: &str, method: &str, template: &str) -> Route {
        Route::new(
            name,
            method,
            PathTemplate::parse(template, Grammar::default().with_segment_affixes()).expect("valid extended template"),
        )
    }

    #[test]
    fn route_field_name_sanitizes_keywords_and_dots() {
        assert_eq!(route_field_name("shelf"), "shelf");
        assert_eq!(route_field_name("shelf.id"), "shelf_id");
        assert_eq!(route_field_name("type"), "_f_type");
        assert_eq!(route_field_name("a-b"), "_f_a_b");
    }

    #[test]
    fn is_valid_variant_rejects_keywords_and_non_idents() {
        assert!(is_valid_variant("GetShelf"));
        assert!(is_valid_variant("Διαδρομή"));
        assert!(is_valid_variant("路由"));
        assert!(!is_valid_variant("type"));
        assert!(!is_valid_variant("2go"));
        assert!(!is_valid_variant(""));
    }

    #[test]
    fn the_segment_limit_reports_only_over_deep_templates() {
        let shallow = PathTemplate::parse("/books/{book}/reviews", Grammar::default()).expect("valid template");
        assert_eq!(template_depth(shallow.segments()), 3);
        assert_eq!(depth_limit_error(shallow.segments()), None);

        let at_limit = "/a".repeat(MAX_TEMPLATE_SEGMENTS);
        let at_limit = PathTemplate::parse(&at_limit, Grammar::default()).expect("valid template");
        assert_eq!(template_depth(at_limit.segments()), MAX_TEMPLATE_SEGMENTS);
        assert_eq!(depth_limit_error(at_limit.segments()), None);

        let too_deep = "/a".repeat(MAX_TEMPLATE_SEGMENTS + 1);
        let too_deep = PathTemplate::parse(&too_deep, Grammar::default()).expect("valid template");
        let message = depth_limit_error(too_deep.segments()).expect("a template past the limit is reported");
        assert!(message.contains("declares 257 segments"), "{message}");
        assert!(message.contains("at most 256"), "{message}");
    }

    #[test]
    fn build_trie_reports_max_segments_and_verb_usage() {
        let trie = build_trie(&[rule("A", "GET", "/books"), rule("B", "GET", "/books/{book}/reviews/{review}")]);
        assert_eq!(trie.max_segments, 4);
        assert!(!trie.any_verb);

        let verbed = build_trie(&[rule("Arch", "POST", "/books/{book}:archive")]);
        assert!(verbed.any_verb);
    }

    #[test]
    fn conflicts_reports_identical_route_shapes() {
        let trie = build_trie(&[rule("First", "GET", "/books/{book}"), rule("Second", "GET", "/books/{other}")]);
        let conflicts = conflicts(&trie.root);
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].contains("GET /books/*"), "{}", conflicts[0]);
        assert!(conflicts[0].contains("First, Second"), "{}", conflicts[0]);
    }

    #[test]
    fn affix_edges_are_ordered_most_specific_first() {
        let trie = build_trie(&[ext_rule("Short", "GET", "/v{v}"), ext_rule("Long", "GET", "/img-{id}.png")]);
        let order: Vec<_> = affix_edges_in_match_order(&trie.root)
            .into_iter()
            .map(|((p, s), _)| (p.clone(), s.clone()))
            .collect();
        // The longer prefix+suffix ("img-" + ".png") sorts before ("v", "").
        assert_eq!(order[0], ("img-".to_owned(), ".png".to_owned()));
    }

    #[test]
    fn affix_order_is_by_summed_literal_length_not_key_or_product() {
        // `A = ("z", "bbbb")` sums to 5; `B = ("aa", "dd")` sums to 4, so the
        // summed literal length orders A before B even though A's key sorts after
        // B's — confirming the sort uses prefix+suffix length, not key order.
        let trie = build_trie(&[ext_rule("A", "GET", "/z{a}bbbb"), ext_rule("B", "GET", "/aa{b}dd")]);
        let order: Vec<_> = affix_edges_in_match_order(&trie.root)
            .into_iter()
            .map(|((p, s), _)| (p.clone(), s.clone()))
            .collect();
        assert_eq!(order[0], ("z".to_owned(), "bbbb".to_owned()));
        assert_eq!(order[1], ("aa".to_owned(), "dd".to_owned()));
    }

    #[test]
    fn two_affix_edges_that_can_match_the_same_segment_are_reported_as_a_conflict() {
        let trie = build_trie(&[ext_rule("First", "GET", "/a{x}"), ext_rule("Second", "GET", "/{y}b")]);
        let conflicts = conflicts(&trie.root);
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert!(conflicts[0].contains("GET /{}b"), "{}", conflicts[0]);
        assert!(conflicts[0].contains("GET /a{}"), "{}", conflicts[0]);
        assert!(conflicts[0].contains("First"), "{}", conflicts[0]);
        assert!(conflicts[0].contains("Second"), "{}", conflicts[0]);
        assert!(conflicts[0].contains("equally specific"), "{}", conflicts[0]);
    }

    #[test]
    fn equally_specific_affix_edges_with_disjoint_literals_are_not_a_conflict() {
        // Same specificity, but no segment can start with both `a` and `b`, end
        // with both `a` and `b`, or start with `a` while ending with `b` after a
        // prefix of `c`.
        let disjoint = [["/a{x}", "/b{y}"], ["/{x}a", "/{y}b"], ["/ab{x}", "/ac{y}"], ["/{x}ba", "/{y}ca"]];
        for [left, right] in disjoint {
            let trie = build_trie(&[ext_rule("Left", "GET", left), ext_rule("Right", "GET", right)]);
            assert!(
                conflicts(&trie.root).is_empty(),
                "`{left}` and `{right}` cannot both match a segment"
            );
        }
    }

    #[test]
    fn equally_specific_affix_edges_overlap_when_one_literal_extends_the_other() {
        // `/ab{x}` and `/a{y}b` both match `abZb`, and `/a{x}` and `/{y}a` both
        // match `aZa`, so both pairs are ambiguous.
        let overlapping = [["/ab{x}", "/a{y}b"], ["/a{x}", "/{y}a"], ["/ab{x}", "/{y}cb"], ["/ab{x}", "/a{y}c"]];
        for [left, right] in overlapping {
            let trie = build_trie(&[ext_rule("Left", "GET", left), ext_rule("Right", "GET", right)]);
            assert_eq!(conflicts(&trie.root).len(), 1, "`{left}` and `{right}` can both match a segment");
        }
    }

    #[test]
    fn affix_edges_that_can_match_the_same_segment_but_not_the_same_request_are_not_a_conflict() {
        // A segment such as `a-b-c` matches both edges, but the tails differ, so
        // the descent backtracks to exactly one route and nothing is arbitrary.
        let trie = build_trie(&[
            ext_rule("First", "GET", "/n/a-{id}/first"),
            ext_rule("Second", "GET", "/n/{id}-c/second"),
        ]);
        assert!(conflicts(&trie.root).is_empty(), "{:?}", conflicts(&trie.root));

        // Differing HTTP methods likewise keep the choice unambiguous.
        let by_method = build_trie(&[ext_rule("Get", "GET", "/a{x}"), ext_rule("Put", "PUT", "/{y}b")]);
        assert!(conflicts(&by_method.root).is_empty(), "{:?}", conflicts(&by_method.root));
    }

    #[test]
    fn overlapping_affix_edges_conflict_through_deeper_matching_tails() {
        // The tails agree only because `*` and `**` also match the literal, so
        // the ambiguity is only visible below the affix edges themselves.
        let single = build_trie(&[ext_rule("Left", "GET", "/a{x}/lit"), ext_rule("Right", "GET", "/{y}b/{z}")]);
        assert_eq!(conflicts(&single.root).len(), 1, "{:?}", conflicts(&single.root));

        let rest = build_trie(&[ext_rule("Left", "GET", "/a{x}/one/two"), ext_rule("Right", "GET", "/{y}b/**")]);
        assert_eq!(conflicts(&rest.root).len(), 1, "{:?}", conflicts(&rest.root));

        // A `**` matches a possibly empty remainder, so `/{y}b/**` is ambiguous
        // with the single-segment `/a{x}`, but one that needs a further literal
        // segment is deeper than `/a{x}` can ever be.
        let too_shallow = build_trie(&[ext_rule("Left", "GET", "/a{x}"), ext_rule("Right", "GET", "/{y}b/lit/**")]);
        assert!(conflicts(&too_shallow.root).is_empty(), "{:?}", conflicts(&too_shallow.root));
    }

    #[test]
    fn affix_edges_of_different_specificity_are_left_to_the_precedence_order() {
        // `/a{x}c` and `/a{y}` both match `abc`, but the longer literal wins by
        // the documented specificity order, so this is not reported.
        let trie = build_trie(&[ext_rule("Long", "GET", "/a{x}c"), ext_rule("Short", "GET", "/a{y}")]);
        assert!(conflicts(&trie.root).is_empty(), "{:?}", conflicts(&trie.root));
    }

    #[test]
    fn affix_literals_are_compared_case_sensitively_like_the_runtime_matcher() {
        // The runtime matcher compares affix literals byte-wise, so no segment
        // starts with both `a` and `A` and the two edges never both match.
        let trie = build_trie(&[ext_rule("Lower", "GET", "/a{x}"), ext_rule("Upper", "GET", "/A{y}")]);
        assert!(conflicts(&trie.root).is_empty(), "{:?}", conflicts(&trie.root));
    }

    #[test]
    fn a_leaf_carries_precomputed_capture_field_names() {
        let trie = build_trie(&[rule("Get", "GET", "/books/{book.id}")]);
        let node = trie
            .root
            .literals
            .get("books")
            .expect("books edge")
            .single
            .as_ref()
            .expect("single edge");
        let leaf = node.exact.first().expect("exact leaf");
        assert_eq!(leaf.vars[0].field(), "book_id");
        // The runtime `capture` key is the original (unmangled) variable name,
        // distinct from the sanitized field identifier above.
        assert_eq!(leaf.vars[0].key(), "book.id");
    }
}
