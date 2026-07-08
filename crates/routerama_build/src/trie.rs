// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The framework-neutral routing trie — the shared matching IR.
//!
//! A route set is lowered into a [`Node`] trie whose edges encode the
//! `google.api.http` matching precedence (literal → intra-segment affix →
//! single-segment wildcard → `**` catch-all, deeper-before-shorter). The trie is
//! consumed by **two backends** that must resolve identically:
//!
//! - the **static** backend (`crate::codegen`, behind the `codegen` feature)
//!   lowers the trie to a compile-time `match`;
//! - the **dynamic** backend (`routerama`'s runtime router, behind that crate's
//!   `dynamic` feature) walks the same trie at runtime.
//!
//! This module carries no `proc-macro2` / `quote` dependency, so the runtime
//! interpreter can build the trie without pulling code-generation crates into a
//! server binary. The precedence-sensitive edge ordering lives here
//! ([`affix_edges_in_match_order`]) so both backends share one source of truth.

use std::collections::BTreeMap;

use http_path_template::Segment;

use crate::route_rule::RouteRule;

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
/// Each variant carries the pre-computed field name (see [`route_field_name`])
/// so neither backend re-derives it per match: the static codegen turns it into
/// a struct-variant field identifier, and the dynamic interpreter uses it as the
/// capture key.
#[derive(Debug)]
pub enum VarPlan {
    /// A value spanning atoms `a..=b`, sliced from the path as
    /// `&body[starts[a]..ends[b]]`.
    Span { field: String, a: usize, b: usize },
    /// A `**` capture from atom `a` to the end of the path.
    Rest { field: String, a: usize },
    /// An intra-segment capture: the middle of segment `a`, sliced as
    /// `&body[starts[a] + prefix_len .. ends[a] - suffix_len]`.
    Affix {
        field: String,
        a: usize,
        prefix_len: usize,
        suffix_len: usize,
    },
}

