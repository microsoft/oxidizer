// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static REST router code generation.
//!
//! [`Router::generate`] lowers a set of [`Route`]s into a `resolve` function
//! that matches an HTTP method + path against the routes. Because the route set
//! is fully known at build time, the routes are lowered into a **compile-time
//! trie**: a nested `match` over the path's `/`-separated segments. Unlike a
//! general-purpose runtime trie, there are no node structures, no
//! bounds-checked node traversal, and no per-request allocation — the segment
//! offsets are scanned into stack buffers (`rest_over_grpc::scan_segments`) and the
//! captured path variables are returned inline via `rest_over_grpc::RouteMatch`.
//!
//! At each trie node the branches are tried most-specific-first — literal
//! segments before a single-segment wildcard before a `**` catch-all, and a
//! deeper (longer) match before a shorter one — so overlapping templates resolve
//! deterministically.

use std::collections::BTreeMap;

use http_path_template::Segment;
use proc_macro2::{Literal, TokenStream};
use quote::quote;

use crate::route::Route;

/// A flattened path-template element, with variables expanded into their
/// constituent atoms.
enum Atom {
    Literal(String),
    Single,
    Rest,
}

/// A variable's capture span over the flattened atom sequence.
struct VarSpan {
    field_path: Vec<String>,
    first: usize,
    last: usize,
}

/// A plan for materializing one captured path variable at a leaf.
enum VarPlan {
    /// A value spanning atoms `a..=b`, sliced from the path as
    /// `&body[starts[a]..ends[b]]`.
    Span { field_path: Vec<String>, a: usize, b: usize },
    /// A `**` capture from atom `a` to the end of the path.
    Rest { field_path: Vec<String>, a: usize },
}

/// A route that terminates at a trie node: how to dispatch it and what to bind.
struct Leaf {
    method: String,
    verb: Option<String>,
    rpc: String,
    vars: Vec<VarPlan>,
}

/// A node of the compile-time routing trie.
#[derive(Default)]
struct Node {
    /// Literal-segment edges, keyed by the literal text (ordered for
    /// deterministic codegen).
    literals: BTreeMap<String, Self>,
    /// The single-segment wildcard edge (`*` / `{var}`), if any route uses one
    /// at this depth.
    single: Option<Box<Self>>,
    /// Routes that end exactly at this node (matched when the path has exactly
    /// this many segments).
    exact: Vec<Leaf>,
    /// Routes whose `**` catch-all begins at this node (matched by any, possibly
    /// empty, remainder).
    rest: Vec<Leaf>,
}

/// Collects the routes of a single gRPC service and emits a static REST router.
///
/// # Examples
///
/// ```
/// use rest_over_grpc_build::{HttpMethod, HttpRule, Router};
///
/// let routes = HttpRule::new("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")
///     .lower()
///     .expect("valid path template");
/// let tokens = Router::new(routes).generate();
///
/// assert!(!tokens.to_string().is_empty());
/// assert!(tokens.to_string().contains("resolve"));
/// ```
#[derive(Debug)]
pub struct Router {
    routes: Vec<Route>,
}

impl Router {
    /// Creates a router from the lowered `routes` of a service.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc_build::{HttpMethod, HttpRule, Router};
    ///
    /// let routes = HttpRule::new("GetBook", HttpMethod::Get, "/v1/books/{book}")
    ///     .lower()
    ///     .expect("valid path template");
    /// let router = Router::new(routes);
    ///
    /// assert!(router.generate().to_string().contains("GetBook"));
    /// ```
    #[must_use]
    pub fn new(routes: Vec<Route>) -> Self {
        Self { routes }
    }

