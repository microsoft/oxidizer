// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::option_if_let_else, reason = "Darling's macro expansion currently uses this pattern")]
#![expect(
    clippy::needless_continue,
    reason = "Darling's macro expansion triggers this lint until next version gets released (https://github.com/TedDriggs/darling/pull/402)"
)]

use std::collections::HashSet;

use darling::{FromAttributes, FromField};
use proc_macro2::{Ident, TokenStream};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Attribute, DataStruct, Field};

use crate::template_parser::{ParamGroup, TemplatePart, UriTemplate};

type FieldMap<'a> = std::collections::HashMap<String, &'a Field>;
type FieldOptsMap<'a> = std::collections::HashMap<String, &'a FieldOpts>;

#[derive(Debug, FromAttributes)]
#[darling(attributes(templated))]
pub struct Opts {
    #[darling(rename = "template")]
    pub input_template: String,
    #[darling(default)]
    pub unredacted: bool,
    /// Optional label for telemetry. When provided, this label is used in metrics
    /// instead of the full template string, which is useful for complex templates.
    #[darling(default)]
    pub label: Option<String>,
}

#[derive(Debug, FromField)]
#[darling(attributes(templated))]
pub struct FieldOpts {
    pub ident: Option<Ident>,
    #[darling(default)]
    pub unredacted: bool,
}

/// Represents the fields of a struct with their options parsed from attributes.
struct Fields {
    fields: Vec<FieldOpts>,
}

impl Fields {
    /// Constructs a new `Fields` instance by parsing a slice of `Field`.
    fn from_fields(fields: &[&Field]) -> darling::Result<Self> {
        let fields = fields
            .iter()
            .map(|&f| {
                let mut opts = FieldOpts::from_field(f)?;
                // Also check for standalone #[unredacted] attribute
                if !opts.unredacted {
                    opts.unredacted = f.attrs.iter().any(|attr| attr.path().is_ident("unredacted"));
                }
                Ok(opts)
            })
            .collect::<darling::Result<Vec<_>>>()?;
        Ok(Self { fields })
    }

    /// Returns the names of the fields in the struct
    fn field_names(&self) -> HashSet<String> {
        self.fields
            .iter()
            .filter_map(|f| f.ident.as_ref().map(ToString::to_string))
            .collect()
    }
}

// #[proc_macro_derive(TemplatedPathAndQuery, attributes(templated, unredacted))]
pub fn struct_template(ident: Ident, data: &DataStruct, attrs: &[Attribute]) -> TokenStream {
    if !matches!(data.fields, syn::Fields::Named(_)) {
        crate::bail!(ident, "#[templated] can only be applied to structs with named fields");
    }

    // Parse the derive input using the Opts struct with custom parsing
    let struct_name = ident.to_string();
    let Opts {
        input_template,
        unredacted,
        label,
    } = match Opts::from_attributes(attrs) {
        Ok(opts) => opts,
        Err(err) => return err.write_errors(),
    };

    let template = match UriTemplate::parse(&input_template) {
        Ok(template) => template,
        Err(err) => {
            return err.to_compile_error(ident.span());
        }
    };

    let format_template = template.format_template();

    let template_params: Vec<_> = template.params().collect();
    let struct_fields: Vec<&Field> = data.fields.iter().collect();

    let fields = match Fields::from_fields(struct_fields.as_slice()) {
        Ok(fields) => fields,
        Err(err) => return err.write_errors(),
    };

    let struct_field_names = fields.field_names();
    let template_param_names = template.param_names();

    // Compare the template parameters with the struct fields and errors if there are mismatches.
    let mut excess_values: Vec<_> = struct_field_names.difference(&template_param_names).collect();
    excess_values.sort();

    let mut missing_values: Vec<_> = template_param_names.difference(&struct_field_names).collect();
    missing_values.sort();

    if !missing_values.is_empty() {
        crate::bail!(ident, "Missing values in struct: {missing_values:?}")
    }

    if !excess_values.is_empty() {
        crate::bail!(ident, "Excess values in struct: {excess_values:?}")
    }

    // Determine which parameters are unrestricted (Can contain any value) and which are restricted (Must be `Escaped`).
    let unrestricted_params: HashSet<String> = template_params
        .iter()
        .filter(|p| p.is_unrestricted)
        .map(|p| p.name.to_owned())
        .collect();

    let render_body = construct_render(&template, &struct_fields, &unrestricted_params);
    let redacted_display = construct_redacted_display(&template, &struct_fields, &fields, unredacted);

    let label_impl = label.as_ref().map_or_else(
        || quote! { ::core::option::Option::None },
        |l| quote! { ::core::option::Option::Some(#l) },
    );

    quote! {
        impl ::templated_uri::PathAndQueryTemplate for #ident {
            fn template(&self) -> &'static core::primitive::str {
                #input_template
            }

            fn format_template(&self) -> &'static core::primitive::str {
                #format_template
            }

            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                #label_impl
            }

            fn render(&self) -> ::std::string::String {
                #render_body
            }

            fn to_path_and_query(&self) -> ::std::result::Result<::templated_uri::http::uri::PathAndQuery, ::templated_uri::UriError> {
                Ok(::templated_uri::http::uri::PathAndQuery::try_from(::templated_uri::PathAndQueryTemplate::render(self))?)
            }
        }

        impl ::std::fmt::Debug for #ident {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.debug_tuple(#struct_name)
                    .field(&#input_template)
                    .finish()
            }
        }

        impl ::templated_uri::RedactedDisplay for #ident {
            fn fmt(&self, redactor: &dyn ::templated_uri::Redactor, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                #redacted_display
            }
        }

        impl From<#ident> for ::templated_uri::PathAndQuery {
            fn from(value: #ident) -> Self {
                ::templated_uri::PathAndQuery::from_template(value)
            }
        }
    }
}

