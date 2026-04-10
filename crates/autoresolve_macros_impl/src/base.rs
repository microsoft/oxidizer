use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::spanned::Spanned;
use syn::{Fields, Ident, ItemStruct, Type};

/// Classifies fields in a `#[base]` struct.
struct BaseField<'a> {
    ident: &'a Ident,
    ty: &'a Type,
    is_spread: bool,
}

/// Parsed attribute arguments for `#[base]`.
struct BaseAttrs {
    /// The `crate::`-rooted absolute path where the helper module will be accessible.
    helper_path: syn::Path,
    /// If present, this is a scoped base — the path (or bare ident) names the parent base type.
    scoped_parent: Option<syn::Path>,
}

fn parse_attrs(attr: TokenStream) -> syn::Result<BaseAttrs> {
    if attr.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[base] requires `helper_module_exported_as = crate::path::to::helper`",
        ));
    }

    let meta_list: syn::punctuated::Punctuated<syn::Meta, syn::Token![,]> =
        syn::parse::Parser::parse2(syn::punctuated::Punctuated::parse_terminated, attr)?;

    let mut helper_path: Option<syn::Path> = None;
    let mut scoped_parent: Option<syn::Path> = None;

    for meta in &meta_list {
        match meta {
            syn::Meta::NameValue(nv) if nv.path.is_ident("helper_module_exported_as") => {
                let path: syn::Path = match &nv.value {
                    syn::Expr::Path(ep) => syn::parse2(quote::quote! { #ep })?,
                    other => return Err(syn::Error::new_spanned(other, "expected a path")),
                };
                if !is_crate_rooted(&path) {
                    return Err(syn::Error::new_spanned(
                        &path,
                        "`helper_module_exported_as` must be a `crate::`-rooted path",
                    ));
                }
                if path.segments.len() < 2 {
                    return Err(syn::Error::new_spanned(
                        &path,
                        "`helper_module_exported_as` must have at least two segments (e.g., `crate::helper`)",
                    ));
                }
                helper_path = Some(path);
            }
            syn::Meta::List(list) if list.path.is_ident("scoped") => {
                let parent: syn::Path = syn::parse2(list.tokens.clone())?;
                scoped_parent = Some(parent);
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "unexpected attribute; expected `helper_module_exported_as = ...` or `scoped(...)`",
                ));
            }
        }
    }

    let helper_path = helper_path.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[base] requires `helper_module_exported_as = crate::path::to::helper`",
        )
    })?;

    Ok(BaseAttrs {
        helper_path,
        scoped_parent,
    })
}

/// Returns `true` if the path begins with `crate`.
fn is_crate_rooted(path: &syn::Path) -> bool {
    path.segments.first().is_some_and(|s| s.ident == "crate")
}

/// Implements the `#[base]` attribute macro.
///
/// Parses the attributed struct, generates `ResolveFrom` impls for each field,
/// a `BaseType` impl, a declarative helper macro, and module-level re-exports.
pub fn base(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let attrs = parse_attrs(attr)?;
    let the_struct: ItemStruct = syn::parse2(item)?;

    if !the_struct.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &the_struct.generics,
            "#[base] does not support generic structs",
        ));
    }

    let named_fields = match &the_struct.fields {
        Fields::Named(named) => &named.named,
        _ => {
            return Err(syn::Error::new_spanned(&the_struct, "#[base] requires a struct with named fields"));
        }
    };

    if named_fields.is_empty() {
        return Err(syn::Error::new_spanned(&the_struct, "#[base] requires at least one field"));
    }

    let struct_name = &the_struct.ident;

    // Classify fields: #[spread] vs regular.
    let fields: Vec<BaseField<'_>> = named_fields
        .iter()
        .map(|field| {
            let is_spread = field.attrs.iter().any(|a| a.path().is_ident("spread"));
            let ident = field.ident.as_ref().expect("guarded by named-fields check above");
            BaseField {
                ident,
                ty: &field.ty,
                is_spread,
            }
        })
        .collect();

    // Validate: all field types must be paths.
    for field in named_fields.iter() {
        if !matches!(&field.ty, Type::Path(_)) {
            return Err(syn::Error::new_spanned(&field.ty, "#[base] field types must be type paths"));
        }
    }

    // Re-emit the struct, stripping #[spread] attributes from fields.
    let struct_vis = &the_struct.vis;
    let struct_attrs: Vec<_> = the_struct.attrs.iter().collect();
    let clean_fields: Vec<_> = named_fields
        .iter()
        .map(|field| {
            let attrs: Vec<_> = field.attrs.iter().filter(|a| !a.path().is_ident("spread")).collect();
            let vis = &field.vis;
            let ident = &field.ident;
            let ty = &field.ty;
            quote! { #(#attrs)* #vis #ident: #ty }
        })
        .collect();
    let clean_struct = quote! {
        #(#struct_attrs)*
        #struct_vis struct #struct_name {
            #(#clean_fields),*
        }
    };

    match &attrs.scoped_parent {
        None => generate_primary(&attrs, struct_name, &clean_struct, &fields),
        Some(parent) => generate_scoped(&attrs, struct_name, &clean_struct, &fields, parent),
    }
}

