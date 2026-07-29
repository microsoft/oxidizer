// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static resolver code generation.
//!
//! [`Generator`](crate::Generator) lowers [`Route`]s into a route enum and a
//! nested segment match. Captures borrow directly from the request path.
//!
//! At each trie node the branches are tried most-specific-first — literal
//! segments before intra-segment affix parameters before a single-segment
//! wildcard before a `**` catch-all, and a deeper (longer) match before a
//! shorter one — so overlapping templates resolve deterministically.
//!
//! [`routerama::codegen_helpers::scan_segments`]: https://docs.rs/routerama

use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use http_path_template::PathTemplate;
use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::quote;

use crate::route::Route;
use crate::trie::{
    Leaf, Node, VarPlan, affix_edges_in_match_order, build_trie_with_templates, capture_field_names, conflicts, field_name,
    is_valid_variant,
};

/// Generates a route enum and its resolver implementation.
pub(crate) fn generate(routes: &[Route], route_type: &str, public: bool, full_api: bool, runtime: &TokenStream) -> TokenStream {
    let visibility = if public {
        quote! { pub }
    } else {
        quote! {}
    };
    let (route_enum, route_type_error) = if is_valid_variant(route_type) {
        (Ident::new(route_type, Span::call_site()), None)
    } else {
        let message = format!("generated route type name `{route_type}` is not a valid Rust identifier");
        (
            Ident::new("__InvalidRouteramaRouteType", Span::call_site()),
            Some(quote! { ::core::compile_error!(#message); }),
        )
    };

    let templates: Vec<PathTemplate<'_>> = routes.iter().map(Route::template).collect();
    let trie = build_trie_with_templates(routes, &templates);
    let root = trie.root;
    let any_verb = trie.any_verb;
    let cap = trie.max_segments;

    let body_bind = if any_verb {
        quote! { let (__body, __verb) = #runtime::split_verb(path); }
    } else {
        quote! { let __body: &str = path; }
    };

    let cap_lit = Literal::usize_unsuffixed(cap);

    let match_tree = emit_node(&root, 0, any_verb, &route_enum);

    let conflicts = conflicts(&root);
    let conflict_errors = conflicts.iter().map(|message| quote! { ::core::compile_error!(#message); });

    let (variants, variant_errors) = collect_variants(routes, &templates);
    let route_generic = variants.iter().any(|variant| !variant.fields.is_empty());
    let checked_resolve_body = quote! {
        let method: &str = ::core::convert::AsRef::<str>::as_ref(&method);
        let path: &'p str = ::core::convert::AsRef::<str>::as_ref(path);
        #(#conflict_errors)*
        #body_bind
        #runtime::with_scanned_path(__body, #cap_lit, |__path| {
            let __count = __path.count();
            #match_tree
            ::core::option::Option::None
        })
    };

    let enum_lt = if route_generic {
        quote! { <'p> }
    } else {
        quote! {}
    };
    let enum_def = emit_enum_def(&visibility, &route_enum, &variants, &enum_lt, full_api);
    let resolver_and_match = emit_resolver_and_match(
        &route_enum,
        &variants,
        runtime,
        &visibility,
        route_generic,
        &checked_resolve_body,
        full_api,
    );

    quote! {
        #route_type_error
        #(#variant_errors)*

        #enum_def

        #resolver_and_match
    }
}

/// Emits the enum's `resolve` function and optional `RouteMatch` implementation.
fn emit_resolver_and_match(
    route_enum: &Ident,
    variants: &[VariantSpec],
    runtime: &TokenStream,
    visibility: &TokenStream,
    route_generic: bool,
    resolve_body: &TokenStream,
    full_api: bool,
) -> TokenStream {
    let route_ty = if route_generic {
        quote! { #route_enum<'p> }
    } else {
        quote! { #route_enum }
    };
    let (impl_generics, fn_generics) = if route_generic {
        (quote! { <'p> }, quote! { <__P> })
    } else {
        (quote! {}, quote! { <'p, __P> })
    };
    let name_arms = variants.iter().map(|variant| {
        let ident = &variant.ident;
        let name = &variant.name;
        if variant.fields.is_empty() {
            quote! { #route_enum::#ident => #name, }
        } else {
            quote! { #route_enum::#ident { .. } => #name, }
        }
    });
    let capture_arms = variants.iter().map(|variant| {
        let ident = &variant.ident;
        if variant.fields.is_empty() {
            quote! { #route_enum::#ident => ::core::option::Option::None, }
        } else {
            // Prefix bindings so captured names cannot shadow generated locals.
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
    let empty_capture_arm = variants.is_empty().then(|| {
        quote! { _ => ::core::option::Option::None, }
    });
    let allow = quote! {
        #[allow(
            clippy::all,
            clippy::pedantic,
            clippy::nursery,
            clippy::restriction,
            dead_code,
            unused,
            missing_docs,
            reason = "code generated by routerama_build"
        )]
    };
    let route_match = full_api.then(|| {
        quote! {
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
                        #empty_capture_arm
                    }
                }
            }
        }
    });
    let resolve_methods = emit_resolve_methods(visibility, &fn_generics, &route_ty, runtime, resolve_body);
    quote! {
        #allow
        impl #impl_generics #route_ty {
            #resolve_methods
        }

        #route_match
    }
}

fn emit_resolve_methods(
    visibility: &TokenStream,
    fn_generics: &TokenStream,
    route_ty: &TokenStream,
    runtime: &TokenStream,
    resolve_body: &TokenStream,
) -> TokenStream {
    quote! {
        /// Resolves an HTTP method + path against this route table.
        ///
        /// Returns the matched route (with any captured path variables) or
        /// [`None`] when nothing matches.
        #[inline]
        #visibility fn resolve #fn_generics (
            method: impl ::core::convert::AsRef<str>,
            path: &'p __P,
        ) -> ::core::option::Option<#route_ty>
        where
            __P: ::core::convert::AsRef<str> + ?::core::marker::Sized,
        {
            Self::__resolve_checked(method, path).ok().flatten()
        }

        #[doc(hidden)]
        #[inline]
        pub(crate) fn __resolve_checked #fn_generics (
            method: impl ::core::convert::AsRef<str>,
            path: &'p __P,
        ) -> ::core::result::Result<::core::option::Option<#route_ty>, #runtime::InvalidPath>
        where
            __P: ::core::convert::AsRef<str> + ?::core::marker::Sized,
        {
            #resolve_body
        }
    }
}

/// Emits the `Route` enum definition (one struct/unit variant per route), with
/// the canonical derive set.
fn emit_enum_def(visibility: &TokenStream, route_enum: &Ident, variants: &[VariantSpec], lt: &TokenStream, full_api: bool) -> TokenStream {
    let variant_defs = variants.iter().map(|variant| {
        let ident = &variant.ident;
        if variant.fields.is_empty() {
            quote! { #ident }
        } else {
            let fields = variant.fields.iter().map(|field| quote! { #field: &'p str });
            quote! { #ident { #(#fields),* } }
        }
    });
    let derives = full_api.then(|| {
        quote! {
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
        }
    });
    quote! {
        /// A resolved route, with one variant per route name.
        ///
        /// A route that captures `{variable}` path segments is a struct variant
        /// carrying them as `&'p str` fields (borrowed from the request path); a
        /// route with no variables is a unit variant.
        #derives
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

/// Metadata for one generated enum variant.
struct VariantSpec {
    ident: Ident,
    name: String,
    fields: Vec<Ident>,
    keys: Vec<String>,
}

/// Collects variants and diagnostics in route declaration order.
fn collect_variants(routes: &[Route], templates: &[PathTemplate<'_>]) -> (Vec<VariantSpec>, Vec<TokenStream>) {
    if use_indexed_variant_collection(routes.len()) {
        collect_variants_impl::<true>(routes, templates)
    } else {
        collect_variants_impl::<false>(routes, templates)
    }
}

#[cfg_attr(test, mutants::skip)]
const fn use_indexed_variant_collection(route_count: usize) -> bool {
    route_count >= 1_024
}

fn collect_variants_impl<const INDEXED: bool>(routes: &[Route], templates: &[PathTemplate<'_>]) -> (Vec<VariantSpec>, Vec<TokenStream>) {
    let mut specs: Vec<VariantSpec> = Vec::new();
    let mut spec_indices = BTreeMap::<&str, usize>::new();
    let mut errors: Vec<TokenStream> = Vec::new();
    for (route, template) in routes.iter().zip(templates) {
        let names = capture_field_names(template.segments());
        let fields: Vec<Ident> = names.iter().map(|name| field_ident(name)).collect();
        let keys: Vec<String> = names.iter().map(|name| name.join(".")).collect();

        let existing_index = if INDEXED {
            spec_indices.get(route.name()).copied()
        } else {
            specs.iter().position(|spec| spec.name == route.name())
        };
        if let Some(index) = existing_index {
            let spec = &specs[index];
            // Compare original keys because distinct keys can sanitize alike.
            let mut existing = spec.keys.clone();
            let mut incoming = keys;
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

        if INDEXED {
            spec_indices.insert(route.name(), specs.len());
        }
        if !is_valid_variant(route.name()) {
            let message = format!(
                "route name `{}` is not a valid Rust identifier; route names must be valid identifiers so they can name a route enum variant",
                route.name()
            );
            errors.push(quote! { ::core::compile_error!(#message); });
        } else if route.name() == "resolve" {
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
    if is_valid_variant(name) && name != "resolve" {
        Ident::new(name, Span::call_site())
    } else {
        let sanitized: String = name
            .chars()
            .map(|c| if c == '_' || c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        Ident::new(&format!("_R_{sanitized}"), Span::call_site())
    }
}

/// Emits the matching code for a trie `node` reached after consuming `depth`
/// segments. On a match the code `return`s; otherwise it falls through.
fn emit_node(node: &Node, depth: usize, any_verb: bool, route_enum: &Ident) -> TokenStream {
    let depth_lit = Literal::usize_unsuffixed(depth);

    let has_literals = !node.literals.is_empty();
    let has_affix = !node.affix.is_empty();
    let needs_seg = has_literals || has_affix;
    let seg_bind = if needs_seg {
        quote! {
            let __seg: &[u8] = __path
                .segment(#depth_lit)
                .expect("guarded by the segment count check")
                .as_bytes();
        }
    } else {
        quote! {}
    };
    let literal_arms = node.literals.iter().map(|(lit, child)| {
        let child = emit_node(child, depth + 1, any_verb, route_enum);
        // Byte matching avoids repeated UTF-8 boundary checks.
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
    let affix_code = affix_edges_in_match_order(node).into_iter().map(|((prefix, suffix), child)| {
        let child = emit_node(child, depth + 1, any_verb, route_enum);
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
    let single_code = emit_single(node.single.as_deref(), depth, any_verb, route_enum);
    let rest_dispatch = emit_leaves(&node.rest, any_verb, route_enum);

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

    let exact_dispatch = emit_leaves(&node.exact, any_verb, route_enum);
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

fn emit_single(child: Option<&Node>, depth: usize, any_verb: bool, route_enum: &Ident) -> TokenStream {
    let Some(child) = child else {
        return TokenStream::new();
    };
    let depth_lit = Literal::usize_unsuffixed(depth);
    let child = emit_node(child, depth + 1, any_verb, route_enum);
    quote! {
        if !__path
            .segment(#depth_lit)
            .expect("guarded by the segment count check")
            .is_empty()
        {
            #child
        }
    }
}

/// Emits `method` (and, when any route uses one, `:verb`) dispatch for a set of
/// leaves terminating at the same trie node, returning the matched `Route` on a hit.
fn emit_leaves(leaves: &[Leaf], any_verb: bool, route_enum: &Ident) -> TokenStream {
    if leaves.is_empty() {
        return quote! {};
    }

    // Preserve declaration order while deduplicating method/verb pairs.
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
                let ret = emit_return(leaf, route_enum);
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
            emit_return(entries.first().expect("each group has at least one leaf"), route_enum)
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
fn emit_return(leaf: &Leaf, route_enum: &Ident) -> TokenStream {
    let variant = variant_ident(&leaf.name);
    let route_val = if leaf.vars.is_empty() {
        quote! { #route_enum::#variant }
    } else {
        let field_inits = leaf.vars.iter().map(|plan| {
            let field = Ident::new(plan.field(), Span::call_site());
            let value = var_slice_expr(plan);
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
fn var_slice_expr(plan: &VarPlan) -> TokenStream {
    match plan {
        VarPlan::Span { a, b, .. } => {
            let a = Literal::usize_unsuffixed(*a);
            let b = Literal::usize_unsuffixed(*b);
            quote! {
                __path
                    .capture(#a, #b)
                    .expect("route capture references scanned segment indices")
            }
        }
        VarPlan::Rest { a, .. } => {
            let a = Literal::usize_unsuffixed(*a);
            quote! {
                __path
                    .rest(#a)
                    .expect("route rest starts at or before the segment count")
            }
        }
        VarPlan::Affix {
            a, prefix_len, suffix_len, ..
        } => {
            let a = Literal::usize_unsuffixed(*a);
            let prefix_lit = Literal::usize_unsuffixed(*prefix_len);
            let suffix_lit = Literal::usize_unsuffixed(*suffix_len);
            quote! {
                __path
                    .affix(#a, #prefix_lit, #suffix_lit)
                    .expect("matched affix literals delimit a valid capture")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use core::fmt::Write as _;

    use http_path_template::Grammar;

    use super::*;
    use crate::generator::Generator;

    fn rules(specs: &[(&str, &str, &str)]) -> Vec<Route> {
        specs
            .iter()
            .map(|(name, method, pattern)| {
                Route::new(
                    *name,
                    *method,
                    PathTemplate::parse(pattern, Grammar::default()).expect("valid template"),
                )
            })
            .collect()
    }

    fn generated(specs: &[(&str, &str, &str)]) -> String {
        generate_code(rules(specs)).to_string()
    }

    #[test]
    fn large_route_sets_use_indexed_variant_collection() {
        assert!(!use_indexed_variant_collection(1_023));
        assert!(use_indexed_variant_collection(1_024));

        let routes: Vec<Route> = (0..1_024)
            .map(|index| {
                Route::new(
                    format!("Route{index}"),
                    "GET",
                    PathTemplate::parse("/shared", Grammar::default()).expect("valid template"),
                )
            })
            .collect();
        let templates: Vec<PathTemplate<'_>> = routes.iter().map(Route::template).collect();

        let (variants, errors) = collect_variants(&routes, &templates);

        assert_eq!(variants.len(), routes.len());
        assert!(errors.is_empty());
    }

    /// Test helper: generate a `pub` `Route` enum.
    fn generate_code(routes: impl IntoIterator<Item = Route>) -> TokenStream {
        generate_code_named(routes, "Route")
    }

    /// Test helper: generate a `pub` enum with a custom name.
    fn generate_code_named(routes: impl IntoIterator<Item = Route>, route_type: &str) -> TokenStream {
        let mut generator = Generator::new(route_type, true);
        generator.add_all(routes);
        generator.generate()
    }

    fn ext_rules(specs: &[(&str, &str, &str)]) -> Vec<Route> {
        specs
            .iter()
            .map(|(name, method, pattern)| {
                Route::new(
                    *name,
                    *method,
                    PathTemplate::parse(pattern, Grammar::default().with_segment_affixes()).expect("valid template"),
                )
            })
            .collect()
    }

    #[test]
    fn generates_a_resolve_function() {
        let code = generated(&[("GetShelf", "GET", "/v1/shelves/{shelf}")]);
        assert!(code.contains("fn resolve"));
        assert!(code.contains("GetShelf"));
        assert!(code.contains("with_scanned_path"));
    }

    #[test]
    fn generated_scanner_does_not_size_stack_arrays_from_route_depth() {
        let mut path = String::new();
        for index in 0..128 {
            path.push('/');
            let _ = write!(path, "segment{index}");
        }
        let code = generate_code(rules(&[("Deep", "GET", &path)])).to_string().replace(' ', "");

        assert!(code.contains("with_scanned_path(__body,128"));
        assert!(!code.contains("[0usize;128]"));
    }

    #[test]
    fn resolve_is_emitted_on_the_enum() {
        // For a capturing route the enum is `Route<'p>` and its inherent `resolve`
        // reuses `'p`.
        let file: syn::File = syn::parse2(generate_code(rules(&[("GetShelf", "GET", "/v1/shelves/{shelf}")]))).expect("valid Rust");
        let code = prettyplease::unparse(&file).replace(' ', "");
        assert!(code.contains("impl<'p>Route<'p>{"), "resolve impl is on the enum: {code}");
        assert!(code.contains("fnresolve<__P>"), "capturing enum reuses the impl's `'p`: {code}");
        assert!(code.contains("RouteMatch<'p>forRoute"), "RouteMatch impl: {code}");
        assert!(
            code.contains("\"shelf\"=>") && code.contains("Some(*__cap_shelf)"),
            "capture arm: {code}"
        );
    }

    #[test]
    fn generates_a_route_enum_with_a_variant_per_name() {
        // The `Route` enum drives `O(1)` jump-table dispatch: one variant per
        // distinct name, deduplicated (a name bound to two routes with the same
        // captures yields one variant), plus a `name` accessor recovering the
        // declared string. A capturing route becomes a struct variant.
        let file: syn::File = syn::parse2(generate_code(rules(&[
            ("ListShelves", "GET", "/v1/shelves"),
            ("GetShelf", "GET", "/v1/shelves/{shelf}"),
            ("GetShelf", "DELETE", "/v1/shelves/{shelf}"),
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
    fn resolve_is_on_the_configured_enum_name() {
        // With a configured `route_type`, the enum takes that name and `resolve`
        // is an associated fn on it, called as `BookRoute::resolve(..)`.
        let file: syn::File =
            syn::parse2(generate_code_named(rules(&[("GetBook", "GET", "/books/{book}")]), "BookRoute")).expect("valid Rust");
        let flat = prettyplease::unparse(&file).replace(' ', "");
        assert!(flat.contains("enumBookRoute<'p>"), "{flat}");
        assert!(flat.contains("impl<'p>BookRoute<'p>{"), "resolve impl is on the enum: {flat}");
        assert!(flat.contains("fnresolve<__P>"), "{flat}");
        assert!(flat.contains("::core::option::Option<BookRoute<'p>>"), "{flat}");
    }

    #[test]
    fn non_capturing_enum_resolve_introduces_its_own_lifetime() {
        // With no captures the enum is non-generic, so `resolve` introduces its
        // own `'p` for the path and returns the bare `Route`.
        let file: syn::File = syn::parse2(generate_code(rules(&[("Health", "GET", "/health")]))).expect("valid Rust");
        let flat = prettyplease::unparse(&file).replace(' ', "");
        // A non-generic enum: `impl Route { fn resolve<'p, __P>... }`.
        assert!(flat.contains("implRoute{"), "resolve impl on the bare enum: {flat}");
        assert!(flat.contains("fnresolve<'p,__P>"), "{flat}");
        assert!(flat.contains("::core::option::Option<Route>"), "{flat}");
    }

    #[test]
    fn generated_enum_implements_route_match() {
        // The enum implements `RouteMatch`; resolving is its inherent `resolve`.
        // Capturing: the enum is `Route<'p>`.
        let capturing = prettyplease::unparse(&syn::parse2(generate_code(rules(&[("GetBook", "GET", "/books/{book}")]))).expect("valid"))
            .replace(' ', "");
        assert!(
            capturing.contains("impl<'p>::routerama::codegen_helpers::RouteMatch<'p>forRoute<'p>{"),
            "{capturing}"
        );
        assert!(capturing.contains("impl<'p>Route<'p>{"), "resolve impl on the enum: {capturing}");

        // Capture-less: the enum is non-generic.
        let unit =
            prettyplease::unparse(&syn::parse2(generate_code(rules(&[("Health", "GET", "/health")]))).expect("valid")).replace(' ', "");
        assert!(
            unit.contains("impl<'p>::routerama::codegen_helpers::RouteMatch<'p>forRoute{"),
            "{unit}"
        );
    }

    #[test]
    fn capturing_routes_become_struct_variants_and_non_capturing_stay_unit() {
        // A route with `{variable}` segments carries them as named `&'p str`
        // fields; a route with none is a unit variant. The enum is
        // lifetime-parameterized because at least one variant carries a field.
        let file: syn::File = syn::parse2(generate_code(rules(&[
            ("ListShelves", "GET", "/v1/shelves"),
            ("GetReview", "GET", "/v1/shelves/{shelf}/reviews/{review}"),
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
        let file: syn::File = syn::parse2(generate_code(rules(&[("GetShelf", "GET", "/v1/shelves/{shelf.id}")]))).expect("valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("shelf_id: &'p str"), "{pretty}");
    }

    #[test]
    fn a_router_with_no_captures_has_a_non_generic_route_enum() {
        // With no route capturing a variable, `Route` needs no lifetime.
        let file: syn::File = syn::parse2(generate_code(rules(&[("ListShelves", "GET", "/v1/shelves")]))).expect("valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("enum Route {"), "{pretty}");
        assert!(!pretty.contains("enum Route<"), "{pretty}");
    }

    #[test]
    fn reusing_a_name_with_different_captures_is_a_compile_error() {
        let code = generated(&[
            ("GetShelf", "GET", "/v1/shelves/{shelf}"),
            ("GetShelf", "GET", "/v2/shelves/{shelf}/books/{book}"),
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
            ("UserRepo", "GET", "/users/{user}/repos/{repo}"),
            ("UserRepo", "GET", "/repos/{repo}/users/{user}"),
        ]);
        assert!(!code.contains("compile_error"), "{code}");
        assert!(!code.contains("capture different path variables"), "{code}");
    }

    #[test]
    fn reusing_a_name_with_distinct_keys_that_sanitize_identically_is_rejected() {
        let code = generated(&[
            ("GetShelf", "GET", "/v1/shelves/{shelf.id}"),
            ("GetShelf", "GET", "/v2/shelves/{shelf_id}"),
        ]);
        assert!(code.contains("compile_error"), "{code}");
        assert!(code.contains("capture different path variables"), "{code}");
        assert!(code.contains("shelf.id"), "{code}");
        assert!(code.contains("shelf_id"), "{code}");
    }

    #[test]
    fn reusing_a_name_with_a_fieldless_binding_renders_none_in_the_diagnostic() {
        // One binding captures nothing and the other captures `{book}`, so the
        // conflict diagnostic formats the empty field set as `none`.
        let code = generated(&[("GetShelf", "GET", "/v1/shelves"), ("GetShelf", "GET", "/v1/shelves/{book}")]);
        assert!(code.contains("compile_error"), "{code}");
        assert!(code.contains("none vs"), "{code}");
    }

    #[test]
    fn a_route_capturing_the_same_field_twice_is_a_compile_error() {
        // A single route whose template binds two variables to the same field
        // cannot map them into one struct field, so it is rejected.
        let code = generated(&[("GetPair", "GET", "/{shelf}/x/{shelf}")]);
        assert!(code.contains("compile_error"), "{code}");
        assert!(code.contains("same field"), "{code}");
    }

    #[test]
    fn only_an_actual_inherent_method_collision_is_rejected() {
        let name = generated(&[("name", "GET", "/name")]);
        assert!(!name.contains("compile_error"), "{name}");
        syn::parse_str::<syn::File>(&name).expect("a variant may share a name with a trait method");

        let resolve = generated(&[("resolve", "GET", "/resolve")]);
        assert!(resolve.contains("compile_error"), "{resolve}");
        assert!(resolve.contains("collides with a generated method"), "{resolve}");
    }

    #[test]
    fn capturing_a_field_named_like_the_key_parameter_compiles() {
        // A captured field named `__key` must not shadow the `capture` method's
        // key parameter: the generated variant fields are bound under a `__cap_`
        // prefix, so the emitted `RouteMatch` impl is valid Rust.
        let file: syn::File = syn::parse2(generate_code(rules(&[("GetKey", "GET", "/things/{__key}")])))
            .expect("a `{__key}` capture must still emit valid Rust");
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
        let file: syn::File = syn::parse2(generate_code(rules(&[("GetBook", "GET", "/books/{book}")]))).expect("valid Rust");
        let flat: String = prettyplease::unparse(&file).split_whitespace().collect();
        assert!(flat.contains("Route::GetBook{book:__path.capture(1,1).expect("), "{flat}");
    }

    #[test]
    fn route_enum_derives_the_canonical_traits() {
        // The enum is `Copy` and derives the full canonical comparison/hashing
        // set, so callers can freely compare, order, hash, and store it (e.g. in
        // a `BTreeMap`/`HashMap` keyed by route).
        let file: syn::File = syn::parse2(generate_code(rules(&[("GetShelf", "GET", "/v1/shelves/{shelf}")]))).expect("valid Rust");
        let pretty = prettyplease::unparse(&file);
        for trait_name in ["Clone", "Copy", "Debug", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash"] {
            assert!(pretty.contains(trait_name), "derive is missing `{trait_name}`: {pretty}");
        }
    }

    #[test]
    fn route_enum_does_not_implement_display_or_as_ref_str() {
        // Presentation/conversion traits are the caller's choice; the macro only
        // exposes the route name via `RouteMatch::name`.
        let file: syn::File = syn::parse2(generate_code(rules(&[("GetShelf", "GET", "/v1/shelves/{shelf}")]))).expect("valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(!pretty.contains("Display for Route"), "{pretty}");
        assert!(!pretty.contains("AsRef<str> for Route"), "{pretty}");
    }

    #[test]
    fn a_name_that_is_not_an_identifier_is_a_compile_error() {
        let code = generated(&[("0:GetShelf", "GET", "/v1/shelves/{shelf}")]);
        assert!(code.contains("compile_error"), "{code}");
        assert!(code.contains("not a valid Rust identifier"), "{code}");
    }

    #[test]
    fn an_invalid_generated_type_name_is_a_compile_error_instead_of_a_panic() {
        let code = generate_code_named(rules(&[("GetShelf", "GET", "/shelves")]), "type").to_string();
        assert!(code.contains("compile_error"), "{code}");
        assert!(code.contains("generated route type name"), "{code}");
    }

    #[test]
    fn is_valid_variant_accepts_only_non_keyword_identifiers() {
        // Valid identifiers (used verbatim as enum variants).
        assert!(is_valid_variant("GetShelf"));
        assert!(is_valid_variant("_private"));
        assert!(is_valid_variant("Route2"));
        assert!(is_valid_variant("Διαδρομή"));
        assert!(is_valid_variant("路由"));

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
            ("ListShelves", "GET", "/v1/shelves"),
            ("CreateShelf", "POST", "/v1/shelves"),
            ("GetShelf", "GET", "/v1/shelves/{shelf}"),
            ("ArchiveShelf", "POST", "/v1/shelves/{shelf}:archive"),
            ("GetTree", "GET", "/v1/tree/{path=**}"),
        ]));
        let file: syn::File = syn::parse2(tokens).expect("generated router is valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("fn resolve"));
        assert!(pretty.contains("routerama"));
        assert!(
            !pretty.contains("unsafe"),
            "generated resolvers must contain only safe Rust: {pretty}"
        );
    }

    #[test]
    fn empty_router_is_valid_and_matches_nothing() {
        let file: syn::File = syn::parse2(generate_code(core::iter::empty())).expect("valid");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("fn resolve"));
        assert!(
            pretty.contains("_ => ::core::option::Option::None"),
            "capture on a reference to an empty enum needs a wildcard arm: {pretty}"
        );
    }

    #[test]
    fn conflicting_routes_emit_a_compile_error() {
        // Two GET routes with the same path shape map to different names; only the
        // first could ever match.
        let code = generated(&[("GetBookA", "GET", "/v1/books/{book}"), ("GetBookB", "GET", "/v1/books/{other}")]);
        assert!(code.contains("compile_error"), "{code}");
        assert!(code.contains("conflicting routes"), "{code}");
    }

    #[test]
    fn no_verb_split_when_no_route_uses_a_verb() {
        // Without any custom verb, the generated code binds `__body = path`
        // directly rather than calling `split_verb`.
        let code = generated(&[("GetShelf", "GET", "/v1/shelves/{shelf}")]);
        assert!(!code.contains("split_verb"), "{code}");

        let with_verb = generated(&[("ArchiveShelf", "POST", "/v1/shelves/{shelf}:archive")]);
        assert!(with_verb.contains("split_verb"), "{with_verb}");
    }

    #[test]
    fn rest_capture_emitted_once() {
        // The `**` capture is sliced once for the matched variant's field; the
        // underlying trie produces exactly one capture site.
        let code = generate_code(rules(&[("GetTree", "GET", "/v1/{path=**}")]))
            .to_string()
            .replace(' ', "");
        assert_eq!(
            code.matches("__path.rest(1)").count(),
            1,
            "rest capture emitted once for the variant field: {code}"
        );
    }

    #[test]
    fn affix_route_generates_valid_rust_with_guards() {
        let tokens = generate_code(ext_rules(&[("GetFile", "GET", "/files/{name}.json")]));
        let file: syn::File = syn::parse2(tokens).expect("generated router is valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("ends_with"), "{pretty}");
        assert!(pretty.contains("GetFile"), "{pretty}");
    }

    #[test]
    fn affix_prefix_and_suffix_emit_both_guards() {
        let code = generated_ext(&[("GetImg", "GET", "/img-{id}.png")]);
        assert!(code.contains("starts_with"), "{code}");
        assert!(code.contains("ends_with"), "{code}");
    }

    #[test]
    fn affix_binding_slices_the_segment_middle() {
        // The captured value is the segment minus its literal prefix/suffix.
        let code = generated_ext(&[("GetFile", "GET", "/files/{name}.json")]).replace(' ', "");
        assert!(code.contains("__path.affix(1,0,5)"), "{code}");
    }

    #[test]
    fn literal_takes_priority_over_affix_at_same_depth() {
        let tokens = generate_code(ext_rules(&[
            ("GetLatest", "GET", "/files/latest.json"),
            ("GetFile", "GET", "/files/{name}.json"),
        ]));
        // Both coexist; the literal edge is matched before the affix guard.
        let file: syn::File = syn::parse2(tokens).expect("valid Rust");
        let pretty = prettyplease::unparse(&file);
        let literal_pos = pretty.find("latest.json").expect("literal arm emitted");
        let affix_pos = pretty.find("ends_with").expect("affix guard emitted");
        assert!(literal_pos < affix_pos, "literal must precede affix:\n{pretty}");
    }

    fn generated_ext(specs: &[(&str, &str, &str)]) -> String {
        generate_code(ext_rules(specs)).to_string()
    }

    // Whitespace-stripped generated source, for asserting exact segment indices,
    // count guards, and binding slices (which pin the trie's depth/offset
    // arithmetic and the flattening of each segment kind).
    fn flat(specs: &[(&str, &str, &str)]) -> String {
        generated(specs).replace(' ', "")
    }

    #[test]
    fn literal_and_span_variable_bind_at_the_right_depth() {
        // `/v1/shelves/{shelf}` — two literals then a bounded variable at index 2.
        let code = flat(&[("GetShelf", "GET", "/v1/shelves/{shelf}")]);
        assert!(code.contains("\"v1\"=>"), "literal `v1` arm: {code}");
        assert!(code.contains("\"shelves\"=>"), "literal `shelves` arm: {code}");
        // The leaf fires only at exactly three segments (depth arithmetic).
        assert!(code.contains("__count==3"), "leaf gated on count 3: {code}");
        // The bound variable is a span slice at index 2, not the open-ended rest form.
        assert!(code.contains("__path.capture(2,2)"), "span slice at index 2: {code}");
        assert!(!code.contains("__path.rest(2)"), "must not use rest form for a bounded var: {code}");
    }

    #[test]
    fn single_wildcard_is_guarded_on_a_non_empty_segment() {
        // `/v1/*` — a top-level single wildcard at depth 1.
        let code = flat(&[("Any", "GET", "/v1/*")]);
        assert!(
            code.contains("__path.segment(1).expect(") && code.contains(".is_empty()"),
            "single wildcard non-empty guard at depth 1: {code}"
        );
        assert!(code.contains("__count==2"), "leaf gated on count 2: {code}");
    }

    #[test]
    fn rest_capture_uses_open_ended_slice_from_its_depth() {
        // `/v1/tree/{path=**}` — the `**` capture starts at index 2.
        let code = flat(&[("GetTree", "GET", "/v1/tree/{path=**}")]);
        assert!(code.contains("__count>=2"), "rest catch-all guarded by `>= depth`: {code}");
        assert!(
            code.contains("__path.rest(2)"),
            "rest capture is an open-ended slice from index 2: {code}"
        );
    }

    #[test]
    fn affix_guard_pins_the_prefix_and_suffix_lengths() {
        // `/img-{id}.png` — prefix `img-` (4) + suffix `.png` (4) = 8.
        let code = generated_ext(&[("GetImg", "GET", "/img-{id}.png")]).replace(' ', "");
        assert!(code.contains("__seg.len()>8"), "affix length guard is prefix+suffix = 8: {code}");
        assert!(code.contains("starts_with(b\"img-\")"), "affix prefix guard: {code}");
        assert!(code.contains("ends_with(b\".png\")"), "affix suffix guard: {code}");
        // The capture is the segment middle: start + prefix_len .. end - suffix_len.
        assert!(
            code.contains("__path.affix(0,4,4)"),
            "affix binding slices off prefix/suffix: {code}"
        );
    }

    #[test]
    fn distinct_methods_at_the_same_node_each_get_an_arm() {
        // Same path, two methods — the method grouping must emit both arms.
        let code = flat(&[("ListX", "GET", "/v1/x"), ("CreateX", "POST", "/v1/x")]);
        assert!(code.contains("\"GET\"=>"), "GET arm present: {code}");
        assert!(code.contains("\"POST\"=>"), "POST arm present: {code}");
        // Two different methods do not conflict.
        assert!(!code.contains("compile_error"), "distinct methods must not conflict: {code}");
    }

    #[test]
    fn distinct_verbs_on_one_method_each_get_an_arm() {
        // Same method + path, two custom verbs — both verb arms must be emitted.
        let code = flat(&[
            ("Inspect", "GET", "/v1/shelves/{shelf}:inspect"),
            ("Watch", "GET", "/v1/shelves/{shelf}:watch"),
        ]);
        assert!(code.contains("Some(\"inspect\")"), "inspect verb arm: {code}");
        assert!(code.contains("Some(\"watch\")"), "watch verb arm: {code}");
        assert!(!code.contains("compile_error"), "distinct verbs must not conflict: {code}");
    }

    #[test]
    fn a_single_route_per_bucket_is_not_reported_as_a_conflict() {
        // One route per (method, verb) at a node must not emit a conflict.
        let code = generated(&[("GetShelf", "GET", "/v1/shelves/{shelf}")]);
        assert!(!code.contains("compile_error"), "a lone route must never conflict: {code}");
    }

    #[test]
    fn a_segment_reading_node_binds_the_segment_slice() {
        // A node with literal (or affix) edges must read `__seg` before matching.
        let code = flat(&[("GetShelf", "GET", "/v1/shelves/{shelf}")]);
        assert!(
            code.contains("let__seg:&[u8]=__path.segment(0).expect("),
            "root binds __seg: {code}"
        );
    }

    #[test]
    fn a_segment_after_an_affix_is_matched_one_level_deeper() {
        // `/a-{id}.z/tail` — the affix occupies index 0, so `tail` sits at index 1
        // and the leaf fires at exactly two segments.
        let code = generated_ext(&[("Tail", "GET", "/a-{id}.z/tail")]).replace(' ', "");
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
        let code = flat(&[("All", "GET", "/v1/**")]);
        assert!(code.contains("__count>=1"), "top-level `**` is a catch-all from index 1: {code}");
    }

    #[test]
    fn variable_subtemplate_literal_is_flattened_into_a_match() {
        // `{name=shelves/*}` expands to a literal `shelves` then a `*`, so the
        // literal from the sub-template must appear as its own segment match.
        let code = flat(&[("Search", "GET", "/v1/{name=shelves/*}")]);
        assert!(
            code.contains("\"shelves\"=>"),
            "sub-template literal is flattened into a match arm: {code}"
        );
    }

    #[test]
    fn multiple_affixes_at_one_node_are_ordered_and_prefix_only_needs_no_suffix_guard() {
        // Two affix edges at the root exercise the specificity sort, and a
        // prefix-only affix (empty suffix) exercises the no-suffix-guard path.
        let code = generated_ext(&[("Ver", "GET", "/v{version}"), ("Rev", "GET", "/rev-{id}")]).replace(' ', "");
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