/// Checks whether `ty` is syntactically `Option<T>` (or `std::option::Option<T>`, etc.)
/// and returns the inner type `T`.
///
/// This is the standard approach used by serde, clap, and other derive macros.
/// It won't detect type aliases for `Option`, which is a known and accepted limitation.
fn extract_option_inner(ty: &syn::Type) -> Option<&syn::Type> {
    let syn::Type::Path(type_path) = ty else {
        return None;
    };
    let last_segment = type_path.path.segments.last()?;
    if last_segment.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last_segment.arguments else {
        return None;
    };
    if args.args.len() != 1 {
        return None;
    }
    let syn::GenericArgument::Type(inner_ty) = &args.args[0] else {
        return None;
    };
    Some(inner_ty)
}

/// Generates the body of the `render()` method.
///
/// Walks the parsed template parts and emits `Write::write_fmt` calls (via UFCS,
/// to avoid bringing `fmt::Write` into the caller's scope), handling RFC 6570
/// undefined-value semantics for `Option<T>` fields.
fn construct_render(template: &UriTemplate, struct_fields: &[&Field], unrestricted_params: &HashSet<String>) -> TokenStream {
    let field_map: FieldMap<'_> = struct_fields
        .iter()
        .filter_map(|f| f.ident.as_ref().map(|ident| (ident.to_string(), *f)))
        .collect();

    // Pre-allocate the rendered-URI buffer using a static heuristic computed from the
    // template parts. See `render_capacity_hint` for the precise semantics; it may
    // slightly over-allocate when a group's parameters are all `None` at runtime, which
    // is far cheaper than the realloc chain a fresh `String::new()` would incur.
    let initial_capacity = render_capacity_hint(template);

    let statements: Vec<TokenStream> = template
        .template_parts()
        .iter()
        .flat_map(|part| match part {
            TemplatePart::Content(content) => {
                vec![quote! { __out.push_str(#content); }]
            }
            TemplatePart::ParamGroup(group) => construct_render_group(group, &field_map, unrestricted_params),
        })
        .collect();

    quote! {
        let mut __out = ::std::string::String::with_capacity(#initial_capacity);
        #(#statements)*
        __out
    }
}

/// Compile-time capacity heuristic for the `String` buffer that holds a rendered URI.
///
/// The returned size counts every byte the macro can statically *name*:
///
/// - Static `Content` segments are always present.
/// - Each parameter group's fixed literals (prefix, separators between values, and the
///   `key=` literal for key/value expansions like `{?a}` and `{;a}`).
///
/// Parameter values themselves are not counted because their runtime size is unknown.
/// The heuristic does **not** discount per-parameter literals attached to `Option<T>`
/// fields, so when a group's optional values are all `None` at runtime, the buffer
/// over-allocates by a few bytes (typically one prefix byte plus the key length plus
/// `=`). That small over-attribution is harmless: a strict lower bound would require
/// threading field metadata into this function, and the worst-case waste is bounded
/// by a few bytes per group, well below a single allocator slab.
fn render_capacity_hint(template: &UriTemplate) -> usize {
    template
        .template_parts()
        .iter()
        .map(|part| match part {
            TemplatePart::Content(content) => content.len(),
            TemplatePart::ParamGroup(group) => {
                let prefix_len = group.prefix().map_or(0, str::len);
                let param_names = group.param_names();
                // Every RFC 6570 separator emitted by `ParamKind::separator()` (`,`, `;`,
                // `&`, `.`, `/`) is exactly 1 byte, so the separator-byte count equals the
                // number of separators. If the vocabulary ever grows a multi-byte member,
                // multiply by `group.separator().len()` and add a unit test for it.
                let separators_len = param_names.len().saturating_sub(1);
                let kv_len = if group.is_kv() {
                    // Each value gets `key=` prepended.
                    param_names.iter().map(|n| n.len() + 1).sum::<usize>()
                } else {
                    0
                };
                prefix_len + separators_len + kv_len
            }
        })
        .sum()
}

/// Returns true if any parameter in `param_names` is backed by an `Option<T>` field.
fn group_has_any_optional(param_names: &[&str], field_map: &FieldMap<'_>) -> bool {
    param_names
        .iter()
        .any(|name| field_map.get(*name).is_some_and(|f| extract_option_inner(&f.ty).is_some()))
}

/// Returns the `(emit_delim, emit_kv)` token-stream pair used inside the optional-aware
/// `__first`-tracked code paths in both `render_group_with_optional` and
/// `redacted_display_group_with_optional`.
///
/// `write_lit` is a closure that emits the writer-specific statement for writing a
/// single string literal: `__out.push_str(s)` for render, `f.write_str(s)?` for the
/// `RedactedDisplay` impl. Factoring this here keeps both code paths in lockstep so
/// future operator tweaks can't drift between `render()` and `RedactedDisplay::fmt`.
fn emit_optional_delim_and_kv(
    prefix: &str,
    separator: &str,
    key: Option<&str>,
    write_lit: impl Fn(&str) -> TokenStream,
) -> (TokenStream, TokenStream) {
    let emit_delim = if prefix.is_empty() {
        let sep = write_lit(separator);
        quote! { if !__first { #sep } }
    } else {
        let pfx = write_lit(prefix);
        let sep = write_lit(separator);
        quote! { if __first { #pfx } else { #sep } }
    };
    let emit_kv = key.map_or_else(TokenStream::new, |k| {
        let key_tok = write_lit(k);
        let eq_tok = write_lit("=");
        quote! { #key_tok #eq_tok }
    });
    (emit_delim, emit_kv)
}

/// Generates render code for a single parameter group (e.g. `{?x,y}`, `{/a,b}`, `{x}`).
///
/// Dispatches to the all-required fast path or the optional-aware path depending on
/// whether the group contains any `Option<T>` field.
fn construct_render_group(group: &ParamGroup, field_map: &FieldMap<'_>, unrestricted_params: &HashSet<String>) -> Vec<TokenStream> {
    if group_has_any_optional(group.param_names(), field_map) {
        render_group_with_optional(group, field_map, unrestricted_params)
    } else {
        render_group_all_required(group, field_map, unrestricted_params)
    }
}

/// Render path for groups whose parameters are all required.
///
/// Emits a flat sequence of `push_str` and `write!` statements. No `__first`
/// tracking is needed because every parameter contributes a value.
fn render_group_all_required(group: &ParamGroup, field_map: &FieldMap<'_>, unrestricted_params: &HashSet<String>) -> Vec<TokenStream> {
    let prefix = group.prefix().unwrap_or_default();
    let separator = group.separator();
    let is_kv = group.is_kv();
    let param_names = group.param_names();

    let mut stmts = Vec::new();
    for (i, param_name) in param_names.iter().enumerate() {
        let delim = if i == 0 { prefix } else { separator };
        if !delim.is_empty() {
            stmts.push(quote! { __out.push_str(#delim); });
        }
        if is_kv {
            let key = *param_name;
            stmts.push(quote! { __out.push_str(#key); __out.push_str("="); });
        }
        let field = field_map.get(*param_name).expect("field should exist (validated earlier)");
        let field_ident = field.ident.as_ref().expect("struct fields must be named");
        let ty_span = field.ty.span();
        if unrestricted_params.contains(*param_name) {
            stmts.push(quote_spanned! { ty_span => ::std::fmt::Write::write_fmt(&mut __out, ::core::format_args!("{}", ::templated_uri::Raw::raw(&self.#field_ident))).expect("Display impls for templated URI parameter values are expected to be infallible"); });
        } else {
            stmts.push(
                quote_spanned! { ty_span => ::std::fmt::Write::write_fmt(&mut __out, ::core::format_args!("{}", ::templated_uri::Escape::escape(&self.#field_ident))).expect("Display impls for templated URI parameter values are expected to be infallible"); },
            );
        }
    }
    stmts
}

/// Render path for groups containing at least one `Option<T>` parameter.
///
/// Emits a `__first`-tracked block per RFC 6570 section 3.2: when a variable is
/// undefined (`None`), its prefix or separator is also omitted so that the
/// first *defined* variable receives the prefix and subsequent defined
/// variables receive the separator.
fn render_group_with_optional(group: &ParamGroup, field_map: &FieldMap<'_>, unrestricted_params: &HashSet<String>) -> Vec<TokenStream> {
    let prefix = group.prefix().unwrap_or_default();
    let separator = group.separator();
    let is_kv = group.is_kv();
    let param_names = group.param_names();

    let mut inner_stmts = Vec::new();
    inner_stmts.push(quote! { let mut __first = true; });

    for param_name in param_names {
        let field = field_map.get(*param_name).expect("field should exist (validated earlier)");
        let field_ident = field.ident.as_ref().expect("struct fields must be named");
        let optional_inner = extract_option_inner(&field.ty);
        let ty_span = optional_inner.map_or_else(|| field.ty.span(), syn::spanned::Spanned::span);

        // For `Option<&T>` the `Some(ref __val)` binding produces `__val: &&T`. Pass `*__val`
        // (which is `&T`) to the trait method so resolution sees the intended receiver type.
        let inner_is_reference = optional_inner.is_some_and(|ty| matches!(ty, syn::Type::Reference(_)));
        let val_arg = if inner_is_reference {
            quote! { *__val }
        } else {
            quote! { __val }
        };

        let escape_expr = if unrestricted_params.contains(*param_name) {
            quote_spanned! { ty_span => ::templated_uri::Raw::raw(#val_arg) }
        } else {
            quote_spanned! { ty_span => ::templated_uri::Escape::escape(#val_arg) }
        };

        let key_for_kv = is_kv.then_some(*param_name);
        let (emit_delim, emit_kv) = emit_optional_delim_and_kv(prefix, separator, key_for_kv, |s| quote! { __out.push_str(#s); });

        let body = quote! {
            #emit_delim
            #emit_kv
            ::std::fmt::Write::write_fmt(&mut __out, ::core::format_args!("{}", #escape_expr)).expect("Display impls for templated URI parameter values are expected to be infallible");
            __first = false;
        };

        if optional_inner.is_some() {
            inner_stmts.push(quote! {
                if let ::core::option::Option::Some(ref __val) = self.#field_ident {
                    #body
                }
            });
        } else {
            inner_stmts.push(quote! {
                {
                    let __val = &self.#field_ident;
                    #body
                }
            });
        }
    }

    vec![quote! { { #(#inner_stmts)* } }]
}

fn construct_redacted_display(template: &UriTemplate, struct_fields: &[&Field], fields: &Fields, unredacted: bool) -> TokenStream {
    let field_map: FieldMap<'_> = struct_fields
        .iter()
        .filter_map(|f| f.ident.as_ref().map(|ident| (ident.to_string(), *f)))
        .collect();

    let field_opts_map: FieldOptsMap<'_> = fields
        .fields
        .iter()
        .filter_map(|f| f.ident.as_ref().map(|ident| (ident.to_string(), f)))
        .collect();

    let statements: Vec<TokenStream> = template
        .template_parts()
        .iter()
        .flat_map(|part| match part {
            TemplatePart::Content(content) => {
                vec![quote! { f.write_str(#content)?; }]
            }
            TemplatePart::ParamGroup(group) => construct_redacted_display_group(group, &field_map, &field_opts_map, unredacted),
        })
        .collect();

    quote! {
        #(#statements)*
        ::std::result::Result::Ok(())
    }
}

/// Generates redacted-display code for a single parameter group.
///
/// Dispatches to the all-required fast path or the optional-aware path depending on
/// whether the group contains any `Option<T>` field.
fn construct_redacted_display_group(
    group: &ParamGroup,
    field_map: &FieldMap<'_>,
    field_opts_map: &FieldOptsMap<'_>,
    unredacted: bool,
) -> Vec<TokenStream> {
    if group_has_any_optional(group.param_names(), field_map) {
        redacted_display_group_with_optional(group, field_map, field_opts_map, unredacted)
    } else {
        redacted_display_group_all_required(group, field_map, field_opts_map, unredacted)
    }
}

/// Redacted-display path for groups whose parameters are all required.
fn redacted_display_group_all_required(
    group: &ParamGroup,
    field_map: &FieldMap<'_>,
    field_opts_map: &FieldOptsMap<'_>,
    unredacted: bool,
) -> Vec<TokenStream> {
    let prefix = group.prefix().unwrap_or_default();
    let separator = group.separator();
    let is_kv = group.is_kv();
    let param_names = group.param_names();

    let mut stmts = Vec::new();
    for (i, param_name) in param_names.iter().enumerate() {
        let delim = if i == 0 { prefix } else { separator };
        if !delim.is_empty() {
            stmts.push(quote! { f.write_str(#delim)?; });
        }
        if is_kv {
            let key = *param_name;
            stmts.push(quote! { f.write_str(#key)?; f.write_str("=")?; });
        }
        let field = field_map.get(*param_name).expect("field should exist (validated earlier)");
        let field_ident = field.ident.as_ref().expect("struct fields must be named");
        let field_type = &field.ty;
        let field_unredacted = field_opts_map.get(*param_name).is_some_and(|opts| opts.unredacted);

        if unredacted || field_unredacted {
            stmts.push(quote! { ::std::write!(f, "{}", self.#field_ident)?; });
        } else {
            stmts.push(quote! { <#field_type as ::templated_uri::RedactedDisplay>::fmt(&self.#field_ident, redactor, f)?; });
        }
    }
    stmts
}

/// Redacted-display path for groups containing at least one `Option<T>` parameter.
///
/// Mirrors `render_group_with_optional`: undefined values are skipped along with
/// their prefix/separator using `__first` tracking.
fn redacted_display_group_with_optional(
    group: &ParamGroup,
    field_map: &FieldMap<'_>,
    field_opts_map: &FieldOptsMap<'_>,
    unredacted: bool,
) -> Vec<TokenStream> {
    let prefix = group.prefix().unwrap_or_default();
    let separator = group.separator();
    let is_kv = group.is_kv();
    let param_names = group.param_names();

    let mut inner_stmts = Vec::new();
    inner_stmts.push(quote! { let mut __first = true; });

    for param_name in param_names {
        let field = field_map.get(*param_name).expect("field should exist (validated earlier)");
        let field_ident = field.ident.as_ref().expect("struct fields must be named");
        let optional_inner = extract_option_inner(&field.ty);
        let field_unredacted = field_opts_map.get(*param_name).is_some_and(|opts| opts.unredacted);

        let key_for_kv = is_kv.then_some(*param_name);
        let (emit_delim, emit_kv) = emit_optional_delim_and_kv(prefix, separator, key_for_kv, |s| quote! { f.write_str(#s)?; });

        if let Some(inner_type) = optional_inner {
            // For `Option<&T>` the `Some(ref __val)` binding produces `__val: &&T`. Peel
            // the AST `Type::Reference` and dereference once so the generated trait call
            // resolves against `T: RedactedDisplay`, not the less useful `&T: RedactedDisplay`.
            let (self_ty, val_arg) = match inner_type {
                syn::Type::Reference(reference) => {
                    let elem = &*reference.elem;
                    (quote! { #elem }, quote! { *__val })
                }
                _ => (quote! { #inner_type }, quote! { __val }),
            };
            let display_value = if unredacted || field_unredacted {
                quote! { ::std::write!(f, "{}", #val_arg)?; }
            } else {
                quote! { <#self_ty as ::templated_uri::RedactedDisplay>::fmt(#val_arg, redactor, f)?; }
            };

            inner_stmts.push(quote! {
                if let ::core::option::Option::Some(ref __val) = self.#field_ident {
                    #emit_delim
                    #emit_kv
                    #display_value
                    __first = false;
                }
            });
        } else {
            let display_value = if unredacted || field_unredacted {
                quote! { ::std::write!(f, "{}", self.#field_ident)?; }
            } else {
                let field_type = &field.ty;
                quote! { <#field_type as ::templated_uri::RedactedDisplay>::fmt(&self.#field_ident, redactor, f)?; }
            };

            inner_stmts.push(quote! {
                {
                    #emit_delim
                    #emit_kv
                    #display_value
                    __first = false;
                }
            });
        }
    }

    vec![quote! { { #(#inner_stmts)* } }]
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::extract_option_inner;

    #[test]
    fn extract_option_inner_some_for_simple_option() {
        let ty: syn::Type = parse_quote! { Option<u32> };
        let inner = extract_option_inner(&ty).expect("Option<u32> should match");
        let inner_str = quote::quote! { #inner }.to_string();
        assert_eq!(inner_str, "u32");
    }

    #[test]
    fn extract_option_inner_some_for_qualified_option() {
        // Both `std::option::Option<T>` and `core::option::Option<T>` are accepted by
        // matching only on the last path segment.
        let ty: syn::Type = parse_quote! { std::option::Option<String> };
        let inner = extract_option_inner(&ty).expect("qualified Option should match");
        let inner_str = quote::quote! { #inner }.to_string();
        assert_eq!(inner_str, "String");
    }

    #[test]
    fn extract_option_inner_none_for_non_path_type() {
        // `&Option<u32>` is `Type::Reference`, not `Type::Path` — must short-circuit.
        let ty: syn::Type = parse_quote! { &Option<u32> };
        assert!(extract_option_inner(&ty).is_none());
    }

    #[test]
    fn extract_option_inner_none_for_non_option_path() {
        let ty: syn::Type = parse_quote! { Vec<u32> };
        assert!(extract_option_inner(&ty).is_none());
    }

    #[test]
    fn extract_option_inner_none_for_bare_option_without_generics() {
        // Bare `Option` parses with `PathArguments::None`, not `AngleBracketed`.
        let ty: syn::Type = parse_quote! { Option };
        assert!(extract_option_inner(&ty).is_none());
    }

    #[test]
    fn extract_option_inner_none_for_option_with_two_type_args() {
        // Malformed `Option<T, U>` — the length check rejects it.
        let ty: syn::Type = parse_quote! { Option<u32, String> };
        assert!(extract_option_inner(&ty).is_none());
    }

    #[test]
    fn extract_option_inner_none_for_option_with_lifetime_arg() {
        // `Option<'a>` parses but the generic arg is a `Lifetime`, not a `Type`.
        let ty: syn::Type = parse_quote! { Option<'a> };
        assert!(extract_option_inner(&ty).is_none());
    }
}