/// Constructs the type alias identifier for a regular field: `StructName_PartN_TypeName`.
fn part_alias_name(struct_name: &Ident, index: usize, ty: &Type) -> Ident {
    let type_name = match ty {
        Type::Path(tp) => tp.path.segments.last().expect("guarded by path validation above").ident.to_string(),
        _ => panic!("guarded by path validation above"),
    };
    Ident::new(&format!("{struct_name}_Part{index}_{type_name}"), ty.span())
}

/// Generates type aliases inside the helper module for regular (non-spread) fields.
fn type_aliases(struct_name: &Ident, fields: &[BaseField<'_>]) -> Vec<TokenStream> {
    fields
        .iter()
        .filter(|f| !f.is_spread)
        .enumerate()
        .map(|(i, f)| {
            let alias = part_alias_name(struct_name, i, f.ty);
            let ty = f.ty;
            quote! {
                #[doc(hidden)]
                #[allow(non_camel_case_types)]
                pub type #alias = #ty;
            }
        })
        .collect()
}

/// Returns the last segment of the helper path (the helper module name).
fn helper_mod_name(helper_path: &syn::Path) -> &Ident {
    &helper_path
        .segments
        .last()
        .expect("helper_path validated to have >= 2 segments")
        .ident
}

/// Converts a `crate::`-rooted path to a `$crate::`-rooted token stream for use in macro bodies.
fn to_dollar_crate_path(path: &syn::Path) -> TokenStream {
    let segs: Vec<&Ident> = path.segments.iter().skip(1).map(|s| &s.ident).collect();
    quote! { $crate :: #(#segs)::* }
}

/// Creates a unique mangled name from the full helper path.
///
/// Since `#[macro_export]` macros live at the crate root, we include ALL path
/// segments (except `crate`) joined by `_` to avoid name collisions between
/// helpers that share the same last segment.
fn mangled_name(prefix: &str, helper_path: &syn::Path) -> Ident {
    let joined: String = helper_path
        .segments
        .iter()
        .skip(1)
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("_");
    Ident::new(&format!("{prefix}_{joined}"), Span::call_site())
}

fn generate_primary(
    attrs: &BaseAttrs,
    struct_name: &Ident,
    clean_struct: &TokenStream,
    fields: &[BaseField<'_>],
) -> syn::Result<TokenStream> {
    let helper_path = &attrs.helper_path;
    let helper_mod = helper_mod_name(helper_path);
    let dollar_crate_helper = to_dollar_crate_path(helper_path);

    let aliases = type_aliases(struct_name, fields);

    // Destructure field names for the BaseType impl.
    let field_idents: Vec<_> = fields.iter().map(|f| f.ident).collect();

    // For each `#[spread]` field, re-export the spread macro under a
    // `__spread_`-prefixed alias inside the helper module.  The prefix avoids
    // E0659 ambiguity with the original macro name brought in by `use super::*;`.
    //
    // - Single-segment type (bare ident): `pub use super::Name as __spread_Name;`
    //   requires the caller to `pub use` the spread type into scope.
    // - Qualified path (e.g. `runtime::core::Builtins`): `pub use runtime::core::Builtins as ...;`
    //   works directly because the source path is already public.
    let spread_helper_items: Vec<_> = fields
        .iter()
        .filter(|f| f.is_spread)
        .map(|f| {
            let reexport_src = spread_reexport_path(f.ty);
            let alias = spread_helper_alias(f.ty);
            quote! {
                #[doc(hidden)]
                pub use #reexport_src as #alias;
            }
        })
        .collect();

    // Insertion logic: spread fields delegate to their macro, regular fields use store_value.
    let insert_stmts: Vec<_> = fields
        .iter()
        .map(|f| {
            let ident = f.ident;
            if f.is_spread {
                let path = spread_macro_path(f.ty);
                quote! { #path!(@insert __store, #ident); }
            } else {
                quote! { __store.store_value(#ident); }
            }
        })
        .collect();

    // In the generator macro, `$helper_seg` captures the path segments after
    // `crate::`.  `$dollar` is a metavariable that expands to the literal `$`
    // token, allowing us to emit `$crate`, `$base`, `$store`, `$name` inside
    // the generated macro.
    let impls_body = impls_body_for_generator(struct_name, fields);
    let insert_body = insert_body_for_generator(fields);

    // Canary: validate that type aliases resolve correctly.
    let canaries: Vec<_> = fields
        .iter()
        .filter(|f| !f.is_spread)
        .enumerate()
        .map(|(i, f)| {
            let alias = part_alias_name(struct_name, i, f.ty);
            quote! { _ = std::mem::size_of::<#helper_mod::#alias>(); }
        })
        .collect();

    // Selective re-exports for the `@reexport` arm: everything EXCEPT the
    // struct-name macro, to avoid an ambiguity between the glob-imported
    // original macro and the newly generated one.
    let reexport_items = reexport_item_list(struct_name, fields);

    let mangled_macro_name = mangled_name("__autoresolve", helper_path);
    let mangled_gen_name = mangled_name("__autoresolve_gen", helper_path);

    Ok(quote! {
        #clean_struct

        #[doc(hidden)]
        pub mod #helper_mod {
            use super::*;

            #(#aliases)*

            #(#spread_helper_items)*

            // Generator macro: takes a `crate::`-rooted helper path in brackets
            // and emits the final macro with `$crate::` path segments baked in.
            // The pattern `[crate $(:: $helper_seg:ident)*]` strips the leading
            // `crate` keyword; the generated macro uses `$crate $(:: $helper_seg)*`
            // so the path resolves to the *defining* crate — crucial for cross-crate
            // usage where `crate::` would incorrectly resolve to the expansion site.
            #[doc(hidden)]
            #[macro_export]
            macro_rules! #mangled_gen_name {
                ([crate $(:: $helper_seg:ident)*], $mangled:ident, $dollar:tt) => {
                    #[doc(hidden)]
                    #[macro_export]
                    macro_rules! $mangled {
                        (@ impls $dollar base:path) => {
                            #impls_body
                        };
                        (@ insert $dollar store:ident, $dollar name:ident) => {
                            #insert_body
                        };
                        (@ reexport [$dollar ($dollar new_helper:tt)*], $dollar new_mangled:ident, $dollar dd:tt) => {
                            #(#[allow(unused_imports)] pub use #dollar_crate_helper :: #reexport_items;)*
                            #dollar_crate_helper :: __generator!([$dollar ($dollar new_helper)*], $dollar new_mangled, $dollar dd);
                        };
                    }
                };
            }
            #[doc(hidden)]
            pub use #mangled_gen_name as __generator;

            // Invoke the generator with the original helper path to produce the
            // final macro.  The generator strips `crate` and uses `$crate` so the
            // generated macro resolves paths cross-crate.
            __generator!([#helper_path], #mangled_macro_name, $);
            #[doc(hidden)]
            pub use #mangled_macro_name as #struct_name;
        }
        #[doc(hidden)]
        pub use #helper_mod :: #struct_name;

        // Self-invoke the macro to generate ResolveFrom<Self> impls for all
        // types (own + spread). Uses the `pub use` alias which routes through
        // the helper module path, avoiding the
        // `macro_expanded_macro_exports_accessed_by_absolute_paths` lint.
        #struct_name!(@impls #struct_name);

        impl ::autoresolve::BaseType for #struct_name {
            type Parent = ();

            fn insert_into(self, __store: &mut impl ::autoresolve::ResolverStore<#struct_name>) {
                let Self { #(#field_idents),* } = self;
                #(#insert_stmts)*
            }
        }

        const _: fn() = || {
            #(#canaries)*
        };
    })
}

/// Builds the `@impls` body for the generator pattern.
///
/// Uses `$dollar crate $(:: $helper_seg)*` to produce `$crate::path::to::helper`
/// in the generated macro.
///
/// - `#[spread]` fields: invokes `$crate::helper::__spread_MacroName!(@impls $base)`.
///   The helper module contains `pub use super::MacroName as __spread_MacroName;`,
///   which resolves both same-crate and cross-crate.
/// - Regular fields: emits `ResolveFrom` impl using `$crate::helper::AliasName`
fn impls_body_for_generator(struct_name: &Ident, fields: &[BaseField<'_>]) -> TokenStream {
    let mut regular_idx = 0usize;
    let stmts: Vec<_> = fields
        .iter()
        .map(|f| {
            if f.is_spread {
                let alias = spread_helper_alias(f.ty);
                quote! { $dollar crate $(:: $helper_seg)* :: #alias!(@ impls $dollar base); }
            } else {
                let alias = part_alias_name(struct_name, regular_idx, f.ty);
                regular_idx += 1;
                quote! {
                    impl ::autoresolve::ResolveFrom<$dollar base> for $dollar crate $(:: $helper_seg)* :: #alias {
                        type Inputs = ::autoresolve::ResolutionDepsEnd;

                        fn new(_: ::autoresolve::ResolutionDepsEnd) -> Self {
                            unreachable!("root types are pre-inserted into the resolver")
                        }
                    }
                }
            }
        })
        .collect();

    quote! { #(#stmts)* }
}

/// Builds the body of the `@insert` macro arm for the generator pattern.
///
/// - `#[spread]` fields: delegates to `$crate::helper::__spread_MacroName!(@insert ...)`
/// - Regular fields: calls `$dollar store.store_value($dollar name.field)`
fn insert_body_for_generator(fields: &[BaseField<'_>]) -> TokenStream {
    let stmts: Vec<_> = fields
        .iter()
        .map(|f| {
            let ident = f.ident;
            if f.is_spread {
                let alias = spread_helper_alias(f.ty);
                quote! {
                    {
                        let __spread = $dollar name.#ident;
                        $dollar crate $(:: $helper_seg)* :: #alias!(@ insert $dollar store, __spread);
                    }
                }
            } else {
                quote! { $dollar store.store_value($dollar name.#ident); }
            }
        })
        .collect();

    quote! { #(#stmts)* }
}

/// Builds the list of items the `@reexport` arm should selectively re-export
/// from the original helper module into the new helper module.
///
/// Includes type aliases, spread field macros, and `__generator`, but
/// deliberately **excludes** the struct-name macro (e.g. `Builtins`).  This
/// prevents an ambiguity between the glob-imported original macro and the new
/// macro generated by the `@reexport` invocation.
fn reexport_item_list(struct_name: &Ident, fields: &[BaseField<'_>]) -> Vec<Ident> {
    let mut items = Vec::new();

    // Type aliases for regular (non-spread) fields and `__spread_`-prefixed
    // aliases for spread field forwarding macros.
    let mut regular_idx = 0usize;
    for f in fields {
        if f.is_spread {
            // Re-export the `__spread_`-prefixed forwarding alias.
            items.push(spread_helper_alias(f.ty));
        } else {
            items.push(part_alias_name(struct_name, regular_idx, f.ty));
            regular_idx += 1;
        }
    }

    // The `__generator` alias so the new helper can invoke it.
    items.push(Ident::new("__generator", Span::call_site()));

    items
}

/// Extracts the macro name (last path segment) for a `#[spread]` field's type.
fn spread_macro_ident(ty: &Type) -> &Ident {
    match ty {
        Type::Path(tp) => &tp.path.segments.last().expect("guarded by path validation above").ident,
        _ => panic!("guarded by path validation above"),
    }
}

/// Returns the full path for a `#[spread]` field's type, for use in macro calls.
///
/// Handles both bare `Builtins` and qualified `runtime::core::Builtins`.
fn spread_macro_path(ty: &Type) -> &syn::Path {
    match ty {
        Type::Path(tp) => &tp.path,
        _ => panic!("guarded by path validation above"),
    }
}

/// Returns the `__spread_`-prefixed alias name used inside the helper module
/// for a spread field's forwarding macro.
///
/// Using a `__spread_` prefix avoids E0659 ambiguity with the original macro
/// name brought in by `use super::*;`.
fn spread_helper_alias(ty: &Type) -> Ident {
    let macro_name = spread_macro_ident(ty);
    Ident::new(&format!("__spread_{macro_name}"), macro_name.span())
}

/// Returns the source path for `pub use ... as __spread_MacroName;` inside
/// the helper module.
///
/// - Single-segment type (bare ident, e.g. `Builtins`) → `super::Builtins`
///   (resolved through the caller's `use` import via `use super::*;`)
/// - Multi-segment type (qualified path, e.g. `runtime::core::Builtins`) →
///   `runtime::core::Builtins` (the full path, used as-is)
fn spread_reexport_path(ty: &Type) -> TokenStream {
    match ty {
        Type::Path(tp) if tp.path.segments.len() == 1 => {
            let macro_name = spread_macro_ident(ty);
            quote! { super :: #macro_name }
        }
        Type::Path(tp) => {
            let path = &tp.path;
            quote! { #path }
        }
        _ => panic!("guarded by path validation above"),
    }
}

fn generate_scoped(
    attrs: &BaseAttrs,
    struct_name: &Ident,
    clean_struct: &TokenStream,
    fields: &[BaseField<'_>],
    parent: &syn::Path,
) -> syn::Result<TokenStream> {
    let helper_path = &attrs.helper_path;
    let helper_mod = helper_mod_name(helper_path);
    let dollar_crate_helper = to_dollar_crate_path(helper_path);

    // Extract the last segment (struct name) from the parent path for macro invocation.
    let parent_ident = &parent
        .segments
        .last()
        .expect("scoped parent path parsed by syn always has >= 1 segment")
        .ident;

    // For the `pub use` inside the helper module:
    // - bare ident `Base` → `pub use super::Base;`
    // - qualified path `crate::Base` → `pub use crate::Base;`
    let parent_reexport = if parent.segments.len() == 1 {
        quote! { super::#parent }
    } else {
        quote! { #parent }
    };

    let aliases = type_aliases(struct_name, fields);

    let field_idents: Vec<_> = fields.iter().map(|f| f.ident).collect();

    // Spread re-exports (same rationale as generate_primary).
    let spread_helper_items: Vec<_> = fields
        .iter()
        .filter(|f| f.is_spread)
        .map(|f| {
            let reexport_src = spread_reexport_path(f.ty);
            let alias = spread_helper_alias(f.ty);
            quote! {
                #[doc(hidden)]
                pub use #reexport_src as #alias;
            }
        })
        .collect();

    // Insertion logic.
    let insert_stmts: Vec<_> = fields
        .iter()
        .map(|f| {
            let ident = f.ident;
            if f.is_spread {
                let path = spread_macro_path(f.ty);
                quote! { #path!(@insert __store, #ident); }
            } else {
                quote! { __store.store_value(#ident); }
            }
        })
        .collect();

    let own_impls_body = impls_body_for_generator(struct_name, fields);
    let own_insert_body = insert_body_for_generator(fields);

    let canaries: Vec<_> = fields
        .iter()
        .filter(|f| !f.is_spread)
        .enumerate()
        .map(|(i, f)| {
            let alias = part_alias_name(struct_name, i, f.ty);
            quote! { _ = std::mem::size_of::<#helper_mod::#alias>(); }
        })
        .collect();

    // Selective re-exports for the `@reexport` arm.
    let mut reexport_items = reexport_item_list(struct_name, fields);
    // Also re-export the parent macro so delegated `@impls` calls work.
    reexport_items.push(parent_ident.clone());

    let mangled_macro_name = mangled_name("__autoresolve", helper_path);
    let mangled_gen_name = mangled_name("__autoresolve_gen", helper_path);

    Ok(quote! {
        #clean_struct

        #[doc(hidden)]
        pub mod #helper_mod {
            use super::*;

            // Re-export the parent macro into this helper module so that
            // `$crate::helper::#parent_ident!()` works without an absolute path to the
            // parent, avoiding the `macro_expanded_macro_exports_accessed_by_absolute_paths` lint.
            //
            // Bare ident: `pub use super::Parent;` (resolves via `use super::*;`)
            // Qualified path: `pub use crate::path::Parent;` (used as-is)
            pub use #parent_reexport;

            #(#aliases)*

            #(#spread_helper_items)*

            // Generator macro for the scoped base. The `@impls` arm delegates
            // to the parent's macro (routed through `$crate::helper`) and then
            // emits its own impls.
            #[doc(hidden)]
            #[macro_export]
            macro_rules! #mangled_gen_name {
                ([crate $(:: $helper_seg:ident)*], $mangled:ident, $dollar:tt) => {
                    #[doc(hidden)]
                    #[macro_export]
                    macro_rules! $mangled {
                        (@ impls $dollar base:path) => {
                            $dollar crate $(:: $helper_seg)* :: #parent_ident!(@ impls $dollar base);
                            #own_impls_body
                        };
                        (@ insert $dollar store:ident, $dollar name:ident) => {
                            #own_insert_body
                        };
                        (@ reexport [$dollar ($dollar new_helper:tt)*], $dollar new_mangled:ident, $dollar dd:tt) => {
                            #(#[allow(unused_imports)] pub use #dollar_crate_helper :: #reexport_items;)*
                            #dollar_crate_helper :: __generator!([$dollar ($dollar new_helper)*], $dollar new_mangled, $dollar dd);
                        };
                    }
                };
            }
            #[doc(hidden)]
            pub use #mangled_gen_name as __generator;

            // Invoke the generator with the original helper path to produce the
            // final macro.
            __generator!([#helper_path], #mangled_macro_name, $);
            #[doc(hidden)]
            pub use #mangled_macro_name as #struct_name;
        }
        #[doc(hidden)]
        pub use #helper_mod :: #struct_name;

        // Self-invocation: propagate parent + own types into this scope.
        #struct_name!(@impls #struct_name);

        impl ::autoresolve::BaseType for #struct_name {
            type Parent = #parent;

            fn insert_into(self, __store: &mut impl ::autoresolve::ResolverStore<#struct_name>) {
                let Self { #(#field_idents),* } = self;
                #(#insert_stmts)*
            }
        }

        const _: fn() = || {
            #(#canaries)*
        };
    })
}

/// Implements the `#[reexport_base]` attribute macro.
///
/// Generates a `pub type` alias and a new helper module that delegates to the
/// original base's helper macro, enabling re-export from a different module path.
pub fn reexport_base(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    // Parse the attribute: `helper_module_exported_as = crate::new::helper::path`
    let meta_list: syn::punctuated::Punctuated<syn::Meta, syn::Token![,]> =
        syn::parse::Parser::parse2(syn::punctuated::Punctuated::parse_terminated, attr)?;

    let mut new_helper_path: Option<syn::Path> = None;

    for meta in &meta_list {
        match meta {
            syn::Meta::NameValue(nv) if nv.path.is_ident("helper_module_exported_as") => {
                let path: syn::Path = match &nv.value {
                    syn::Expr::Path(ep) => syn::parse2(quote::quote! { #ep })?,
                    other => return Err(syn::Error::new_spanned(other, "expected a path")),
                };
                if !is_crate_rooted(&path) {
                    return Err(syn::Error::new_spanned(
                        &path,
                        "`helper_module_exported_as` must be a `crate::`-rooted path",
                    ));
                }
                if path.segments.len() < 2 {
                    return Err(syn::Error::new_spanned(
                        &path,
                        "`helper_module_exported_as` must have at least two segments",
                    ));
                }
                new_helper_path = Some(path);
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "unexpected attribute; expected `helper_module_exported_as = ...`",
                ));
            }
        }
    }

    let new_helper_path = new_helper_path.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "#[reexport_base] requires `helper_module_exported_as = crate::path::to::helper`",
        )
    })?;

    // Parse the item: `pub type Foo = path::to::Original;`
    let type_alias: syn::ItemType = syn::parse2(item)?;
    let struct_name = &type_alias.ident;
    let vis = &type_alias.vis;

    // Extract the original type path from the alias.
    let original_path = match type_alias.ty.as_ref() {
        syn::Type::Path(tp) => &tp.path,
        other => {
            return Err(syn::Error::new_spanned(other, "#[reexport_base] target must be a type path"));
        }
    };

    let new_helper_mod = helper_mod_name(&new_helper_path);

    // Build a path to reach the original struct/macro from within the new helper
    // module.  The helper module is one level deeper than the re-export site, so
    // relative paths need an extra `super::`.
    let original_in_helper = if is_crate_rooted(original_path) {
        quote! { #original_path }
    } else {
        quote! { super::#original_path }
    };

    let mangled_macro_name = mangled_name("__autoresolve", &new_helper_path);

    Ok(quote! {
        #vis type #struct_name = #original_path;

        #[doc(hidden)]
        pub mod #new_helper_mod {
            // NB: no `use super::*;` here.  The `@reexport` arm selectively
            // re-imports type aliases and the `__generator` macro from the
            // original helper — but NOT the struct-name macro, preventing an
            // E0659 ambiguity between a glob-imported name and the new macro.

            // Use the original macro's @reexport arm to:
            // 1. Selectively re-export type aliases and macros from the original helper
            // 2. Generate a new macro at this helper path via the generator
            //
            // Passes `crate::`-rooted path; the generator strips `crate` and uses
            // `$crate` so the generated macro resolves paths cross-crate.
            #original_in_helper!(@reexport [#new_helper_path], #mangled_macro_name, $);

            // Shadow the glob-imported struct macro with the newly generated one.
            #[doc(hidden)]
            pub use #mangled_macro_name as #struct_name;
        }
        #[doc(hidden)]
        #vis use #new_helper_mod :: #struct_name;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pretty_print(tokens: TokenStream) -> String {
        let file = syn::parse2::<syn::File>(tokens).unwrap_or_else(|e| panic!("Failed to parse generated code: {e}"));
        prettyplease::unparse(&file)
    }

    /// Snapshot: primary base with one `#[spread]` and one regular field.
    #[test]
    fn primary_base_with_spread_and_regular() {
        let attr = quote! { helper_module_exported_as = crate::app_base_helper };
        let input = quote! {
            struct Base {
                #[spread]
                builtins: Builtins,
                telemetry: Telemetry,
            }
        };
        let result = base(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    /// Snapshot: primary base with all regular (non-spread) fields.
    #[test]
    fn primary_base_all_regular() {
        let attr = quote! { helper_module_exported_as = crate::base_helper };
        let input = quote! {
            struct Base {
                scheduler: Scheduler,
                clock: Clock,
            }
        };
        let result = base(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    /// Snapshot: scoped base with a single field.
    #[test]
    fn scoped_base() {
        let attr = quote! { scoped(Base), helper_module_exported_as = crate::scoped_helper };
        let input = quote! {
            struct ScopedRoots {
                request_context: RequestContext,
            }
        };
        let result = base(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    /// Rejects a struct with no `helper_module_exported_as` attribute.
    #[test]
    fn error_on_missing_helper() {
        let attr = TokenStream::new();
        let input = quote! {
            struct Base {
                field: Field,
            }
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }

    /// Rejects a generic struct.
    #[test]
    fn error_on_generics() {
        let attr = quote! { helper_module_exported_as = crate::base_helper };
        let input = quote! {
            struct Base<T> {
                field: T,
            }
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }

    /// Rejects a struct with no fields.
    #[test]
    fn error_on_empty_struct() {
        let attr = quote! { helper_module_exported_as = crate::base_helper };
        let input = quote! {
            struct Base {}
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }

    /// Snapshot: scoped base with a `#[spread]` field.
    #[test]
    fn scoped_base_with_spread() {
        let attr = quote! { scoped(Base), helper_module_exported_as = crate::scoped_helper };
        let input = quote! {
            struct ScopedRoots {
                #[spread]
                builtins: Builtins,
                request_context: RequestContext,
            }
        };
        let result = base(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    /// Rejects an unrecognized attribute key.
    #[test]
    fn error_on_invalid_attribute() {
        let attr = quote! { something_wrong };
        let input = quote! {
            struct Base {
                field: Field,
            }
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }

    /// Rejects a `helper_module_exported_as` path not rooted with `crate::`.
    #[test]
    fn error_on_non_crate_rooted_helper() {
        let attr = quote! { helper_module_exported_as = super::base_helper };
        let input = quote! {
            struct Base {
                field: Field,
            }
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }

    /// Snapshot: scoped base with a fully-qualified `crate::Base` parent path.
    #[test]
    fn scoped_base_with_path() {
        let attr = quote! { scoped(crate::Base), helper_module_exported_as = crate::scoped_helper };
        let input = quote! {
            struct ScopedRoots {
                request_context: RequestContext,
            }
        };
        let result = base(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    /// Snapshot: `#[reexport_base]` generating a type alias and delegating helper module.
    #[test]
    fn reexport_base_snapshot() {
        let attr = quote! { helper_module_exported_as = crate::runtime::exports::builtins_helper };
        let input = quote! {
            pub type Builtins = super::internal::Builtins;
        };
        let result = reexport_base(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }
}
