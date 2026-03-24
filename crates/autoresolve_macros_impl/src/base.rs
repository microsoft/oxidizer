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

    // Insertion logic: spread fields delegate to their macro, regular fields use store_value.
    let insert_stmts: Vec<_> = fields
        .iter()
        .map(|f| {
            let ident = f.ident;
            if f.is_spread {
                let macro_name = spread_macro_ident(f.ty);
                quote! { #macro_name!(@insert __store, #ident); }
            } else {
                quote! { __store.store_value(#ident); }
            }
        })
        .collect();

    let impls_body = impls_body_for(struct_name, fields, &dollar_crate_helper);
    let insert_body = insert_body_for(fields, &dollar_crate_helper);

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

    let mangled_macro_name = Ident::new(&format!("__autoresolve_{helper_mod}"), Span::call_site());

    Ok(quote! {
        #clean_struct

        #[doc(hidden)]
        pub mod #helper_mod {
            use super::*;

            #(#aliases)*

            #[doc(hidden)]
            #[macro_export]
            macro_rules! #mangled_macro_name {
                (@impls $base:path) => {
                    #impls_body
                };
                (@insert $store:ident, $name:ident) => {
                    #insert_body
                };
            }
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

        impl ::autoresolve::BaseType<#struct_name> for #struct_name {
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

/// Builds the body of the `@impls` macro arm for the given fields.
///
/// - `#[spread]` fields: invokes `SpreadType!(@impls $base)` (the macro is in scope by name)
/// - Regular fields: emits `ResolveFrom<$base>` impl using `$crate::helper::AliasName`
fn impls_body_for(struct_name: &Ident, fields: &[BaseField<'_>], dollar_crate_helper: &TokenStream) -> TokenStream {
    let mut regular_idx = 0usize;
    let stmts: Vec<_> = fields
        .iter()
        .map(|f| {
            if f.is_spread {
                let macro_name = spread_macro_ident(f.ty);
                quote! { #macro_name!(@impls $base); }
            } else {
                let alias = part_alias_name(struct_name, regular_idx, f.ty);
                regular_idx += 1;
                quote! {
                    impl ::autoresolve::ResolveFrom<$base> for #dollar_crate_helper :: #alias {
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

/// Builds the body of the `@insert` macro arm for the given fields.
///
/// - `#[spread]` fields: delegates to `SpreadType!(@insert $store, field_name)`
/// - Regular fields: calls `$store.store_value($name.field)`
fn insert_body_for(fields: &[BaseField<'_>], _dollar_crate_helper: &TokenStream) -> TokenStream {
    let stmts: Vec<_> = fields
        .iter()
        .map(|f| {
            let ident = f.ident;
            if f.is_spread {
                let macro_name = spread_macro_ident(f.ty);
                quote! {
                    {
                        let __spread = $name.#ident;
                        #macro_name!(@insert $store, __spread);
                    }
                }
            } else {
                quote! { $store.store_value($name.#ident); }
            }
        })
        .collect();

    quote! { #(#stmts)* }
}

/// Extracts the macro name (last path segment) for a `#[spread]` field's type.
fn spread_macro_ident(ty: &Type) -> &Ident {
    match ty {
        Type::Path(tp) => &tp.path.segments.last().expect("guarded by path validation above").ident,
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

    // Insertion logic.
    let insert_stmts: Vec<_> = fields
        .iter()
        .map(|f| {
            let ident = f.ident;
            if f.is_spread {
                let macro_name = spread_macro_ident(f.ty);
                quote! { #macro_name!(@insert __store, #ident); }
            } else {
                quote! { __store.store_value(#ident); }
            }
        })
        .collect();

    let own_impls_body = impls_body_for(struct_name, fields, &dollar_crate_helper);
    let own_insert_body = insert_body_for(fields, &dollar_crate_helper);

    let canaries: Vec<_> = fields
        .iter()
        .filter(|f| !f.is_spread)
        .enumerate()
        .map(|(i, f)| {
            let alias = part_alias_name(struct_name, i, f.ty);
            quote! { _ = std::mem::size_of::<#helper_mod::#alias>(); }
        })
        .collect();

    let mangled_macro_name = Ident::new(&format!("__autoresolve_{helper_mod}"), Span::call_site());

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

            #[doc(hidden)]
            #[macro_export]
            macro_rules! #mangled_macro_name {
                (@impls $base:path) => {
                    #dollar_crate_helper :: #parent_ident!(@impls $base);
                    #own_impls_body
                };
                (@insert $store:ident, $name:ident) => {
                    #own_insert_body
                };
            }
            #[doc(hidden)]
            pub use #mangled_macro_name as #struct_name;
        }
        #[doc(hidden)]
        pub use #helper_mod :: #struct_name;

        impl ::autoresolve::ScopedUnder for #struct_name {
            type Parent = #parent;
        }

        // Self-invocation: propagate parent + own types into this scope.
        #struct_name!(@impls #struct_name);

        impl ::autoresolve::BaseType<#struct_name> for #struct_name {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pretty_print(tokens: TokenStream) -> String {
        let file = syn::parse2::<syn::File>(tokens).unwrap_or_else(|e| panic!("Failed to parse generated code: {e}"));
        prettyplease::unparse(&file)
    }

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

    #[test]
    fn error_on_empty_struct() {
        let attr = quote! { helper_module_exported_as = crate::base_helper };
        let input = quote! {
            struct Base {}
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }

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
}
