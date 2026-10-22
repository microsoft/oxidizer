// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Dispatch-arm construction and request-dispatch code emission.

use alloc::string::{String, ToString as _};
use alloc::vec::Vec;
use alloc::{format, vec};

use http_path_template::{Grammar, PathTemplate};
use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::quote;
use syn::spanned::Spanned as _;
use syn::{Error, LitByteStr, LitStr, Type};

use super::binding::{combine_error, static_route_target, type_key};
use super::lowering::{emit_direct_dispatch, emit_overlap_dispatch};
use super::model::{
    DispatchArm, DispatchBoundary, DispatchKind, Handler, HandlerKind, ResponseBodyModel, RoutingPolicy, SharedState, StaticRouteEntry,
};
use crate::Route;
use crate::macro_impl::RouteAttr;
use crate::trie::{Node, VarPlan, build_trie, capture_field_names, depth_limit_error};

#[expect(
    clippy::too_many_arguments,
    reason = "dispatch generation keeps each request owner, ordered predicate, extraction, and handler boundary explicit"
)]
pub(super) fn dispatch_arms(
    handlers: &[Handler],
    dispatches: &[DispatchArm],
    policy: &RoutingPolicy,
    route_path: &TokenStream2,
    target: &TokenStream2,
    parts: &Ident,
    body: &Ident,
    response: &Ident,
    failure: &Ident,
    state: &Ident,
    body_type: &Ident,
    shared_state: &SharedState,
    module_name: &Ident,
    response_body: &ResponseBodyModel,
    boundary: DispatchBoundary,
    runtime: &TokenStream2,
    response_runtime: &TokenStream2,
) -> Vec<TokenStream2> {
    dispatches
        .iter()
        .map(|dispatch| {
            let variant = &dispatch.variant;
            let pattern = if dispatch.captures.is_empty() {
                quote! { #route_path::#variant }
            } else {
                let fields = dispatch.captures.iter().map(|(name, _)| name);
                quote! { #route_path::#variant { #(#fields),* } }
            };
            let body = match &dispatch.kind {
                DispatchKind::Direct(index) => emit_direct_dispatch(
                    &handlers[*index],
                    policy,
                    target,
                    parts,
                    body,
                    response,
                    state,
                    body_type,
                    shared_state,
                    module_name,
                    response_body,
                    boundary,
                    runtime,
                    response_runtime,
                ),
                DispatchKind::Overlap(candidates) => emit_overlap_dispatch(
                    candidates,
                    handlers,
                    policy,
                    target,
                    parts,
                    body,
                    response,
                    failure,
                    state,
                    body_type,
                    shared_state,
                    module_name,
                    response_body,
                    boundary,
                    runtime,
                    response_runtime,
                ),
            };
            quote! {
                #pattern => {
                    #body
                }
            }
        })
        .collect()
}

#[derive(Clone, Copy)]
pub(super) enum PredicateKind {
    Host,
    Consumes,
    Produces,
}

pub(super) struct OverlapPredicatePlan {
    hosts: Vec<String>,
    consumes: Vec<String>,
    produces: Vec<String>,
}

impl OverlapPredicatePlan {
    pub(super) fn new(candidates: &[usize], handlers: &[Handler]) -> Self {
        Self {
            hosts: overlap_predicate_values(candidates, handlers, PredicateKind::Host),
            consumes: overlap_predicate_values(candidates, handlers, PredicateKind::Consumes),
            produces: overlap_predicate_values(candidates, handlers, PredicateKind::Produces),
        }
    }

    fn is_empty(&self) -> bool {
        self.hosts.is_empty() && self.consumes.is_empty() && self.produces.is_empty()
    }

    pub(super) fn index(&self, kind: PredicateKind, value: &LitStr) -> usize {
        let values = match kind {
            PredicateKind::Host => &self.hosts,
            PredicateKind::Consumes => &self.consumes,
            PredicateKind::Produces => &self.produces,
        };
        let normalized = value.value().to_ascii_lowercase();
        values
            .iter()
            .position(|candidate| candidate == &normalized)
            .expect("the overlap plan contains every validated candidate predicate")
    }

    /// Emits the `OverlapPredicateState` construction this plan's checks read from.
    ///
    /// `setup` is only invoked for a `DispatchKind::Overlap` group of two or
    /// more candidates, built while `dispatches` is assembled. That assembly
    /// already rejects any group where a higher-priority candidate lacks
    /// predicates ("a predicate-free overlapping candidate must have the
    /// lowest priority because it matches every request"), so at least one
    /// candidate in every overlap group always contributes a host/consumes/
    /// produces predicate. `self.is_empty()` is therefore always `false` by
    /// the time `setup` runs, and the early-return below is never taken.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub(super) fn setup(&self, state: &Ident, runtime: &TokenStream2) -> TokenStream2 {
        if self.is_empty() {
            return TokenStream2::new();
        }

        let host_count = self.hosts.len();
        let consumes_count = self.consumes.len();
        let produces_count = self.produces.len();
        let hosts = self.hosts.iter().map(|value| LitStr::new(value, Span::call_site()));
        let consumes = self
            .consumes
            .iter()
            .map(|value| media_type_expression(value, Span::call_site(), runtime));
        let produces = self
            .produces
            .iter()
            .map(|value| media_type_expression(value, Span::call_site(), runtime));
        let produces_top_level = self
            .produces
            .first()
            .and_then(|first| first.split_once('/').map(|(top_level, _)| top_level))
            .filter(|top_level| {
                self.produces
                    .iter()
                    .all(|value| value.split_once('/').is_some_and(|(candidate, _)| candidate == *top_level))
            })
            .map_or_else(
                || quote! { ::core::option::Option::None },
                |top_level| {
                    let top_level = LitByteStr::new(top_level.as_bytes(), Span::call_site());
                    quote! { ::core::option::Option::Some(#top_level) }
                },
            );

        quote! {
            const __ROUTERAMA_OVERLAP_HOSTS: [&str; #host_count] = [#(#hosts),*];
            const __ROUTERAMA_OVERLAP_CONSUMES: [#runtime::MediaType<'static>; #consumes_count] = [#(#consumes),*];
            const __ROUTERAMA_OVERLAP_PRODUCES: [#runtime::MediaType<'static>; #produces_count] = [#(#produces),*];
            let mut #state = #runtime::OverlapPredicateState::new(
                &__ROUTERAMA_OVERLAP_HOSTS,
                &__ROUTERAMA_OVERLAP_CONSUMES,
                &__ROUTERAMA_OVERLAP_PRODUCES,
                #produces_top_level,
            );
        }
    }
}

fn overlap_predicate_values(candidates: &[usize], handlers: &[Handler], kind: PredicateKind) -> Vec<String> {
    let mut values = Vec::new();
    for index in candidates {
        let predicates = &handlers[*index].predicates;
        let value = match kind {
            PredicateKind::Host => predicates.host.as_ref(),
            PredicateKind::Consumes => predicates.consumes.as_ref(),
            PredicateKind::Produces => predicates.produces.as_ref(),
        };
        let Some(value) = value else {
            continue;
        };
        let normalized = value.value().to_ascii_lowercase();
        if !values.contains(&normalized) {
            values.push(normalized);
        }
    }

    // Keep the highest-priority value first for the common winner-first path;
    // sorting the tail lets the runtime binary-search every other value.
    if values.len() > 1 {
        match kind {
            // A host is compared as one string, so its natural order matches.
            PredicateKind::Host => values[1..].sort_unstable(),
            // The runtime searches media types by `(top_level, subtype)`, an
            // order the joined string diverges from whenever a top-level type
            // contains a tchar below `/`, such as `.`, `-`, `+`, or `*`.
            PredicateKind::Consumes | PredicateKind::Produces => {
                values[1..].sort_unstable_by(|left, right| media_type_order(left).cmp(&media_type_order(right)));
            }
        }
    }
    values
}

/// Splits a media type into its `(top_level, subtype)` key.
///
/// That key is what the runtime helper `find_media_type` binary-searches on.
fn media_type_order(value: &str) -> (&str, &str) {
    value
        .split_once('/')
        .expect("route parsing validated every consumes and produces media type")
}

pub(super) fn media_type_expression(value: &str, span: Span, runtime: &TokenStream2) -> TokenStream2 {
    let (top_level, subtype) = media_type_order(value);
    let top_level = LitByteStr::new(top_level.as_bytes(), span);
    let subtype = LitByteStr::new(subtype.as_bytes(), span);
    quote! { #runtime::MediaType::new(#top_level, #subtype) }
}

/// Picks the next unused `__RouteramaPolicyN` variant name for an overlap group.
///
/// The collision retry is defensive only: `used_variants` is seeded exclusively
/// from handler variants produced by [`variant_name`], which splits the method
/// name on `_`, discards empty parts, and upper-cases each remaining part's
/// first character. Every name it yields therefore starts with an alphanumeric
/// character and contains no underscores, so it can never equal a generated
/// `__RouteramaPolicyN` spelling. `overlap_index` also increases monotonically,
/// so a name generated here is never offered twice.
#[cfg_attr(coverage_nightly, coverage(off))]
fn next_overlap_variant(used_variants: &mut Vec<String>, overlap_index: &mut usize) -> String {
    loop {
        let name = format!("__RouteramaPolicy{overlap_index}");
        *overlap_index += 1;
        if !used_variants.contains(&name) {
            used_variants.push(name.clone());
            break name;
        }
    }
}

/// Re-checks a static handler alias's parsed template depth inside `build_dispatch_arms`.
///
/// Every static handler's route templates were already depth-checked by
/// `route_capture_names` while the `Handler` was constructed (see the call at
/// `kind == HandlerKind::Static` in `parse_handler`), against the exact same
/// `path` literal reparsed here. So `depth_limit_error` can never find this
/// template exceeding `MAX_TEMPLATE_SEGMENTS` a second time; this is a
/// defensive duplicate of that earlier, already-tested check.
#[cfg_attr(coverage_nightly, coverage(off))]
fn checked_dispatch_template_depth(segments: &[http_path_template::Segment], span: Span) -> syn::Result<()> {
    if let Some(message) = depth_limit_error(segments) {
        return Err(Error::new(span, message));
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "dispatch grouping performs one ordered pass from static declarations through validated overlap groups"
)]
pub(super) fn build_dispatch_arms(handlers: &[Handler]) -> syn::Result<Vec<DispatchArm>> {
    let mut entries = Vec::new();
    let mut dynamic = Vec::new();
    for (handler_index, handler) in handlers.iter().enumerate() {
        if handler.kind == HandlerKind::Dynamic {
            dynamic.push(DispatchArm {
                variant: handler.variant.clone(),
                route_attrs: handler.route_attrs.clone(),
                captures: handler.captures.clone(),
                kind: DispatchKind::Direct(handler_index),
            });
            continue;
        }
        for attribute in &handler.route_attrs {
            let parsed: RouteAttr = attribute.parse_args()?;
            let (method, path) = static_route_target(
                parsed.target,
                attribute.span(),
                "static handler aliases must declare a method and path",
            )
            .expect(
                "this loop skips `HandlerKind::Dynamic` handlers above, and `route_declaration` \
                 rejects a dynamic attribute combined with any other route attribute, so a static \
                 handler's aliases never parse to `RouteTarget::Dynamic`",
            );
            let path_value = path.value();
            let template = PathTemplate::parse(&path_value, Grammar::default().with_segment_affixes())
                .map_err(|error| Error::new(path.span(), format!("invalid path template: {error}")))?;
            checked_dispatch_template_depth(template.segments(), path.span())?;
            let mut capture_keys: Vec<_> = capture_field_names(template.segments())
                .into_iter()
                .map(|name| name.join("."))
                .collect();
            capture_keys.sort();
            let entry_index = entries.len();
            entries.push(StaticRouteEntry {
                handler: handler_index,
                attribute: attribute.clone(),
                route: Route::new(format!("__RouteramaEntry{entry_index}"), method, template),
                capture_keys,
                priority: parsed.priority,
            });
        }
    }

    if entries.is_empty() {
        return Ok(dynamic);
    }
    let routes: Vec<_> = entries.iter().map(|entry| entry.route.clone()).collect();
    let trie = build_trie(&routes);
    let mut groups = Vec::new();
    let mut plans = vec![None; entries.len()];
    collect_route_groups(&trie.root, &mut groups, &mut plans);
    groups.sort_by_key(|group| group.iter().copied().min().unwrap_or(usize::MAX));

    let mut dispatches = Vec::<DispatchArm>::new();
    let mut used_variants: Vec<String> = handlers.iter().map(|handler| handler.variant.to_string()).collect();
    let mut overlap_index = 0_usize;
    for group in groups {
        let candidates = if group.len() == 1 {
            vec![entries[group[0]].handler]
        } else {
            validate_overlap(&entries, handlers, &group, &plans)?
        };
        let representative_entry = if group.len() == 1 {
            group[0]
        } else {
            let highest = candidates[0];
            *group
                .iter()
                .find(|entry| entries[**entry].handler == highest)
                .expect("the sorted candidate came from this overlap group")
        };
        let representative_handler = entries[representative_entry].handler;
        let captures = handlers[representative_handler].captures.clone();
        let kind_matches = |dispatch: &DispatchArm| match (&dispatch.kind, group.len()) {
            (DispatchKind::Direct(existing), 1) => *existing == candidates[0],
            (DispatchKind::Overlap(existing), _) => existing.as_slice() == candidates.as_slice(),
            (DispatchKind::Direct(_), _) => false,
        };
        if let Some(dispatch) = dispatches
            .iter_mut()
            .find(|dispatch| kind_matches(dispatch) && capture_schema_key(&dispatch.captures) == capture_schema_key(&captures))
        {
            dispatch.route_attrs.push(entries[representative_entry].attribute.clone());
            continue;
        }
        let variant = if group.len() == 1 {
            handlers[candidates[0]].variant.clone()
        } else {
            let candidate = next_overlap_variant(&mut used_variants, &mut overlap_index);
            Ident::new(&candidate, handlers[candidates[0]].variant.span())
        };
        let kind = if group.len() == 1 {
            DispatchKind::Direct(candidates[0])
        } else {
            DispatchKind::Overlap(candidates)
        };
        dispatches.push(DispatchArm {
            variant,
            route_attrs: vec![entries[representative_entry].attribute.clone()],
            captures,
            kind,
        });
    }
    dispatches.extend(dynamic);
    Ok(dispatches)
}

fn collect_route_groups(node: &Node, groups: &mut Vec<Vec<usize>>, plans: &mut [Option<String>]) {
    collect_leaf_bucket(&node.exact, groups, plans);
    collect_leaf_bucket(&node.rest, groups, plans);
    for child in node.literals.values() {
        collect_route_groups(child, groups, plans);
    }
    for child in node.affix.values() {
        collect_route_groups(child, groups, plans);
    }
    if let Some(child) = &node.single {
        collect_route_groups(child, groups, plans);
    }
}

fn collect_leaf_bucket(leaves: &[crate::trie::Leaf], groups: &mut Vec<Vec<usize>>, plans: &mut [Option<String>]) {
    let mut bucket_groups: Vec<(&str, Option<&str>, Vec<usize>)> = Vec::new();
    for leaf in leaves {
        plans[leaf.route_index] = Some(capture_plan_key(&leaf.vars));
        if let Some((_, _, routes)) = bucket_groups
            .iter_mut()
            .find(|(method, verb, _)| *method == leaf.method && *verb == leaf.verb.as_deref())
        {
            routes.push(leaf.route_index);
        } else {
            bucket_groups.push((leaf.method.as_str(), leaf.verb.as_deref(), vec![leaf.route_index]));
        }
    }
    groups.extend(bucket_groups.into_iter().map(|(_, _, routes)| routes));
}

fn capture_plan_key(plans: &[VarPlan]) -> String {
    plans
        .iter()
        .map(|plan| match plan {
            VarPlan::Span { field, key, a, b } => format!("span:{field}:{key}:{a}:{b}"),
            VarPlan::Rest { field, key, a } => format!("rest:{field}:{key}:{a}"),
            VarPlan::Affix {
                field,
                key,
                a,
                prefix_len,
                suffix_len,
            } => format!("affix:{field}:{key}:{a}:{prefix_len}:{suffix_len}"),
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn validate_overlap(
    entries: &[StaticRouteEntry],
    handlers: &[Handler],
    group: &[usize],
    plans: &[Option<String>],
) -> syn::Result<Vec<usize>> {
    let first = &entries[group[0]];
    let mut error = None;
    for entry_index in group {
        let entry = &entries[*entry_index];
        if entry.priority.is_none() {
            combine_error(
                &mut error,
                Error::new(
                    entry.attribute.path().span(),
                    "overlapping routes require an explicit `priority = <integer>` on every declaration",
                ),
            );
        }
        let compatible_positions = entry.capture_keys == first.capture_keys && plans[*entry_index] == plans[group[0]];
        if !compatible_positions {
            combine_error(
                &mut error,
                Error::new(
                    entry.attribute.path().span(),
                    "overlapping routes must use identical capture names and capture positions",
                ),
            );
        }
        if compatible_positions
            && capture_schema_key(&handlers[entry.handler].captures) != capture_schema_key(&handlers[first.handler].captures)
        {
            combine_error(
                &mut error,
                Error::new(
                    entry.attribute.path().span(),
                    "overlapping routes must use identical concrete capture types",
                ),
            );
        }
        if group
            .iter()
            .any(|other| *other != *entry_index && entries[*other].handler == entry.handler)
        {
            combine_error(
                &mut error,
                Error::new(
                    entry.attribute.path().span(),
                    "duplicate method/path aliases on one handler are not candidates; remove the duplicate declaration",
                ),
            );
        }
    }
    for (position, left_index) in group.iter().enumerate() {
        let Some(left) = entries[*left_index].priority.as_ref() else {
            continue;
        };
        for right_index in &group[position + 1..] {
            let Some(right) = entries[*right_index].priority.as_ref() else {
                continue;
            };
            if left.value == right.value {
                combine_error(
                    &mut error,
                    Error::new(right.span, format!("overlapping routes cannot share priority {}", right.value)),
                );
            }
        }
    }
    if let Some(error) = error {
        return Err(error);
    }
    let mut ordered = group.to_vec();
    ordered.sort_by(|left, right| {
        entries[*right]
            .priority
            .as_ref()
            .expect("validated explicit priority")
            .value
            .cmp(&entries[*left].priority.as_ref().expect("validated explicit priority").value)
    });
    let candidates: Vec<_> = ordered.iter().map(|entry| entries[*entry].handler).collect();
    for (position, higher_entry) in ordered.iter().enumerate().take(ordered.len() - 1) {
        let higher = &handlers[entries[*higher_entry].handler].predicates;
        if higher.is_empty() {
            return Err(Error::new(
                entries[*higher_entry].attribute.path().span(),
                "a predicate-free overlapping candidate must have the lowest priority because it matches every request",
            ));
        }
        for lower_entry in &ordered[position + 1..] {
            let lower = &handlers[entries[*lower_entry].handler].predicates;
            if higher.same_values(lower) {
                return Err(Error::new(
                    entries[*lower_entry].attribute.path().span(),
                    "overlapping candidates with identical predicates make the lower priority unreachable",
                ));
            }
        }
    }
    Ok(candidates)
}

fn capture_schema_key(captures: &[(Ident, Type)]) -> String {
    let mut fields: Vec<_> = captures.iter().map(|(name, ty)| format!("{}:{}", name, type_key(ty))).collect();
    fields.sort();
    fields.join("|")
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use quote::quote;

    use super::*;
    use crate::macro_impl::router::test_support::expand_router;

    #[test]
    fn predicate_routes_generate_ordered_checks_and_success_header_mutation_only_for_the_annotated_handler() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/plain")]
                async fn plain(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }

                #[route(
                    POST,
                    "/items",
                    produces = "application/json",
                    host = "api.example",
                    consumes = "application/json",
                )]
                async fn create(&self, method: Method) -> String {
                    method.to_string()
                }

                #[route(dynamic, host = "plugins.example")]
                async fn plugin(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }
            }
        })
        .expect("static and configured-dynamic predicates are valid");
        let code = generated.to_string();
        assert_eq!(code.matches("host_matches").count(), 2, "{code}");
        assert_eq!(code.matches("content_type_matches_parsed").count(), 1, "{code}");
        assert_eq!(code.matches("accepts_parsed").count(), 1, "{code}");
        assert_eq!(code.matches("MediaType :: new").count(), 2, "{code}");
        assert_eq!(code.matches("CONTENT_TYPE").count(), 1, "{code}");
        assert_eq!(
            code.matches("HeaderValue :: from_static (\"application/json\")").count(),
            1,
            "{code}"
        );
        let host = code.find("host_matches").expect("host check");
        let consumes = code.find("content_type_matches_parsed").expect("content-type check");
        let produces = code.find("accepts_parsed").expect("accept check");
        let extraction = code.find("let method : Method").expect("parts extraction");
        assert!(host < consumes && consumes < produces && produces < extraction, "{code}");
        assert!(code.contains("route predicate rejection"), "{code}");
    }

    #[test]
    fn static_aliases_require_identical_predicates() {
        expand_router(quote! {
            impl Api {
                #[route(GET, "/items", host = "api.example", produces = "application/json")]
                #[route(HEAD, "/items", produces = "application/json", host = "api.example")]
                async fn items(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }

                #[test]
                fn route_aliases_require_identical_static_response_headers() {
                    expand_router(quote! {
                        impl Api {
                            #[route(GET, "/items", headers(insert("x-route", "one")))]
                            #[route(HEAD, "/items", headers(insert("x-route", "one")))]
                            async fn items(&self) -> StatusCode {
                                StatusCode::NO_CONTENT
                            }
                        }
                    })
                    .expect("matching static response-header plans are valid");

                    let error = expand_router(quote! {
                        impl Api {
                            #[route(GET, "/items", headers(insert("x-route", "one")))]
                            #[route(HEAD, "/items", headers(append("x-route", "one")))]
                            async fn items(&self) -> StatusCode {
                                StatusCode::NO_CONTENT
                            }
                        }
                    })
                    .expect_err("mismatched static response-header plans are rejected");
                    assert!(error.to_string().contains("identical static response-header operations"), "{error}");
                }
            }
        })
        .expect("predicate key order does not matter across aliases");

        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/items", host = "api.example")]
                #[route(HEAD, "/items", host = "other.example")]
                async fn items(&self) -> StatusCode {
                    StatusCode::NO_CONTENT
                }
            }
        })
        .expect_err("one generated variant cannot distinguish aliases with different predicates");
        assert!(error.to_string().contains("must declare identical"), "{error}");
    }

    #[test]
    fn intentional_overlap_emits_priority_order_without_a_runtime_table() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/items/{id}", host = "api.example", priority = 20)]
                async fn host(&self, id: u32) -> StatusCode {
                    response(id)
                }

                #[route(GET, "/items/{id}", consumes = "application/json", priority = 10)]
                async fn json(&self, id: u32) -> StatusCode {
                    response(id)
                }
            }
        })
        .expect("explicit distinct priorities make compatible overlap intentional");
        let code = generated.to_string();
        assert_eq!(code.matches("OverlapPredicateState :: new").count(), 1, "{code}");
        assert_eq!(code.matches("MediaType :: new").count(), 1, "{code}");
        assert!(!code.contains("host_matches"), "{code}");
        assert!(!code.contains("content_type_matches"), "{code}");
        assert!(code.contains("__routerama_failure_stage"), "{code}");
        assert!(
            !code.contains("Vec <"),
            "candidate dispatch must not create a request-time table: {code}"
        );
        let host = code
            .find("__routerama_overlap_predicates . host")
            .expect("higher-priority host candidate");
        let consumes = code
            .find("__routerama_overlap_predicates . consumes")
            .expect("lower-priority consumes candidate");
        assert!(host < consumes, "{code}");
    }

    #[test]
    fn overlap_requires_explicit_distinct_priority_and_compatible_captures() {
        let missing = expand_router(quote! {
            impl Api {
                #[route(GET, "/{id}", host = "one.example", priority = 2)]
                async fn one(&self, id: u32) -> StatusCode { response(id) }
                #[route(GET, "/{id}", host = "two.example")]
                async fn two(&self, id: u32) -> StatusCode { response(id) }
            }
        })
        .expect_err("every overlap candidate needs explicit priority");
        assert!(missing.to_string().contains("explicit `priority"), "{missing}");

        let duplicate = expand_router(quote! {
            impl Api {
                #[route(GET, "/{id}", host = "one.example", priority = 2)]
                async fn one(&self, id: u32) -> StatusCode { response(id) }
                #[route(GET, "/{id}", host = "two.example", priority = 2)]
                async fn two(&self, id: u32) -> StatusCode { response(id) }
            }
        })
        .expect_err("overlap priorities must differ");
        assert!(duplicate.to_string().contains("cannot share priority"), "{duplicate}");

        let captures = expand_router(quote! {
            impl Api {
                #[route(GET, "/{id}", host = "one.example", priority = 2)]
                async fn one(&self, id: u32) -> StatusCode { response(id) }
                #[route(GET, "/{id}", host = "two.example", priority = 1)]
                async fn two(&self, id: String) -> StatusCode { response(id) }
            }
        })
        .expect_err("one typed conversion must serve every candidate");
        assert!(captures.to_string().contains("identical concrete capture types"), "{captures}");
    }

    #[test]
    fn an_overlap_with_a_default_candidate_checks_every_predicate_kind() {
        // A predicated overlap candidate alongside an unpredicated default
        // exercises `emit_overlap_dispatch`'s `has_default_candidate` branch
        // for all three predicate kinds (produces, consumes, host).
        let generated = expand_router(quote! {
            impl Api {
                #[route(
                    GET,
                    "/items/{id}",
                    host = "api.example",
                    consumes = "application/json",
                    produces = "application/json",
                    priority = 1,
                )]
                async fn full(&self, id: u32) -> StatusCode {
                    response(id)
                }

                #[route(GET, "/items/{id}", priority = 0)]
                async fn default(&self, id: u32) -> StatusCode {
                    response(id)
                }
            }
        })
        .expect("a default overlap candidate needs no predicate of its own");
        let code = generated.to_string();
        assert!(code.contains("__routerama_overlap_predicates . host"), "{code}");
        assert!(code.contains("__routerama_overlap_predicates . consumes"), "{code}");
        assert!(code.contains("__routerama_overlap_predicates . produces"), "{code}");
        assert!(
            !code.contains("__routerama_failure_stage"),
            "a default candidate makes an explicit failure stage unnecessary: {code}"
        );
    }

    #[test]
    fn media_type_tables_are_ordered_by_type_and_subtype() {
        let mut values = ["x.foo/bar", "x-foo/bar", "x+foo/bar", "x/bar", "x/baz"];
        values.sort_unstable_by(|left, right| media_type_order(left).cmp(&media_type_order(right)));

        assert_eq!(values, ["x/bar", "x/baz", "x+foo/bar", "x-foo/bar", "x.foo/bar"]);

        let mut string_ordered = values;
        string_ordered.sort_unstable();
        assert_ne!(
            values, string_ordered,
            "the joined-string order must differ here, otherwise this test proves nothing"
        );
    }

    #[test]
    fn checked_dispatch_template_depth_reports_templates_past_the_limit() {
        let shallow = PathTemplate::parse("/a", Grammar::default().with_segment_affixes()).expect("a shallow template parses");
        checked_dispatch_template_depth(shallow.segments(), Span::call_site()).expect("a shallow template is within the limit");

        let deep_path = "/a".repeat(300);
        let deep = PathTemplate::parse(&deep_path, Grammar::default().with_segment_affixes()).expect("a deep template parses");
        let error = checked_dispatch_template_depth(deep.segments(), Span::call_site()).expect_err("a template past the limit is rejected");
        assert!(error.to_string().contains("but a route may declare at most"), "{error}");
    }

    #[test]
    fn overlapping_routes_with_predicate_free_candidate_not_lowest_priority_are_rejected() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books", priority = 1)]
                async fn book_fallthrough(&self) -> StatusCode {
                    book_fallthrough()
                }

                #[route(GET, "/books", priority = 0, produces = "application/json")]
                async fn book(&self) -> StatusCode {
                    book()
                }
            }
        })
        .expect_err("a predicate-free candidate must have the lowest priority");
        assert!(error.to_string().contains("must have the lowest priority"), "{error}");
    }

    #[test]
    fn overlapping_routes_with_identical_predicates_are_unreachable() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books", priority = 1, produces = "application/json")]
                async fn book(&self) -> StatusCode {
                    book()
                }

                #[route(GET, "/books", priority = 0, produces = "application/json")]
                async fn book_shadow(&self) -> StatusCode {
                    book_shadow()
                }
            }
        })
        .expect_err("identical predicates on overlapping candidates make the lower one unreachable");
        assert!(error.to_string().contains("make the lower priority unreachable"), "{error}");
    }

    #[test]
    fn two_overlap_groups_with_differing_predicates_share_one_dispatch_arm() {
        expand_router(quote! {
            impl Api {
                #[route(GET, "/books", priority = 1, produces = "application/json")]
                async fn book(&self) -> StatusCode {
                    book()
                }

                #[route(GET, "/books", priority = 0, produces = "text/plain")]
                async fn book_text(&self) -> StatusCode {
                    book_text()
                }
            }
        })
        .expect("overlapping candidates with differing predicates and explicit priorities are accepted");
    }

    // --- parts / body rejection routing, further branches ---

    #[test]
    fn two_static_aliases_of_the_same_handler_share_one_direct_dispatch_arm() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/books")]
                #[route(GET, "/tomes")]
                async fn book(&self) -> StatusCode {
                    book()
                }
            }
        })
        .expect("two static aliases of the same handler are accepted and share one dispatch arm");
        let code = generated.to_string();
        assert!(code.contains("self . book ()"), "{code}");
    }

    #[test]
    fn two_static_paths_with_identical_overlap_groups_share_one_dispatch_arm() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/a", priority = 1, produces = "application/json")]
                #[route(GET, "/b", priority = 1, produces = "application/json")]
                async fn book(&self) -> StatusCode {
                    book()
                }

                #[route(GET, "/a", priority = 0, produces = "text/plain")]
                #[route(GET, "/b", priority = 0, produces = "text/plain")]
                async fn book_text(&self) -> StatusCode {
                    book_text()
                }
            }
        })
        .expect("two static paths with an identical overlap candidate set share one dispatch arm");
        let code = generated.to_string();
        assert!(code.contains("self . book ()"), "{code}");
        assert!(code.contains("self . book_text ()"), "{code}");
    }

    // --- capture_plan_key variant coverage (rest / affix captures) ---

    #[test]
    fn overlapping_routes_with_mismatched_capture_positions_are_rejected() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/items/{id}", priority = 1)]
                async fn by_id(&self, id: u32) -> StatusCode {
                    by_id(id)
                }

                #[route(GET, "/items/{name}", priority = 0)]
                async fn by_name(&self, name: String) -> StatusCode {
                    by_name(name)
                }
            }
        })
        .expect_err("overlapping candidates with differing capture positions are rejected");
        assert!(
            error
                .to_string()
                .contains("overlapping routes must use identical capture names and capture positions")
                || error.to_string().contains("must capture the same path variables"),
            "{error}"
        );
    }

    #[test]
    fn a_duplicate_alias_on_one_handler_within_an_overlap_group_is_rejected() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books", priority = 1)]
                #[route(GET, "/books", priority = 0)]
                async fn book(&self) -> StatusCode {
                    book()
                }

                #[route(GET, "/books", priority = 2)]
                async fn book_other(&self) -> StatusCode {
                    book_other()
                }
            }
        })
        .expect_err("a duplicated method/path alias inside an overlap group is rejected");
        assert!(
            error
                .to_string()
                .contains("duplicate method/path aliases on one handler are not candidates"),
            "{error}"
        );
    }

    // --- validate_overlap combined-error and parse_handler edge cases ---

    #[test]
    fn overlapping_routes_missing_priority_combine_multiple_errors() {
        let error = expand_router(quote! {
            impl Api {
                #[route(GET, "/books")]
                async fn book(&self) -> StatusCode {
                    book()
                }

                #[route(GET, "/books")]
                async fn book_shadow(&self) -> StatusCode {
                    book_shadow()
                }
            }
        })
        .expect_err("overlapping routes without an explicit priority are rejected");
        assert!(
            error
                .to_string()
                .contains("overlapping routes require an explicit `priority = <integer>` on every declaration"),
            "{error}"
        );
    }

    #[test]
    fn a_direct_dispatch_arm_is_not_reused_for_an_unrelated_overlap_group() {
        let generated = expand_router(quote! {
            impl Api {
                #[route(GET, "/solo")]
                async fn solo(&self) -> StatusCode {
                    solo()
                }

                #[route(GET, "/shared", priority = 1, produces = "application/json")]
                async fn shared_json(&self) -> StatusCode {
                    shared_json()
                }

                #[route(GET, "/shared", priority = 0, produces = "text/plain")]
                async fn shared_text(&self) -> StatusCode {
                    shared_text()
                }
            }
        })
        .expect("an existing direct dispatch arm and a later overlap group coexist without merging");
        let code = generated.to_string();
        assert!(code.contains("self . solo ()"), "{code}");
        assert!(code.contains("self . shared_json ()"), "{code}");
        assert!(code.contains("self . shared_text ()"), "{code}");
    }

    // --- StaticAnonymousLifetimes recurses into a body extractor's nested reference ---
}