    /// Generates the `resolve` function: a static dispatcher mapping an HTTP
    /// method + path to the resolved RPC and its captured path variables.
    ///
    /// The returned [`TokenStream`] defines:
    ///
    /// ```ignore
    /// pub fn resolve<'p>(method: &str, path: &'p str)
    ///     -> Option<rest_over_grpc::RouteMatch<'p>>;
    /// ```
    ///
    /// The routes are lowered into a nested `match` over path segments (a
    /// compile-time trie); overlapping templates resolve most-specific-first.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc_build::{HttpMethod, HttpRule, Router};
    ///
    /// let routes = HttpRule::new("ListBooks", HttpMethod::Get, "/v1/shelves/{shelf}/books")
    ///     .lower()
    ///     .expect("valid path template");
    /// let tokens = Router::new(routes).generate();
    /// let code = tokens.to_string();
    ///
    /// assert!(!code.is_empty());
    /// assert!(code.contains("ListBooks"));
    /// ```
    #[must_use]
    pub fn generate(&self) -> TokenStream {
        let any_verb = self.routes.iter().any(|r| r.template().verb().is_some());

        let mut root = Node::default();
        let mut cap = 0_usize;
        for route in &self.routes {
            cap = cap.max(insert_route(&mut root, route));
        }

        let body_bind = if any_verb {
            quote! { let (__body, __verb) = ::rest_over_grpc::split_verb(path); }
        } else {
            quote! { let __body: &str = path; }
        };

        let cap_lit = Literal::usize_unsuffixed(cap);
        let scan = quote! {
            let mut __starts = [0usize; #cap_lit];
            let mut __ends = [0usize; #cap_lit];
            let __count = ::rest_over_grpc::scan_segments(__body, &mut __starts, &mut __ends);
        };

        let tree = emit_node(&root, 0, any_verb);

        quote! {
            /// Resolves an HTTP `method` + `path` to the gRPC RPC it transcodes
            /// to, capturing any path variables.
            ///
            /// Returns `None` if no route matches. Generated by `rest_over_grpc_build`.
            #[allow(
                clippy::all,
                clippy::pedantic,
                clippy::nursery,
                clippy::restriction,
                dead_code,
                unused,
                reason = "code generated by rest_over_grpc_build"
            )]
            #[must_use]
            pub fn resolve<'p>(
                method: &str,
                path: &'p str,
            ) -> ::core::option::Option<::rest_over_grpc::RouteMatch<'p>> {
                #body_bind
                #scan
                #tree
                ::core::option::Option::None
            }
        }
    }
}

/// Inserts `route` into the trie rooted at `root`, returning the number of atoms
/// (the deepest segment index + 1) the route reaches — used to size the segment
/// scan buffers.
// Coverage off: the only unhit line is the defensive `Atom::Rest` edge arm;
// `**` is only ever the final atom, stored as a leaf rather than an edge.
#[cfg_attr(coverage_nightly, coverage(off))]
fn insert_route(root: &mut Node, route: &Route) -> usize {
    let (atoms, vars) = flatten(route.template().segments());
    let has_rest = matches!(atoms.last(), Some(Atom::Rest));
    let edges = if has_rest { atoms.len() - 1 } else { atoms.len() };

    let mut node = root;
    for atom in atoms.iter().take(edges) {
        node = match atom {
            Atom::Literal(lit) => node.literals.entry(lit.clone()).or_default(),
            Atom::Single => {
                if node.single.is_none() {
                    node.single = Some(Box::new(Node::default()));
                }
                node.single.as_deref_mut().expect("single edge was just inserted")
            }
            // `**` is always the last atom and is handled as a leaf below.
            Atom::Rest => node,
        };
    }

    let plans = vars
        .into_iter()
        .map(|v| {
            if has_rest && v.last == atoms.len() - 1 {
                VarPlan::Rest {
                    field_path: v.field_path,
                    a: v.first,
                }
            } else {
                VarPlan::Span {
                    field_path: v.field_path,
                    a: v.first,
                    b: v.last,
                }
            }
        })
        .collect();

    let leaf = Leaf {
        method: route.method().as_str().to_owned(),
        verb: route.template().verb().map(str::to_owned),
        rpc: route.rpc().to_owned(),
        vars: plans,
    };

    if has_rest {
        node.rest.push(leaf);
    } else {
        node.exact.push(leaf);
    }

    atoms.len()
}