impl VarPlan {
    /// The generated field name / dynamic capture key for this variable.
    #[must_use]
    pub fn field(&self) -> &str {
        match self {
            Self::Span { field, .. } | Self::Rest { field, .. } | Self::Affix { field, .. } => field,
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

/// Builds the routing trie for a set of [`RouteRule`]s.
#[must_use]
pub fn build_trie(routes: &[RouteRule]) -> Trie {
    let mut root = Node::default();
    let mut max_segments = 0_usize;
    let mut any_verb = false;
    for route in routes {
        any_verb |= route.template().verb().is_some();
        max_segments = max_segments.max(insert_route(&mut root, route));
    }
    Trie {
        root,
        max_segments,
        any_verb,
    }
}

/// Inserts one route into the trie, returning its path-segment (atom) count.
fn insert_route(root: &mut Node, route: &RouteRule) -> usize {
    let (atoms, vars) = flatten(route.template().segments());
    let has_rest = matches!(atoms.last(), Some(Atom::Rest));

    // A trailing `**` is stored as a leaf on the node it starts from, never as an
    // edge, so descending it is a no-op: walking every atom lets the `Atom::Rest`
    // arm fall through without a separate edge count.
    let mut node = root;
    for atom in &atoms {
        node = match atom {
            Atom::Literal(lit) => node.literals.entry(lit.clone()).or_default(),
            Atom::Single => {
                if node.single.is_none() {
                    node.single = Some(Box::new(Node::default()));
                }
                node.single.as_deref_mut().expect("single edge was just inserted")
            }
            Atom::Affix { prefix, suffix } => node.affix.entry((prefix.clone(), suffix.clone())).or_default(),
            // `**` is always the last atom and is handled as a leaf below.
            Atom::Rest => node,
        };
    }

    let plans = vars
        .into_iter()
        .map(|v| {
            let field = route_field_name(v.name.join("."));
            if let Some((prefix_len, suffix_len)) = v.affix {
                VarPlan::Affix {
                    field,
                    a: v.first,
                    prefix_len,
                    suffix_len,
                }
            } else if has_rest && v.last == atoms.len() - 1 {
                VarPlan::Rest { field, a: v.first }
            } else {
                VarPlan::Span {
                    field,
                    a: v.first,
                    b: v.last,
                }
            }
        })
        .collect();

    let leaf = Leaf {
        method: route.method().as_str().to_owned(),
        verb: route.template().verb().map(str::to_owned),
        name: route.name().to_owned(),
        vars: plans,
    };

    if has_rest {
        node.rest.push(leaf);
    } else {
        node.exact.push(leaf);
    }

    atoms.len()
}

/// The affix edges of `node` in the order both backends must try them: longer
/// literal prefix+suffix first (more specific wins), ties broken by key so the
/// ordering is deterministic.
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
#[must_use]
pub fn capture_field_names(segments: &[Segment]) -> Vec<Vec<String>> {
    flatten(segments).1.into_iter().map(|var| var.name).collect()
}

/// Maps a route template variable's name to the field name the generated
/// `Route` enum uses for it (and the dynamic router's capture key).
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
/// non-keyword ASCII identifier.
#[must_use]
pub fn is_valid_variant(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    if !chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
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
            Segment::Literal(lit) => atoms.push(Atom::Literal(lit.clone())),
            Segment::Single => atoms.push(Atom::Single),
            Segment::Rest => atoms.push(Atom::Rest),
            Segment::Affix { prefix, name, suffix } => {
                let index = atoms.len();
                atoms.push(Atom::Affix {
                    prefix: prefix.clone(),
                    suffix: suffix.clone(),
                });
                vars.push(VarSpan {
                    name: name.clone(),
                    first: index,
                    last: index,
                    affix: Some((prefix.len(), suffix.len())),
                });
            }
            Segment::Variable(var) => {
                let first = atoms.len();
                for sub in var.segments() {
                    match sub {
                        Segment::Literal(lit) => atoms.push(Atom::Literal(lit.clone())),
                        Segment::Single => atoms.push(Atom::Single),
                        Segment::Rest => atoms.push(Atom::Rest),
                        // Nested variables are rejected by the parser; any
                        // future segment kind is conservatively ignored.
                        _ => {}
                    }
                }
                let last = atoms.len() - 1;
                vars.push(VarSpan {
                    name: var.field_path().to_vec(),
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
    use http_path_template::{Grammar, PathTemplate};

    use super::*;
    use crate::http_method::HttpMethod;

    fn rule(name: &str, method: HttpMethod, template: &str) -> RouteRule {
        RouteRule::new(name, method, template.parse::<PathTemplate>().expect("valid template"))
    }

    fn ext_rule(name: &str, method: HttpMethod, template: &str) -> RouteRule {
        RouteRule::new(
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
        assert!(!is_valid_variant("type"));
        assert!(!is_valid_variant("2go"));
        assert!(!is_valid_variant(""));
    }

    #[test]
    fn build_trie_reports_max_segments_and_verb_usage() {
        let trie = build_trie(&[
            rule("A", HttpMethod::Get, "/books"),
            rule("B", HttpMethod::Get, "/books/{book}/reviews/{review}"),
        ]);
        assert_eq!(trie.max_segments, 4);
        assert!(!trie.any_verb);

        let verbed = build_trie(&[rule("Arch", HttpMethod::Post, "/books/{book}:archive")]);
        assert!(verbed.any_verb);
    }

    #[test]
    fn affix_edges_are_ordered_most_specific_first() {
        let trie = build_trie(&[
            ext_rule("Short", HttpMethod::Get, "/v{v}"),
            ext_rule("Long", HttpMethod::Get, "/img-{id}.png"),
        ]);
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
        // summed length puts A first — even though A's key sorts *after* B's.
        // A constant specificity (mutated to 0 or 1) would tie every edge and
        // fall back to key order (B first); a *product* (1*4 vs 2*2, both 4) would
        // likewise tie and pick B. Only the true sum keeps A ahead of B.
        let trie = build_trie(&[
            ext_rule("A", HttpMethod::Get, "/z{a}bbbb"),
            ext_rule("B", HttpMethod::Get, "/aa{b}dd"),
        ]);
        let order: Vec<_> = affix_edges_in_match_order(&trie.root)
            .into_iter()
            .map(|((p, s), _)| (p.clone(), s.clone()))
            .collect();
        assert_eq!(order[0], ("z".to_owned(), "bbbb".to_owned()));
        assert_eq!(order[1], ("aa".to_owned(), "dd".to_owned()));
    }

    #[test]
    fn a_leaf_carries_precomputed_capture_field_names() {
        let trie = build_trie(&[rule("Get", HttpMethod::Get, "/books/{book.id}")]);
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
    }
}
