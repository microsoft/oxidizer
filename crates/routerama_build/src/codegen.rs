// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static resolver code generation.
//!
//! The [`Generator`](crate::Generator) lowers a set of [`Route`]s into a route
//! `enum` plus a zero-sized resolver whose `resolve` method matches an HTTP
//! method + path against the routes. Because the route set is fully known at
//! build time, the routes are lowered into a **compile-time trie**: a nested `match` over the path's
//! `/`-separated segments. Unlike a general-purpose runtime trie, there are no
//! node structures, no bounds-checked node traversal, and no per-request
//! allocation — the segment offsets are scanned into stack buffers
//! ([`routerama::codegen_helpers::scan_segments`]) and the captured path variables are
//! returned inline in the matched `Route` variant's `&'p str` fields (borrowed
//! from the request path).
//!
//! At each trie node the branches are tried most-specific-first — literal
//! segments before intra-segment affix parameters before a single-segment
//! wildcard before a `**` catch-all, and a deeper (longer) match before a
//! shorter one — so overlapping templates resolve deterministically.
//!
//! [`routerama::codegen_helpers::scan_segments`]: https://docs.rs/routerama

use std::collections::BTreeMap;

use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::quote;

use crate::generator_builder::GeneratorBuilder;
use crate::route::Route;
use crate::trie::{Leaf, Node, VarPlan, affix_edges_in_match_order, build_trie, capture_field_names, field_name, is_valid_variant};