/// Emits the matching code for a trie `node` reached after consuming `depth`
/// segments. On a match the code `return`s; otherwise it falls through.
fn emit_node(node: &Node, depth: usize, any_verb: bool) -> TokenStream {
    let depth_lit = Literal::usize_unsuffixed(depth);

    // Branch taken when a segment exists at `depth`: try literal edges, then the
    // single-segment wildcard, then any `**` catch-all (most-specific-first).
    let has_literals = !node.literals.is_empty();
    let seg_bind = if has_literals {
        quote! { let __seg: &str = &__body[__starts[#depth_lit]..__ends[#depth_lit]]; }
    } else {
        quote! {}
    };
    let literal_arms = node.literals.iter().map(|(lit, child)| {
        let child = emit_node(child, depth + 1, any_verb);
        quote! { #lit => { #child } }
    });
    let literal_match = if has_literals {
        quote! {
            match __seg {
                #(#literal_arms)*
                _ => {}
            }
        }
    } else {
        quote! {}
    };
    // A single-segment wildcard (`*` / `{var}`) matches exactly one non-empty
    // segment (like `PathTemplate::match_path`), so its subtree is guarded on a
    // non-empty segment. Literal edges need no guard; `**` captures the rest.
    let single_code = node
        .single
        .as_ref()
        .map(|child| {
            let child = emit_node(child, depth + 1, any_verb);
            quote! {
                if __ends[#depth_lit] > __starts[#depth_lit] {
                    #child
                }
            }
        })
        .unwrap_or_default();
    let rest_dispatch = emit_leaves(&node.rest, any_verb);

    let has_segment_branch = has_literals || node.single.is_some() || !node.rest.is_empty();
    let segment_branch = if has_segment_branch {
        quote! {
            if __count > #depth_lit {
                #seg_bind
                #literal_match
                #single_code
                #rest_dispatch
            }
        }
    } else {
        quote! {}
    };

    // Branch taken when the path ends here: exact leaves, then any `**` catch-all
    // matching an empty remainder.
    let exact_dispatch = emit_leaves(&node.exact, any_verb);
    let rest_empty_dispatch = emit_leaves(&node.rest, any_verb);
    let has_end_branch = !node.exact.is_empty() || !node.rest.is_empty();
    let end_branch = if has_end_branch {
        quote! {
            if __count == #depth_lit {
                #exact_dispatch
                #rest_empty_dispatch
            }
        }
    } else {
        quote! {}
    };

    quote! {
        #segment_branch
        #end_branch
    }
}

/// Emits `method` (and, when any route uses one, `:verb`) dispatch for a set of
/// leaves terminating at the same trie node, returning a [`RouteMatch`] on a hit.
fn emit_leaves(leaves: &[Leaf], any_verb: bool) -> TokenStream {
    if leaves.is_empty() {
        return quote! {};
    }

    // Group by method (preserving first-seen order), deduplicating (method, verb)
    // pairs so we never emit two arms for the same case.
    let mut groups: Vec<(String, Vec<&Leaf>)> = Vec::new();
    for leaf in leaves {
        let group = if let Some((_, entries)) = groups.iter_mut().find(|(m, _)| *m == leaf.method) {
            entries
        } else {
            groups.push((leaf.method.clone(), Vec::new()));
            &mut groups.last_mut().expect("just pushed").1
        };
        if !group.iter().any(|existing| existing.verb == leaf.verb) {
            group.push(leaf);
        }
    }

    let arms = groups.iter().map(|(method, entries)| {
        let body = if any_verb {
            let verb_arms = entries.iter().map(|leaf| {
                let ret = emit_return(leaf);
                if let Some(verb) = &leaf.verb {
                    quote! { ::core::option::Option::Some(#verb) => { #ret } }
                } else {
                    quote! { ::core::option::Option::None => { #ret } }
                }
            });
            quote! {
                match __verb {
                    #(#verb_arms)*
                    _ => {}
                }
            }
        } else {
            emit_return(entries.first().expect("each group has at least one leaf"))
        };
        quote! { #method => { #body } }
    });

    quote! {
        match method {
            #(#arms)*
            _ => {}
        }
    }
}

/// Emits a `return Some(RouteMatch::with_bindings(...))` for a matched leaf.
fn emit_return(leaf: &Leaf) -> TokenStream {
    let rpc = &leaf.rpc;
    let bindings = leaf.vars.iter().map(emit_binding);
    quote! {
        return ::core::option::Option::Some(
            ::rest_over_grpc::RouteMatch::with_bindings(#rpc, &[ #(#bindings),* ])
        );
    }
}

/// Emits a single [`Binding`] construction from a [`VarPlan`].
fn emit_binding(plan: &VarPlan) -> TokenStream {
    match plan {
        VarPlan::Span { field_path, a, b } => {
            let parts = field_path.iter().map(String::as_str);
            let a = Literal::usize_unsuffixed(*a);
            let b = Literal::usize_unsuffixed(*b);
            quote! {
                ::rest_over_grpc::Binding::new(&[#(#parts),*], &__body[__starts[#a]..__ends[#b]])
            }
        }
        VarPlan::Rest { field_path, a } => {
            let parts = field_path.iter().map(String::as_str);
            let a = Literal::usize_unsuffixed(*a);
            quote! {
                ::rest_over_grpc::Binding::new(
                    &[#(#parts),*],
                    if #a < __count { &__body[__starts[#a]..] } else { "" }
                )
            }
        }
    }
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
                    field_path: var.field_path().to_vec(),
                    first,
                    last,
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
    use super::*;
    use crate::http_method::HttpMethod;
    use crate::http_rule::HttpRule;

    fn router(rules: &[(&str, HttpMethod, &str)]) -> Router {
        let mut routes = Vec::new();
        for (rpc, method, pattern) in rules {
            let rule = HttpRule::new(*rpc, method.clone(), *pattern);
            routes.extend(rule.lower().expect("valid rule"));
        }
        Router::new(routes)
    }

    #[test]
    fn generated_code_is_valid_rust() {
        let r = router(&[
            ("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}"),
            ("ListBooks", HttpMethod::Get, "/v1/shelves/{shelf}/books"),
            ("CreateBook", HttpMethod::Post, "/v1/shelves/{shelf}/books"),
            ("GetName", HttpMethod::Get, "/v1/{name=shelves/*/books/**}"),
        ]);
        let code = r.generate();
        let file: syn::File = syn::parse2(code).expect("generated router must be syntactically valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("pub fn resolve"));
        assert!(pretty.contains("\"GetShelf\""));
    }

    #[test]
    fn empty_router_is_valid() {
        let r = Router::new(Vec::new());
        let file: syn::File = syn::parse2(r.generate()).expect("valid");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("resolve"));
    }

    #[test]
    fn custom_verb_router_is_valid() {
        let r = router(&[
            ("Get", HttpMethod::Get, "/v1/shelves/{shelf}"),
            ("Archive", HttpMethod::Post, "/v1/shelves/{shelf}:archive"),
        ]);
        let file: syn::File = syn::parse2(r.generate()).expect("valid");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("split_verb"));
        assert!(pretty.contains("\"archive\""));
    }

    #[test]
    fn same_method_leaves_are_grouped_for_custom_verbs() {
        let r = router(&[
            ("Inspect", HttpMethod::Get, "/v1/shelves/{shelf}:inspect"),
            ("Watch", HttpMethod::Get, "/v1/shelves/{shelf}:watch"),
        ]);
        let file: syn::File = syn::parse2(r.generate()).expect("valid");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("Some(\"inspect\")"));
        assert!(pretty.contains("Some(\"watch\")"));
    }

    #[test]
    fn flatten_handles_all_current_segment_shapes() {
        let top_level = HttpRule::new("Any", HttpMethod::Get, "/v1/*/**").lower().expect("valid");
        let (atoms, vars) = flatten(top_level[0].template().segments());
        assert_eq!(atoms.len(), 3);
        assert!(matches!(atoms[0], Atom::Literal(ref lit) if lit == "v1"));
        assert!(matches!(atoms[1], Atom::Single));
        assert!(matches!(atoms[2], Atom::Rest));
        assert!(vars.is_empty());

        let subtemplate = HttpRule::new("Named", HttpMethod::Get, "/v1/{name=a/*/**}").lower().expect("valid");
        let (atoms, vars) = flatten(subtemplate[0].template().segments());
        assert_eq!(atoms.len(), 4);
        assert!(matches!(atoms[2], Atom::Single));
        assert!(matches!(atoms[3], Atom::Rest));
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].field_path, ["name"]);
        assert_eq!(vars[0].first, 1);
        assert_eq!(vars[0].last, 3);
    }
}