/// Generates a static resolver for `routes` with emission controlled by `builder`.
///
/// The single entry point behind [`Generator::generate`](crate::Generator::generate);
/// the customer-facing API is the [`Generator`](crate::Generator) type.
pub(crate) fn generate(routes: &[Route], builder: &GeneratorBuilder) -> TokenStream {
    let runtime = builder.runtime_tokens();
    let visibility = builder.visibility_tokens();
    let route_enum = builder.route_type_ident();

    let trie = build_trie(routes);
    let root = trie.root;
    let any_verb = trie.any_verb;
    let cap = trie.max_segments;

    let body_bind = if any_verb {
        quote! { let (__body, __verb) = #runtime::split_verb(path); }
    } else {
        quote! { let __body: &str = path; }
    };

    let cap_lit = Literal::usize_unsuffixed(cap);
    let scan = quote! {
        let mut __starts = [0usize; #cap_lit];
        let mut __ends = [0usize; #cap_lit];
        let __count = #runtime::scan_segments(__body, &mut __starts, &mut __ends);
    };

    let match_tree = emit_node(&root, 0, any_verb, &route_enum, runtime);

    let mut conflicts = Vec::new();
    detect_conflicts(&root, "", &mut conflicts);
    let conflict_errors = conflicts.iter().map(|message| quote! { ::core::compile_error!(#message); });

    // The route enum callers `match` on (one field-carrying variant per route
    // name), plus any `compile_error!`s the route set requires.
    let (variants, variant_errors) = collect_variants(routes);
    // The enum is lifetime-parameterized only when at least one variant carries a
    // captured `&'p str` field; otherwise `'p` would be unused.
    let route_generic = variants.iter().any(|variant| !variant.fields.is_empty());
    let route_ty = if route_generic {
        quote! { #route_enum<'p> }
    } else {
        quote! { #route_enum }
    };

    // The resolve body: bind method/path, scan the path into segments, then walk
    // the compile-time match tree. It lives in the resolver ZST's `resolve`.
    let resolve_body = quote! {
        let method: &str = ::core::convert::AsRef::<str>::as_ref(&method);
        let path: &'p str = ::core::convert::AsRef::<str>::as_ref(path);
        #(#conflict_errors)*
        #body_bind
        #scan
        #match_tree
        ::core::option::Option::None
    };

    // The resolver ZST is mandatory; it defaults to `{RouteEnum}Resolver` (with
    // the enum's visibility) when the caller does not name one.
    let (resolver_visibility, resolver_name): (TokenStream, String) = match builder.resolver_spec() {
        Some((visibility, name)) => (visibility.clone(), name.to_owned()),
        None => (visibility.clone(), format!("{route_enum}Resolver")),
    };

    let enum_def = emit_optional_enum_def(visibility, &route_enum, &variants, route_generic, builder.emits_enum());
    let resolver_and_match = emit_resolver_and_match(
        &route_enum,
        &route_ty,
        &variants,
        runtime,
        &resolver_visibility,
        &resolver_name,
        &resolve_body,
    );

    quote! {
        #(#variant_errors)*

        #enum_def

        #resolver_and_match
    }
}

/// Emits the mandatory zero-sized [`Resolver`](routerama::Resolver) type (which
/// carries the `resolve` logic) plus the [`RouteMatch`](routerama::RouteMatch)
/// impl for the route enum. Resolving always goes through the resolver value;
/// the enum itself is only the match/result type.
fn emit_resolver_and_match(
    route_enum: &Ident,
    route_ty: &TokenStream,
    variants: &[VariantSpec],
    runtime: &TokenStream,
    resolver_visibility: &TokenStream,
    resolver_name: &str,
    resolve_body: &TokenStream,
) -> TokenStream {
    let resolver_ident = Ident::new(resolver_name, Span::call_site());
    let resolver_doc = format!(
        " A zero-sized [`Resolver`](routerama::Resolver) that resolves a request to a [`{route_enum}`]. Composes with a dynamic resolver via `EitherResolver`. Generated by `routerama_build`."
    );
    let name_arms = variants.iter().map(|variant| {
        let ident = &variant.ident;
        let name = &variant.name;
        if variant.fields.is_empty() {
            quote! { #route_enum::#ident => #name, }
        } else {
            quote! { #route_enum::#ident { .. } => #name, }
        }
    });
    // One capture arm per variant: a field-carrying variant matches its captured
    // field name to the requested key; a unit variant captures nothing.
    let capture_arms = variants.iter().map(|variant| {
        let ident = &variant.ident;
        if variant.fields.is_empty() {
            quote! { #route_enum::#ident => ::core::option::Option::None, }
        } else {
            // Bind each variant field under a `__cap_` prefix so it can never
            // shadow the `__key` parameter matched below — a captured field may
            // itself be named `__key` (or any other local name).
            let bindings: Vec<Ident> = variant
                .fields
                .iter()
                .map(|field| Ident::new(&format!("__cap_{field}"), field.span()))
                .collect();
            let field_pats = variant
                .fields
                .iter()
                .zip(&bindings)
                .map(|(field, binding)| quote! { #field: #binding });
            // Match the *original* variable name (e.g. `shelf.id`, `type`), not
            // the sanitized field identifier, so `capture` takes the name as
            // written in the template.
            let key_arms = variant.keys.iter().zip(&bindings).map(|(key, binding)| {
                quote! { #key => ::core::option::Option::Some(*#binding), }
            });
            quote! {
                #route_enum::#ident { #(#field_pats),* } => match __key {
                    #(#key_arms)*
                    _ => ::core::option::Option::None,
                },
            }
        }
    });
    let allow = quote! {
        #[allow(
            clippy::all,
            clippy::pedantic,
            clippy::nursery,
            clippy::restriction,
            dead_code,
            unused,
            reason = "code generated by routerama_build"
        )]
    };
    quote! {
        #[doc = #resolver_doc]
        #[derive(::core::clone::Clone, ::core::marker::Copy, ::core::fmt::Debug, ::core::default::Default)]
        #allow
        #resolver_visibility struct #resolver_ident;

        #allow
        impl #runtime::Resolver for #resolver_ident {
            type Match<'p> = #route_ty;

            #[inline]
            fn resolve<'p, __P>(&'p self, method: impl ::core::convert::AsRef<str>, path: &'p __P) -> ::core::option::Option<#route_ty>
            where
                __P: ::core::convert::AsRef<str> + ?::core::marker::Sized,
            {
                #resolve_body
            }
        }

        #allow
        impl<'p> #runtime::RouteMatch<'p> for #route_ty {
            #[inline]
            fn name(&self) -> &str {
                match *self {
                    #(#name_arms)*
                }
            }

            #[inline]
            fn capture(&self, __key: &str) -> ::core::option::Option<&'p str> {
                match self {
                    #(#capture_arms)*
                }
            }
        }
    }
}

/// Emits the `Route` enum definition (one struct/unit variant per route), with
/// the canonical derive set. Only used by the enum-generating path.
fn emit_enum_def(visibility: &TokenStream, route_enum: &Ident, variants: &[VariantSpec], lt: &TokenStream) -> TokenStream {
    let variant_defs = variants.iter().map(|variant| {
        let ident = &variant.ident;
        if variant.fields.is_empty() {
            quote! { #ident }
        } else {
            let fields = variant.fields.iter().map(|field| quote! { #field: &'p str });
            quote! { #ident { #(#fields),* } }
        }
    });
    quote! {
        /// A resolved route, one variant per route name. Produced by the
        /// `resolve` method of the generated resolver (its `Resolver` impl).
        ///
        /// A route that captures `{variable}` path segments is a struct variant
        /// carrying them as `&'p str` fields (borrowed from the request path); a
        /// route with no variables is a unit variant. Matching on it is an `O(1)`
        /// jump table. Generated by `routerama_build`.
        #[derive(
            ::core::clone::Clone,
            ::core::marker::Copy,
            ::core::fmt::Debug,
            ::core::cmp::PartialEq,
            ::core::cmp::Eq,
            ::core::cmp::PartialOrd,
            ::core::cmp::Ord,
            ::core::hash::Hash
        )]
        #[allow(
            non_camel_case_types,
            missing_docs,
            clippy::all,
            clippy::pedantic,
            clippy::nursery,
            clippy::restriction,
            dead_code,
            unused,
            reason = "code generated by routerama_build"
        )]
        #visibility enum #route_enum #lt {
            #(#variant_defs,)*
        }
    }
}

/// Emits the `Route` enum definition on the enum-generating (`Generator`) path;
/// emits nothing on the `#[resolver]` path, where the caller wrote the enum.
///
/// The enum carries no inherent `resolve`/`name`: resolving goes through the
/// zero-sized resolver and `name` comes from the `RouteMatch` impl.
fn emit_optional_enum_def(
    visibility: &TokenStream,
    route_enum: &Ident,
    variants: &[VariantSpec],
    generic: bool,
    emit_enum: bool,
) -> TokenStream {
    if !emit_enum {
        return quote! {};
    }
    let lt = if generic {
        quote! { <'p> }
    } else {
        quote! {}
    };
    emit_enum_def(visibility, route_enum, variants, &lt)
}

/// One `Route` enum variant: its identifier, the declared name string
/// ([`Route::name`] recovers it), the ordered field identifiers for the path
/// variables it captures, and each variable's original (unmangled) name used as
/// its runtime `capture` key. `fields`/`keys` are empty for a route with no
/// `{variable}` segments, which becomes a unit variant.
struct VariantSpec {
    ident: Ident,
    name: String,
    fields: Vec<Ident>,
    keys: Vec<String>,
}

/// Collects one [`VariantSpec`] per distinct route name (first-appearance order)
/// and any `compile_error!`s the route set requires: a name that cannot name a
/// variant, a name whose routes capture inconsistent variable sets (a name maps
/// to one variant with a fixed field list, so the captures must agree), or a
/// variant with two variables that collapse to the same field identifier.
fn collect_variants(routes: &[Route]) -> (Vec<VariantSpec>, Vec<TokenStream>) {
    let mut specs: Vec<VariantSpec> = Vec::new();
    let mut errors: Vec<TokenStream> = Vec::new();
    for route in routes {
        let names = capture_field_names(route.template().segments());
        let fields: Vec<Ident> = names.iter().map(|name| field_ident(name)).collect();
        let keys: Vec<String> = names.iter().map(|name| name.join(".")).collect();

        if let Some(spec) = specs.iter().find(|spec| spec.name == route.name()) {
            // Struct-variant fields are named, so their positional order is
            // irrelevant: `emit_return` fills them by name. Compare the field
            // *sets* (sorted) so same-name routes that capture the same variables
            // in a different segment order are accepted, not falsely rejected.
            let mut existing: Vec<String> = spec.fields.iter().map(ToString::to_string).collect();
            let mut incoming: Vec<String> = fields.iter().map(ToString::to_string).collect();
            existing.sort();
            incoming.sort();
            if existing != incoming {
                let message = format!(
                    "route name `{}` is bound to routes that capture different path variables ({} vs {}); \
                     a route name maps to one enum variant with a fixed set of fields, so give the routes distinct names",
                    route.name(),
                    fmt_fields(&existing),
                    fmt_fields(&incoming),
                );
                errors.push(quote! { ::core::compile_error!(#message); });
            }
            continue;
        }

        if !is_valid_variant(route.name()) {
            let message = format!(
                "route name `{}` is not a valid Rust identifier; route names must be valid identifiers so they can name a route enum variant",
                route.name()
            );
            errors.push(quote! { ::core::compile_error!(#message); });
        } else if matches!(route.name(), "name" | "resolve") {
            // `name` is a generated `RouteMatch` method and `resolve` is the
            // resolver's method; reserving these route names avoids surprising
            // shadowing in the generated impls. Rename the route instead.
            let message = format!("route name `{}` collides with a generated method; rename the route", route.name());
            errors.push(quote! { ::core::compile_error!(#message); });
        }

        let mut seen: Vec<String> = Vec::new();
        for field in &fields {
            let field = field.to_string();
            if seen.contains(&field) {
                let message = format!(
                    "route name `{}` captures two path variables that map to the same field `{}`; rename one of them",
                    route.name(),
                    field
                );
                errors.push(quote! { ::core::compile_error!(#message); });
            } else {
                seen.push(field);
            }
        }

        specs.push(VariantSpec {
            ident: variant_ident(route.name()),
            name: route.name().to_owned(),
            fields,
            keys,
        });
    }
    (specs, errors)
}

/// Formats a variant's field-name list for a diagnostic, e.g. `{shelf, book}` or
/// `none`.
fn fmt_fields(fields: &[String]) -> String {
    if fields.is_empty() {
        "none".to_owned()
    } else {
        format!("{{{}}}", fields.join(", "))
    }
}

/// The struct-variant field identifier for a captured variable, whose (possibly
/// dotted) name is given as its path segments — the token [`Ident`] form of
/// [`route_field_name`](crate::trie::route_field_name).
fn field_ident(name: &[String]) -> Ident {
    Ident::new(&field_name(&name.join("_")), Span::call_site())
}

/// The enum-variant identifier for a route `name`. Valid identifiers that do not
/// collide with a generated method are used verbatim; anything else is sanitized
/// to a deterministic placeholder so the output still tokenizes (a
/// `compile_error!` reports the real problem).
fn variant_ident(name: &str) -> Ident {
    if is_valid_variant(name) && !matches!(name, "name" | "resolve") {
        Ident::new(name, Span::call_site())
    } else {
        let sanitized: String = name
            .chars()
            .map(|c| if c == '_' || c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        Ident::new(&format!("_R_{sanitized}"), Span::call_site())
    }
}

/// Walks the trie and records a human-readable message for every conflicting
/// route set: two or more routes that reach the same node with the same HTTP
/// method and custom verb, and so match an identical set of requests. Only the
/// first such route would ever be reached, so this is always a mistake.
fn detect_conflicts(node: &Node, prefix: &str, out: &mut Vec<String>) {
    check_bucket(&node.exact, prefix, false, out);
    check_bucket(&node.rest, prefix, true, out);
    for (literal, child) in &node.literals {
        detect_conflicts(child, &format!("{prefix}/{literal}"), out);
    }
    for ((affix_prefix, affix_suffix), child) in &node.affix {
        detect_conflicts(child, &format!("{prefix}/{affix_prefix}{{}}{affix_suffix}"), out);
    }
    if let Some(single) = &node.single {
        detect_conflicts(single, &format!("{prefix}/*"), out);
    }
}

/// Groups a node's leaves by `(method, verb)` and reports any group with more
/// than one route as a conflict.
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
            let verb = verb.map(|v| format!(":{v}")).unwrap_or_default();
            out.push(format!(
                "conflicting routes: `{method} {path}{verb}` maps to multiple names ({}); \
                 each HTTP method and path may resolve to only one route",
                names.join(", ")
            ));
        }
    }
}

/// Emits the matching code for a trie `node` reached after consuming `depth`
/// segments. On a match the code `return`s; otherwise it falls through.
fn emit_node(node: &Node, depth: usize, any_verb: bool, route_enum: &Ident, runtime: &TokenStream) -> TokenStream {
    let depth_lit = Literal::usize_unsuffixed(depth);

    // Branch taken when a segment exists at `depth`: try literal edges, then any
    // intra-segment affix edges, then the single-segment wildcard, then any `**`
    // catch-all (most-specific-first).
    let has_literals = !node.literals.is_empty();
    let has_affix = !node.affix.is_empty();
    let needs_seg = has_literals || has_affix;
    let seg_bind = if needs_seg {
        quote! { let __seg: &[u8] = #runtime::seg_bytes(__body, __starts[#depth_lit], __ends[#depth_lit]); }
    } else {
        quote! {}
    };
    let literal_arms = node.literals.iter().map(|(lit, child)| {
        let child = emit_node(child, depth + 1, any_verb, route_enum, runtime);
        // Match the segment as *bytes* (`b"lit"`), not `&str`: a `&str` range index
        // runs a char-boundary check at every node, and the offsets are already
        // known-valid `/`-delimited boundaries. Capture *values* stay `&str`.
        let lit = Literal::byte_string(lit.as_bytes());
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
    // An affix edge matches a segment shaped `prefix{var}suffix`: it must be long
    // enough to carry a non-empty capture and bound by the literal prefix/suffix.
    // Edges are tried longest-affix-first so the most specific one wins.
    let affix_code = affix_edges_in_match_order(node).into_iter().map(|((prefix, suffix), child)| {
        let child = emit_node(child, depth + 1, any_verb, route_enum, runtime);
        let total_lit = Literal::usize_unsuffixed(prefix.len() + suffix.len());
        let prefix_check = if prefix.is_empty() {
            quote! {}
        } else {
            let prefix = Literal::byte_string(prefix.as_bytes());
            quote! { && __seg.starts_with(#prefix) }
        };
        let suffix_check = if suffix.is_empty() {
            quote! {}
        } else {
            let suffix = Literal::byte_string(suffix.as_bytes());
            quote! { && __seg.ends_with(#suffix) }
        };
        quote! {
            if __seg.len() > #total_lit #prefix_check #suffix_check {
                #child
            }
        }
    });
    // A single-segment wildcard (`*` / `{var}`) matches exactly one non-empty
    // segment, so its subtree is guarded on a non-empty segment. Literal edges
    // need no guard; `**` captures the rest.
    let single_code = node
        .single
        .as_ref()
        .map(|child| {
            let child = emit_node(child, depth + 1, any_verb, route_enum, runtime);
            quote! {
                if __ends[#depth_lit] > __starts[#depth_lit] {
                    #child
                }
            }
        })
        .unwrap_or_default();
    let rest_dispatch = emit_leaves(&node.rest, any_verb, route_enum, runtime);

    let has_segment_branch = has_literals || has_affix || node.single.is_some();
    let segment_branch = if has_segment_branch {
        quote! {
            if __count > #depth_lit {
                #seg_bind
                #literal_match
                #(#affix_code)*
                #single_code
            }
        }
    } else {
        quote! {}
    };

    // Branch taken when the path ends here: exact leaves terminating at this node.
    let exact_dispatch = emit_leaves(&node.exact, any_verb, route_enum, runtime);
    let has_end_branch = !node.exact.is_empty();
    let end_branch = if has_end_branch {
        quote! {
            if __count == #depth_lit {
                #exact_dispatch
            }
        }
    } else {
        quote! {}
    };

    // A `**` catch-all matches any remainder length (including zero), so it is
    // emitted once here — after the more-specific literal/single/exact branches —
    // and its open-ended capture handles the empty remainder at runtime.
    let rest_branch = if node.rest.is_empty() {
        quote! {}
    } else {
        quote! {
            if __count >= #depth_lit {
                #rest_dispatch
            }
        }
    };

    quote! {
        #segment_branch
        #end_branch
        #rest_branch
    }
}

/// Emits `method` (and, when any route uses one, `:verb`) dispatch for a set of
/// leaves terminating at the same trie node, returning the matched `Route` on a hit.
fn emit_leaves(leaves: &[Leaf], any_verb: bool, route_enum: &Ident, runtime: &TokenStream) -> TokenStream {
    if leaves.is_empty() {
        return quote! {};
    }

    // Group by method (preserving first-seen order), deduplicating (method, verb)
    // pairs so no two arms are emitted for the same case.
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
                let ret = emit_return(leaf, route_enum, runtime);
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
            emit_return(entries.first().expect("each group has at least one leaf"), route_enum, runtime)
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

/// Emits a `return Some(...)` of the matched leaf's route enum variant, with its
/// captured path variables filled into the variant's named fields.
fn emit_return(leaf: &Leaf, route_enum: &Ident, runtime: &TokenStream) -> TokenStream {
    let variant = variant_ident(&leaf.name);
    let route_val = if leaf.vars.is_empty() {
        quote! { #route_enum::#variant }
    } else {
        let field_inits = leaf.vars.iter().map(|plan| {
            let field = Ident::new(plan.field(), Span::call_site());
            let value = var_slice_expr(plan, runtime);
            quote! { #field: #value }
        });
        quote! { #route_enum::#variant { #(#field_inits),* } }
    };
    quote! {
        return ::core::option::Option::Some(#route_val);
    }
}

/// Emits the expression that slices a captured path variable's value out of the
/// scanned request path (`&'p str`), used by [`emit_return`] to initialize the
/// matched variant's fields.
fn var_slice_expr(plan: &VarPlan, runtime: &TokenStream) -> TokenStream {
    match plan {
        VarPlan::Span { a, b, .. } => {
            let a = Literal::usize_unsuffixed(*a);
            let b = Literal::usize_unsuffixed(*b);
            quote! { #runtime::substr(__body, __starts[#a], __ends[#b]) }
        }
        VarPlan::Rest { a, .. } => {
            let a = Literal::usize_unsuffixed(*a);
            quote! { if #a < __count { #runtime::substr(__body, __starts[#a], __body.len()) } else { "" } }
        }
        VarPlan::Affix {
            a, prefix_len, suffix_len, ..
        } => {
            let a = Literal::usize_unsuffixed(*a);
            let prefix_lit = Literal::usize_unsuffixed(*prefix_len);
            let suffix_lit = Literal::usize_unsuffixed(*suffix_len);
            quote! { #runtime::substr(__body, __starts[#a] + #prefix_lit, __ends[#a] - #suffix_lit) }
        }
    }
}

#[cfg(test)]
mod tests {
    use http_path_template::{Grammar, PathTemplate};

    use super::*;
    use crate::generator::Generator;
    use crate::http_method::HttpMethod;

    fn rules(specs: &[(&str, HttpMethod, &str)]) -> Vec<Route> {
        specs
            .iter()
            .map(|(name, method, pattern)| {
                Route::new(
                    *name,
                    method.clone(),
                    PathTemplate::parse(pattern, Grammar::default()).expect("valid template"),
                )
            })
            .collect()
    }

    fn generated(specs: &[(&str, HttpMethod, &str)]) -> String {
        generate_code(rules(specs)).to_string()
    }

    /// Test helper: generate with the default options.
    fn generate_code(routes: impl IntoIterator<Item = Route>) -> TokenStream {
        let mut generator = Generator::new();
        generator.add_all(routes);
        generator.generate()
    }

    /// Test helper: generate with a configured builder.
    fn generate_code_with(routes: impl IntoIterator<Item = Route>, builder: GeneratorBuilder) -> TokenStream {
        let mut generator = builder.build();
        generator.add_all(routes);
        generator.generate()
    }

    fn ext_rules(specs: &[(&str, HttpMethod, &str)]) -> Vec<Route> {
        specs
            .iter()
            .map(|(name, method, pattern)| {
                Route::new(
                    *name,
                    method.clone(),
                    PathTemplate::parse(pattern, Grammar::default().with_segment_affixes()).expect("valid template"),
                )
            })
            .collect()
    }

    #[test]
    fn generates_a_resolve_function() {
        let code = generated(&[("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")]);
        assert!(code.contains("fn resolve"));
        assert!(code.contains("GetShelf"));
        assert!(code.contains(":: routerama :: codegen_helpers :: scan_segments") || code.contains("scan_segments"));
    }

    #[test]
    fn router_type_emits_a_zst_resolver_and_route_match_impl() {
        // Opting in via `resolver_type` emits a ZST `Resolver` (under the given
        // name) + a `RouteMatch` impl, so the static resolver plugs into the
        // runtime resolution-trait abstraction.
        let options = GeneratorBuilder::default().resolver_type(quote! { pub }, "ShelfResolver");
        let file: syn::File = syn::parse2(generate_code_with(
            rules(&[("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")]),
            options,
        ))
        .expect("valid Rust");
        let code = prettyplease::unparse(&file).replace(' ', "");
        assert!(code.contains("structShelfResolver"), "ZST resolver uses the given name: {code}");
        assert!(code.contains("ResolverforShelfResolver"), "Resolver impl: {code}");
        assert!(code.contains("RouteMatch<'p>forRoute"), "RouteMatch impl: {code}");
        assert!(
            code.contains("\"shelf\"=>") && code.contains("Some(*__cap_shelf)"),
            "capture arm: {code}"
        );
    }

    #[test]
    fn a_default_resolver_is_emitted() {
        // A resolver ZST is always emitted; with no `resolver_type` it defaults to
        // `{RouteEnum}Resolver` (here `RouteResolver`), alongside the `RouteMatch`
        // impl on the enum.
        let file: syn::File =
            syn::parse2(generate_code(rules(&[("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")]))).expect("valid Rust");
        let code = prettyplease::unparse(&file).replace(' ', "");
        assert!(code.contains("structRouteResolver;"), "default resolver ZST: {code}");
        assert!(code.contains("RouteMatch"), "RouteMatch impl: {code}");
        assert!(code.contains("::Resolver"), "Resolver trait impl: {code}");
    }

    #[test]
    fn generates_a_route_enum_with_a_variant_per_name() {
        // The `Route` enum drives `O(1)` jump-table dispatch: one variant per
        // distinct name, deduplicated (a name bound to two routes with the same
        // captures yields one variant), plus a `name` accessor recovering the
        // declared string. A capturing route becomes a struct variant.
        let file: syn::File = syn::parse2(generate_code(rules(&[
            ("ListShelves", HttpMethod::Get, "/v1/shelves"),
            ("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}"),
            ("GetShelf", HttpMethod::Delete, "/v1/shelves/{shelf}"),
        ])))
        .expect("valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("enum Route"), "{pretty}");
        assert!(pretty.contains("fn name"), "{pretty}");
        // The enum body runs from the `enum` keyword to the following `impl`.
        let enum_src = {
            let start = pretty.find("enum Route").expect("enum is emitted");
            let rest = &pretty[start..];
            &rest[..rest.find("impl").expect("an impl follows the enum")]
        };
        // Two routes share the `GetShelf` name and captures, so it appears once
        // as a variant — a struct variant carrying its `{shelf}` capture.
        assert_eq!(enum_src.matches("GetShelf").count(), 1, "{enum_src}");
        assert_eq!(enum_src.matches("ListShelves").count(), 1, "{enum_src}");
        assert!(enum_src.contains("GetShelf { shelf"), "{enum_src}");
        // `resolve` returns the matched route directly.
        assert!(
            pretty.contains("-> Option<Route<'p>>") || pretty.contains("Option<Route<'p>>"),
            "{pretty}"
        );
    }

    #[test]
    fn resolve_lives_on_the_resolver_and_the_enum_name_is_configurable() {
        // With a configured `route_type`, the enum is renamed and `resolve` lives
        // on its zero-sized resolver, so several resolvers can coexist in one
        // scope and be called as `NameResolver.resolve(..)`.
        let options = GeneratorBuilder::default().route_type("BookRoute");
        let file: syn::File =
            syn::parse2(generate_code_with(rules(&[("GetBook", HttpMethod::Get, "/books/{book}")]), options)).expect("valid Rust");
        let flat = prettyplease::unparse(&file).replace(' ', "");
        assert!(flat.contains("enumBookRoute<'p>"), "{flat}");
        // `resolve` lives on the resolver ZST's `Resolver` impl, not on the enum.
        assert!(flat.contains("structBookRouteResolver;"), "{flat}");
        assert!(flat.contains("fnresolve<'p,__P>"), "{flat}");
        assert!(flat.contains("::core::option::Option<BookRoute<'p>>"), "{flat}");
    }

    #[test]
    fn non_capturing_resolver_has_a_unit_match() {
        // With no captures the enum is non-generic; its resolver's `Match` is the
        // bare `Route`.
        let file: syn::File = syn::parse2(generate_code(rules(&[("Health", HttpMethod::Get, "/health")]))).expect("valid Rust");
        let flat = prettyplease::unparse(&file).replace(' ', "");
        assert!(flat.contains("structRouteResolver;"), "{flat}");
        assert!(flat.contains("fnresolve<'p,__P>"), "{flat}");
        assert!(flat.contains("::core::option::Option<Route>"), "{flat}");
    }

    #[test]
    fn generated_enum_implements_route_match_and_a_resolver() {
        // The enum implements `RouteMatch`; resolving goes through the emitted ZST
        // `Resolver`.
        // Capturing: the enum is `Route<'p>`.
        let capturing =
            prettyplease::unparse(&syn::parse2(generate_code(rules(&[("GetBook", HttpMethod::Get, "/books/{book}")]))).expect("valid"))
                .replace(' ', "");
        assert!(
            capturing.contains("impl<'p>::routerama::codegen_helpers::RouteMatch<'p>forRoute<'p>{"),
            "{capturing}"
        );
        assert!(
            capturing.contains("::routerama::codegen_helpers::ResolverforRouteResolver{"),
            "{capturing}"
        );

        // Capture-less: the enum is non-generic.
        let unit = prettyplease::unparse(&syn::parse2(generate_code(rules(&[("Health", HttpMethod::Get, "/health")]))).expect("valid"))
            .replace(' ', "");
        assert!(
            unit.contains("impl<'p>::routerama::codegen_helpers::RouteMatch<'p>forRoute{"),
            "{unit}"
        );

        // The trait path honors a custom `runtime_path`.
        let rerouted = prettyplease::unparse(
            &syn::parse2(generate_code_with(
                rules(&[("GetBook", HttpMethod::Get, "/books/{book}")]),
                GeneratorBuilder::default().runtime_path(quote::quote! { ::rt }),
            ))
            .expect("valid"),
        )
        .replace(' ', "");
        assert!(rerouted.contains("impl<'p>::rt::RouteMatch<'p>forRoute<'p>{"), "{rerouted}");
    }

    #[test]
    fn capturing_routes_become_struct_variants_and_non_capturing_stay_unit() {
        // A route with `{variable}` segments carries them as named `&'p str`
        // fields; a route with none is a unit variant. The enum is
        // lifetime-parameterized because at least one variant carries a field.
        let file: syn::File = syn::parse2(generate_code(rules(&[
            ("ListShelves", HttpMethod::Get, "/v1/shelves"),
            ("GetReview", HttpMethod::Get, "/v1/shelves/{shelf}/reviews/{review}"),
        ])))
        .expect("valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("enum Route<'p>"), "{pretty}");
        let enum_src = {
            let start = pretty.find("enum Route").expect("enum is emitted");
            let rest = &pretty[start..];
            &rest[..rest.find("impl").expect("an impl follows the enum")]
        };
        assert!(
            enum_src.contains("ListShelves,") || enum_src.contains("ListShelves ,"),
            "{enum_src}"
        );
        assert!(enum_src.contains("GetReview { shelf: &'p str, review: &'p str }"), "{enum_src}");
        // The resolved variant is constructed with its captured fields.
        assert!(pretty.contains("Route::GetReview"), "{pretty}");
    }

    #[test]
    fn a_dotted_variable_becomes_an_underscored_field() {
        // A dotted capture like `{shelf.id}` maps to a `shelf_id` field.
        let file: syn::File =
            syn::parse2(generate_code(rules(&[("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf.id}")]))).expect("valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("shelf_id: &'p str"), "{pretty}");
    }

    #[test]
    fn a_router_with_no_captures_has_a_non_generic_route_enum() {
        // With no route capturing a variable, `Route` needs no lifetime.
        let file: syn::File = syn::parse2(generate_code(rules(&[("ListShelves", HttpMethod::Get, "/v1/shelves")]))).expect("valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("enum Route {"), "{pretty}");
        assert!(!pretty.contains("enum Route<"), "{pretty}");
    }

    #[test]
    fn reusing_a_name_with_different_captures_is_a_compile_error() {
        let code = generated(&[
            ("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}"),
            ("GetShelf", HttpMethod::Get, "/v2/shelves/{shelf}/books/{book}"),
        ]);
        assert!(code.contains("compile_error"), "{code}");
        assert!(code.contains("capture different path variables"), "{code}");
    }

    #[test]
    fn reusing_a_name_with_the_same_captures_in_a_different_order_is_allowed() {
        // Both routes capture `{user, repo}` — the same set — just in a different
        // segment order. The variant's fields are named (order-independent), so
        // this is valid and must NOT be rejected as a capture conflict.
        let code = generated(&[
            ("UserRepo", HttpMethod::Get, "/users/{user}/repos/{repo}"),
            ("UserRepo", HttpMethod::Get, "/repos/{repo}/users/{user}"),
        ]);
        assert!(!code.contains("compile_error"), "{code}");
        assert!(!code.contains("capture different path variables"), "{code}");
    }

    #[test]
    fn reusing_a_name_with_a_fieldless_binding_renders_none_in_the_diagnostic() {
        // One binding captures nothing and the other captures `{book}`, so the
        // conflict diagnostic formats the empty field set as `none`.
        let code = generated(&[
            ("GetShelf", HttpMethod::Get, "/v1/shelves"),
            ("GetShelf", HttpMethod::Get, "/v1/shelves/{book}"),
        ]);
        assert!(code.contains("compile_error"), "{code}");
        assert!(code.contains("none vs"), "{code}");
    }

    #[test]
    fn a_route_capturing_the_same_field_twice_is_a_compile_error() {
        // A single route whose template binds two variables to the same field
        // cannot map them into one struct field, so it is rejected.
        let code = generated(&[("GetPair", HttpMethod::Get, "/{shelf}/x/{shelf}")]);
        assert!(code.contains("compile_error"), "{code}");
        assert!(code.contains("same field"), "{code}");
    }

    #[test]
    fn a_route_named_like_a_generated_method_is_a_compile_error() {
        // A variant named `name` or `resolve` collides with a generated method
        // (`RouteMatch::name` / the resolver's `resolve`), so it is rejected with
        // a clear diagnostic and sanitized so the rest of the output still
        // compiles.
        for reserved in ["name", "resolve"] {
            let code = generated(&[(reserved, HttpMethod::Get, "/x")]);
            assert!(code.contains("compile_error"), "{reserved}: {code}");
            assert!(code.contains("collides with a generated method"), "{reserved}: {code}");
        }
    }

    #[test]
    fn capturing_a_field_named_like_the_key_parameter_compiles() {
        // A captured field named `__key` must not shadow the `capture` method's
        // key parameter: the generated variant fields are bound under a `__cap_`
        // prefix, so the emitted `RouteMatch` impl is valid Rust.
        let options = GeneratorBuilder::default().resolver_type(quote! {}, "KeyRouter");
        let file: syn::File = syn::parse2(generate_code_with(
            rules(&[("GetKey", HttpMethod::Get, "/things/{__key}")]),
            options,
        ))
        .expect("a `{__key}` capture with a named resolver must still emit valid Rust");
        let code = prettyplease::unparse(&file).replace(' ', "");
        assert!(
            code.contains("\"__key\"=>") && code.contains("Some(*__cap___key)"),
            "capture arm: {code}"
        );
    }

    #[test]
    fn resolve_constructs_the_variant_with_the_captured_field_name() {
        // The `return Some(Route::Variant { field: ... })` in the generated
        // `resolve` names each field after its captured variable (`{book}` →
        // `book`), matching the enum variant's field, so the emitted code is
        // internally consistent.
        let file: syn::File = syn::parse2(generate_code(rules(&[("GetBook", HttpMethod::Get, "/books/{book}")]))).expect("valid Rust");
        let flat: String = prettyplease::unparse(&file).split_whitespace().collect();
        assert!(
            flat.contains("Route::GetBook{book:::routerama::codegen_helpers::substr(__body"),
            "{flat}"
        );
    }

    #[test]
    fn route_enum_derives_the_canonical_traits() {
        // The enum is `Copy` and derives the full canonical comparison/hashing
        // set, so callers can freely compare, order, hash, and store it (e.g. in
        // a `BTreeMap`/`HashMap` keyed by route).
        let file: syn::File =
            syn::parse2(generate_code(rules(&[("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")]))).expect("valid Rust");
        let pretty = prettyplease::unparse(&file);
        for trait_name in ["Clone", "Copy", "Debug", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash"] {
            assert!(pretty.contains(trait_name), "derive is missing `{trait_name}`: {pretty}");
        }
    }

    #[test]
    fn route_enum_does_not_implement_display_or_as_ref_str() {
        // Presentation/conversion traits are the caller's choice; the macro only
        // exposes the route name via `RouteMatch::name`.
        let file: syn::File =
            syn::parse2(generate_code(rules(&[("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")]))).expect("valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(!pretty.contains("Display for Route"), "{pretty}");
        assert!(!pretty.contains("AsRef<str> for Route"), "{pretty}");
    }

    #[test]
    fn a_name_that_is_not_an_identifier_is_a_compile_error() {
        let code = generated(&[("0:GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")]);
        assert!(code.contains("compile_error"), "{code}");
        assert!(code.contains("not a valid Rust identifier"), "{code}");
    }

    #[test]
    fn is_valid_variant_accepts_only_non_keyword_identifiers() {
        // Valid identifiers (used verbatim as enum variants).
        assert!(is_valid_variant("GetShelf"));
        assert!(is_valid_variant("_private"));
        assert!(is_valid_variant("Route2"));

        // Empty, non-identifier-first, and non-identifier-rest are all rejected.
        assert!(!is_valid_variant(""));
        assert!(!is_valid_variant("0Route"));
        assert!(!is_valid_variant("Get-Shelf"));

        // Reserved words cannot name a variant.
        assert!(!is_valid_variant("match"));
        assert!(!is_valid_variant("_"));
    }

    #[test]
    fn variant_ident_sanitizes_non_identifier_names() {
        // A valid name is used verbatim; an invalid one is sanitized to a
        // deterministic placeholder (the real error is a `compile_error!`).
        assert_eq!(variant_ident("GetShelf").to_string(), "GetShelf");
        assert_eq!(variant_ident("0:Get").to_string(), "_R_0_Get");
    }

    #[test]
    fn generated_router_parses_as_valid_rust() {
        let tokens = generate_code(rules(&[
            ("ListShelves", HttpMethod::Get, "/v1/shelves"),
            ("CreateShelf", HttpMethod::Post, "/v1/shelves"),
            ("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}"),
            ("ArchiveShelf", HttpMethod::Post, "/v1/shelves/{shelf}:archive"),
            ("GetTree", HttpMethod::Get, "/v1/tree/{path=**}"),
        ]));
        let file: syn::File = syn::parse2(tokens).expect("generated router is valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("fn resolve"));
        assert!(pretty.contains("routerama"));
    }

    #[test]
    fn empty_router_is_valid_and_matches_nothing() {
        let file: syn::File = syn::parse2(generate_code(std::iter::empty())).expect("valid");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("fn resolve"));
    }

    #[test]
    fn conflicting_routes_emit_a_compile_error() {
        // Two GET routes with the same path shape map to different names; only the
        // first could ever match.
        let code = generated(&[
            ("GetBookA", HttpMethod::Get, "/v1/books/{book}"),
            ("GetBookB", HttpMethod::Get, "/v1/books/{other}"),
        ]);
        assert!(code.contains("compile_error"), "{code}");
        assert!(code.contains("conflicting routes"), "{code}");
    }

    #[test]
    fn no_verb_split_when_no_route_uses_a_verb() {
        // Without any custom verb, the generated code binds `__body = path`
        // directly rather than calling `split_verb`.
        let code = generated(&[("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")]);
        assert!(!code.contains("split_verb"), "{code}");

        let with_verb = generated(&[("ArchiveShelf", HttpMethod::Post, "/v1/shelves/{shelf}:archive")]);
        assert!(with_verb.contains("split_verb"), "{with_verb}");
    }

    #[test]
    fn rest_capture_emitted_once() {
        // The `**` capture is sliced once for the matched variant's field; the
        // underlying trie produces exactly one capture site.
        let code = generate_code(rules(&[("GetTree", HttpMethod::Get, "/v1/{path=**}")]))
            .to_string()
            .replace(' ', "");
        assert_eq!(
            code.matches("__starts[1],__body.len()").count(),
            1,
            "rest capture emitted once for the variant field: {code}"
        );
    }

    #[test]
    fn affix_route_generates_valid_rust_with_guards() {
        let tokens = generate_code(ext_rules(&[("GetFile", HttpMethod::Get, "/files/{name}.json")]));
        let file: syn::File = syn::parse2(tokens).expect("generated router is valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("ends_with"), "{pretty}");
        assert!(pretty.contains("GetFile"), "{pretty}");
    }

    #[test]
    fn affix_prefix_and_suffix_emit_both_guards() {
        let code = generated_ext(&[("GetImg", HttpMethod::Get, "/img-{id}.png")]);
        assert!(code.contains("starts_with"), "{code}");
        assert!(code.contains("ends_with"), "{code}");
    }

    #[test]
    fn affix_binding_slices_the_segment_middle() {
        // The captured value is the segment minus its literal prefix/suffix.
        let code = generated_ext(&[("GetFile", HttpMethod::Get, "/files/{name}.json")]).replace(' ', "");
        assert!(code.contains("__starts[1]+0,__ends[1]-5"), "{code}");
    }

    #[test]
    fn literal_takes_priority_over_affix_at_same_depth() {
        let tokens = generate_code(ext_rules(&[
            ("GetLatest", HttpMethod::Get, "/files/latest.json"),
            ("GetFile", HttpMethod::Get, "/files/{name}.json"),
        ]));
        // Both coexist; the literal edge is matched before the affix guard.
        let file: syn::File = syn::parse2(tokens).expect("valid Rust");
        let pretty = prettyplease::unparse(&file);
        let literal_pos = pretty.find("latest.json").expect("literal arm emitted");
        let affix_pos = pretty.find("ends_with").expect("affix guard emitted");
        assert!(literal_pos < affix_pos, "literal must precede affix:\n{pretty}");
    }

    fn generated_ext(specs: &[(&str, HttpMethod, &str)]) -> String {
        generate_code(ext_rules(specs)).to_string()
    }

    // Whitespace-stripped generated source, for asserting exact segment indices,
    // count guards, and binding slices (which pin the trie's depth/offset
    // arithmetic and the flattening of each segment kind).
    fn flat(specs: &[(&str, HttpMethod, &str)]) -> String {
        generated(specs).replace(' ', "")
    }

    #[test]
    fn literal_and_span_variable_bind_at_the_right_depth() {
        // `/v1/shelves/{shelf}` — two literals then a bounded variable at index 2.
        let code = flat(&[("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")]);
        assert!(code.contains("\"v1\"=>"), "literal `v1` arm: {code}");
        assert!(code.contains("\"shelves\"=>"), "literal `shelves` arm: {code}");
        // The leaf fires only at exactly three segments (depth arithmetic).
        assert!(code.contains("__count==3"), "leaf gated on count 3: {code}");
        // The bound variable is a span slice at index 2, not the open-ended rest form.
        assert!(code.contains("__starts[2],__ends[2]"), "span slice at index 2: {code}");
        assert!(
            !code.contains("__starts[2],__body.len()"),
            "must not use rest form for a bounded var: {code}"
        );
    }

    #[test]
    fn single_wildcard_is_guarded_on_a_non_empty_segment() {
        // `/v1/*` — a top-level single wildcard at depth 1.
        let code = flat(&[("Any", HttpMethod::Get, "/v1/*")]);
        assert!(
            code.contains("__ends[1]>__starts[1]"),
            "single wildcard non-empty guard at depth 1: {code}"
        );
        assert!(code.contains("__count==2"), "leaf gated on count 2: {code}");
    }

    #[test]
    fn rest_capture_uses_open_ended_slice_from_its_depth() {
        // `/v1/tree/{path=**}` — the `**` capture starts at index 2.
        let code = flat(&[("GetTree", HttpMethod::Get, "/v1/tree/{path=**}")]);
        assert!(code.contains("__count>=2"), "rest catch-all guarded by `>= depth`: {code}");
        assert!(
            code.contains("__starts[2],__body.len()"),
            "rest capture is an open-ended slice from index 2: {code}"
        );
    }

    #[test]
    fn affix_guard_pins_the_prefix_and_suffix_lengths() {
        // `/img-{id}.png` — prefix `img-` (4) + suffix `.png` (4) = 8.
        let code = generated_ext(&[("GetImg", HttpMethod::Get, "/img-{id}.png")]).replace(' ', "");
        assert!(code.contains("__seg.len()>8"), "affix length guard is prefix+suffix = 8: {code}");
        assert!(code.contains("starts_with(b\"img-\")"), "affix prefix guard: {code}");
        assert!(code.contains("ends_with(b\".png\")"), "affix suffix guard: {code}");
        // The capture is the segment middle: start + prefix_len .. end - suffix_len.
        assert!(
            code.contains("__starts[0]+4,__ends[0]-4"),
            "affix binding slices off prefix/suffix: {code}"
        );
    }

    #[test]
    fn distinct_methods_at_the_same_node_each_get_an_arm() {
        // Same path, two methods — the method grouping must emit both arms.
        let code = flat(&[("ListX", HttpMethod::Get, "/v1/x"), ("CreateX", HttpMethod::Post, "/v1/x")]);
        assert!(code.contains("\"GET\"=>"), "GET arm present: {code}");
        assert!(code.contains("\"POST\"=>"), "POST arm present: {code}");
        // Two different methods do not conflict.
        assert!(!code.contains("compile_error"), "distinct methods must not conflict: {code}");
    }

    #[test]
    fn distinct_verbs_on_one_method_each_get_an_arm() {
        // Same method + path, two custom verbs — both verb arms must be emitted.
        let code = flat(&[
            ("Inspect", HttpMethod::Get, "/v1/shelves/{shelf}:inspect"),
            ("Watch", HttpMethod::Get, "/v1/shelves/{shelf}:watch"),
        ]);
        assert!(code.contains("Some(\"inspect\")"), "inspect verb arm: {code}");
        assert!(code.contains("Some(\"watch\")"), "watch verb arm: {code}");
        assert!(!code.contains("compile_error"), "distinct verbs must not conflict: {code}");
    }

    #[test]
    fn a_single_route_per_bucket_is_not_reported_as_a_conflict() {
        // One route per (method, verb) at a node must not emit a conflict.
        let code = generated(&[("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")]);
        assert!(!code.contains("compile_error"), "a lone route must never conflict: {code}");
    }

    #[test]
    fn a_segment_reading_node_binds_the_segment_slice() {
        // A node with literal (or affix) edges must read `__seg` before matching.
        let code = flat(&[("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")]);
        assert!(
            code.contains("let__seg:&[u8]=::routerama::codegen_helpers::seg_bytes(__body,__starts[0],__ends[0])"),
            "root binds __seg: {code}"
        );
    }

    #[test]
    fn a_segment_after_an_affix_is_matched_one_level_deeper() {
        // `/a-{id}.z/tail` — the affix occupies index 0, so `tail` sits at index 1
        // and the leaf fires at exactly two segments.
        let code = generated_ext(&[("Tail", HttpMethod::Get, "/a-{id}.z/tail")]).replace(' ', "");
        assert!(code.contains("\"tail\"=>"), "affix child literal is emitted: {code}");
        assert!(
            code.contains("__count==2"),
            "leaf one level below the affix fires at count 2: {code}"
        );
    }

    #[test]
    fn top_level_double_star_captures_the_remainder() {
        // A bare top-level `**` (not wrapped in a `{var=**}`) still flattens to a
        // rest atom and emits the open-ended catch-all.
        let code = flat(&[("All", HttpMethod::Get, "/v1/**")]);
        assert!(code.contains("__count>=1"), "top-level `**` is a catch-all from index 1: {code}");
    }

    #[test]
    fn variable_subtemplate_literal_is_flattened_into_a_match() {
        // `{name=shelves/*}` expands to a literal `shelves` then a `*`, so the
        // literal from the sub-template must appear as its own segment match.
        let code = flat(&[("Search", HttpMethod::Get, "/v1/{name=shelves/*}")]);
        assert!(
            code.contains("\"shelves\"=>"),
            "sub-template literal is flattened into a match arm: {code}"
        );
    }

    #[test]
    fn multiple_affixes_at_one_node_are_ordered_and_prefix_only_needs_no_suffix_guard() {
        // Two affix edges at the root exercise the specificity sort, and a
        // prefix-only affix (empty suffix) exercises the no-suffix-guard path.
        let code = generated_ext(&[("Ver", HttpMethod::Get, "/v{version}"), ("Rev", HttpMethod::Get, "/rev-{id}")]).replace(' ', "");
        // Prefix-only affixes emit a prefix guard but no `ends_with` suffix guard.
        assert!(code.contains("starts_with(b\"v\")"), "prefix guard for `v{{version}}`: {code}");
        assert!(code.contains("starts_with(b\"rev-\")"), "prefix guard for `rev-{{id}}`: {code}");
        assert!(!code.contains("ends_with"), "prefix-only affixes need no suffix guard: {code}");
        // The longer prefix (`rev-`, specificity 4) is guarded before the shorter
        // (`v`, specificity 1).
        let rev = code.find("starts_with(b\"rev-\")").expect("rev- guard present");
        let ver = code.find("starts_with(b\"v\")").expect("v guard present");
        assert!(rev < ver, "more specific affix is tried first: {code}");
    }
}
