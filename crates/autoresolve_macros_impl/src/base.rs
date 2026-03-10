use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::spanned::Spanned;
use syn::{Fields, Ident, Item, ItemMod, ItemStruct, Type};

/// Classifies fields in a `#[base]` struct.
struct BaseField<'a> {
    ident: &'a Ident,
    ty: &'a Type,
    is_spread: bool,
}

/// Parsed attribute argument for `#[base]` / `#[base(scoped(ParentBase))]`.
enum BaseMode {
    /// Primary base type: generates `BaseType` impl + `ResolveFrom` impls + insertion logic.
    Primary,
    /// Scoped root declaration: generates `ResolveFrom<Parent>` impls only.
    Scoped(syn::Path),
}

fn parse_mode(attr: TokenStream) -> syn::Result<BaseMode> {
    if attr.is_empty() {
        return Ok(BaseMode::Primary);
    }

    // Expect: `scoped(Path)`
    let tokens: Vec<proc_macro2::TokenTree> = attr.into_iter().collect();
    if tokens.len() != 2 {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[base] attribute must be empty or `scoped(BaseType)`",
        ));
    }

    let keyword = match &tokens[0] {
        proc_macro2::TokenTree::Ident(id) => id,
        other => {
            return Err(syn::Error::new_spanned(other, "expected `scoped`"));
        }
    };

    if keyword != "scoped" {
        return Err(syn::Error::new_spanned(keyword, "expected `scoped`"));
    }

    let group = match &tokens[1] {
        proc_macro2::TokenTree::Group(g) if g.delimiter() == proc_macro2::Delimiter::Parenthesis => g,
        other => {
            return Err(syn::Error::new_spanned(other, "expected `(BaseType)` after `scoped`"));
        }
    };

    let parent: syn::Path = syn::parse2(group.stream())?;

    if parent.segments.len() < 2 {
        return Err(syn::Error::new_spanned(
            &parent,
            "scoped parent must be a module-qualified path (e.g., `parent_mod::ParentType`)",
        ));
    }

    Ok(BaseMode::Scoped(parent))
}

pub fn base(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let mode = parse_mode(attr)?;

    let item_mod: ItemMod = syn::parse2(item)?;

    let (_, items) = item_mod
        .content
        .as_ref()
        .ok_or_else(|| syn::Error::new_spanned(&item_mod, "#[base] requires a module with a body (not `mod name;`)"))?;

    // Find exactly one struct with named fields inside the module.
    let structs: Vec<&ItemStruct> = items
        .iter()
        .filter_map(|item| if let Item::Struct(s) = item { Some(s) } else { None })
        .filter(|s| matches!(s.fields, Fields::Named(_)))
        .collect();

    let the_struct = match structs.len() {
        1 => structs[0],
        0 => {
            return Err(syn::Error::new_spanned(
                &item_mod,
                "#[base] module must contain a struct with named fields",
            ));
        }
        _ => {
            return Err(syn::Error::new_spanned(
                &item_mod,
                "#[base] module must contain exactly one struct with named fields",
            ));
        }
    };

    if !the_struct.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &the_struct.generics,
            "#[base] does not support generic structs",
        ));
    }

    let named_fields = match &the_struct.fields {
        Fields::Named(named) => &named.named,
        _ => unreachable!("guarded by filter above"),
    };

    if named_fields.is_empty() {
        return Err(syn::Error::new_spanned(the_struct, "#[base] requires at least one field"));
    }

    let struct_name = &the_struct.ident;
    let mod_ident = &item_mod.ident;
    let mod_vis = &item_mod.vis;
    let mod_attrs: Vec<_> = item_mod.attrs.iter().collect();

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

    // Validate: #[spread] fields must be module-qualified paths without a qualified self type.
    for f in &fields {
        if f.is_spread {
            if let Type::Path(tp) = f.ty {
                if tp.qself.is_some() {
                    return Err(syn::Error::new_spanned(
                        f.ty,
                        "#[spread] field type must be a simple path (the base type name)",
                    ));
                }
                if tp.path.segments.len() < 2 {
                    return Err(syn::Error::new_spanned(
                        f.ty,
                        "#[spread] field type must be a module-qualified path (e.g., `module::Type`)",
                    ));
                }
            }
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

    // Other items in the module (everything except the struct we found).
    let other_items: Vec<_> = items
        .iter()
        .filter(|item| !matches!(item, Item::Struct(s) if s.ident == *struct_name))
        .collect();

    match mode {
        BaseMode::Primary => generate_primary(mod_ident, mod_vis, &mod_attrs, &other_items, struct_name, &clean_struct, &fields),
        BaseMode::Scoped(parent) => generate_scoped(
            mod_ident,
            mod_vis,
            &mod_attrs,
            &other_items,
            struct_name,
            &clean_struct,
            &fields,
            &parent,
        ),
    }
}

/// Extracts the macro name (last path segment) for a `#[spread]` field's type.
fn spread_macro_ident(ty: &Type) -> &Ident {
    match ty {
        Type::Path(tp) => &tp.path.segments.last().expect("guarded by path validation above").ident,
        _ => panic!("guarded by path validation above"),
    }
}

/// Generates hidden `__PartN` re-exports for regular (non-spread) fields so that the
/// generated `macro_rules!` arms can reference types via `$crate::mod::__PartN`.
fn regular_reexports(fields: &[BaseField<'_>]) -> Vec<TokenStream> {
    fields
        .iter()
        .filter(|f| !f.is_spread)
        .enumerate()
        .map(|(i, f)| {
            let field_ty = f.ty;
            let part_name = Ident::new(&format!("__Part{i}"), field_ty.span());
            quote! {
                #[doc(hidden)]
                pub(crate) use #field_ty as #part_name;
            }
        })
        .collect()
}

/// Generates friendly re-exports for all field types using their last path segment,
/// so consumers who glob-import the module get the field types available.
fn friendly_reexports(fields: &[BaseField<'_>]) -> Vec<TokenStream> {
    fields
        .iter()
        .filter_map(|f| {
            if let Type::Path(tp) = f.ty {
                let last_seg = &tp.path.segments.last()?.ident;
                Some(quote! {
                    #[allow(unused_imports)]
                    pub(crate) use #tp as #last_seg;
                })
            } else {
                None
            }
        })
        .collect()
}

/// Generates canary references that validate the module path is correct at the definition site.
fn canary_refs(fields: &[BaseField<'_>], mod_ident: &Ident) -> Vec<TokenStream> {
    fields
        .iter()
        .filter(|f| !f.is_spread)
        .enumerate()
        .map(|(i, f)| {
            let part_name = Ident::new(&format!("__Part{i}"), f.ty.span());
            quote! { _ = std::mem::size_of::<#mod_ident::#part_name>(); }
        })
        .collect()
}

/// Extracts the module path from a qualified type path by stripping the last segment.
/// For example, `super::builtins::Builtins` yields `super :: builtins`.
fn strip_last_segment(path: &syn::Path) -> TokenStream {
    let segs: Vec<&Ident> = path.segments.iter().map(|s| &s.ident).collect();
    let module_segs = &segs[..segs.len() - 1];
    quote! { #(#module_segs)::* }
}

/// Generates `__spread{N}` re-exports for `#[spread]` fields, pointing to each spread's module.
fn spread_reexports(fields: &[BaseField<'_>]) -> Vec<TokenStream> {
    fields
        .iter()
        .filter(|f| f.is_spread)
        .enumerate()
        .map(|(i, f)| {
            let module_path = match f.ty {
                Type::Path(tp) => strip_last_segment(&tp.path),
                _ => panic!("guarded by path validation above"),
            };
            let alias = Ident::new(&format!("__spread{i}"), f.ty.span());
            quote! {
                #[doc(hidden)]
                pub(crate) use #module_path as #alias;
            }
        })
        .collect()
}

/// Computes the parent module path as seen from **inside** the generated module.
/// Prepends `super::` for relative paths since the generated module is one level deeper.
fn parent_module_path_for_inner(parent: &syn::Path) -> TokenStream {
    let segs: Vec<&Ident> = parent.segments.iter().map(|s| &s.ident).collect();
    let module_segs = &segs[..segs.len() - 1];
    if module_segs.first().is_some_and(|s| *s == "crate") {
        quote! { #(#module_segs)::* }
    } else {
        quote! { super :: #(#module_segs)::* }
    }
}

/// Computes the parent module path as seen from the **outer** scope (definition site).
fn parent_module_path_for_outer(parent: &syn::Path) -> TokenStream {
    strip_last_segment(parent)
}

fn generate_primary(
    mod_ident: &Ident,
    mod_vis: &syn::Visibility,
    mod_attrs: &[&syn::Attribute],
    other_items: &[&Item],
    struct_name: &Ident,
    clean_struct: &TokenStream,
    fields: &[BaseField<'_>],
) -> syn::Result<TokenStream> {
    let reexports = regular_reexports(fields);
    let friendly = friendly_reexports(fields);
    let spread_re = spread_reexports(fields);

    // For regular fields: generate ResolveFrom<Self> impls (inside the module, no macro calls).
    let regular_impls: Vec<_> = fields
        .iter()
        .filter(|f| !f.is_spread)
        .map(|f| {
            let ty = f.ty;
            quote! {
                impl ::autoresolve::ResolveFrom<#struct_name> for #ty {
                    type Inputs = ::autoresolve::ResolutionDepsEnd;

                    fn new(_: ::autoresolve::ResolutionDepsEnd) -> Self {
                        unreachable!("base types are pre-inserted into the resolver")
                    }
                }
            }
        })
        .collect();

    // Definition-site: invoke spread @impls arms using module-path-based calls.
    let qualified_name = quote! { #mod_ident::#struct_name };
    let spread_impls: Vec<_> = fields
        .iter()
        .filter(|f| f.is_spread)
        .enumerate()
        .map(|(i, f)| {
            let macro_name = spread_macro_ident(f.ty);
            let spread_alias = Ident::new(&format!("__spread{i}"), Span::call_site());
            quote! { #mod_ident::#spread_alias::#macro_name!(@impls #qualified_name, #mod_ident::#spread_alias); }
        })
        .collect();

    // Destructure field names for the BaseType impl.
    let field_idents: Vec<_> = fields.iter().map(|f| f.ident).collect();

    // Insertion logic: spread fields use path-based @insert macro arm, regular fields use store_value.
    let mut spread_insert_idx = 0usize;
    let insert_stmts: Vec<_> = fields
        .iter()
        .map(|f| {
            let ident = f.ident;
            if f.is_spread {
                let macro_name = spread_macro_ident(f.ty);
                let spread_alias = Ident::new(&format!("__spread{spread_insert_idx}"), Span::call_site());
                spread_insert_idx += 1;
                quote! { #mod_ident::#spread_alias::#macro_name!(@insert __store, #ident, #mod_ident::#spread_alias); }
            } else {
                quote! { __store.store_value(#ident); }
            }
        })
        .collect();

    let impls_body = impls_body_for(fields);
    let insert_body = insert_body_for(fields);
    let canaries = canary_refs(fields, mod_ident);
    let mangled_macro_name = Ident::new(&format!("__autoresolve_{mod_ident}"), Span::call_site());

    Ok(quote! {
        #(#mod_attrs)*
        #mod_vis mod #mod_ident {
            #(#other_items)*

            #clean_struct

            #(#reexports)*

            #(#friendly)*

            #(#regular_impls)*

            #(#spread_re)*

            #[doc(hidden)]
            #[macro_export]
            macro_rules! #mangled_macro_name {
                (@impls $base:path, $($self_mod:tt)*) => {
                    #impls_body
                };
                (@insert $store:ident, $name:ident, $($self_mod:tt)*) => {
                    #insert_body
                };
            }
            #[doc(hidden)]
            pub use #mangled_macro_name as #struct_name;
        }

        // Definition-site: spread fields propagate their types into this base.
        #(#spread_impls)*

        impl ::autoresolve::BaseType<#mod_ident::#struct_name> for #mod_ident::#struct_name {
            fn insert_into(self, __store: &mut impl ::autoresolve::ResolverStore<#mod_ident::#struct_name>) {
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
/// - `#[spread]` fields: invokes `$($self_mod)*::__spread{N}::MacroName!(@impls $base, ...)`
/// - Regular fields: emits `ResolveFrom<$base>` impl using `$($self_mod)*::__PartN`
fn impls_body_for(fields: &[BaseField<'_>]) -> TokenStream {
    let mut regular_idx = 0usize;
    let mut spread_idx = 0usize;
    let stmts: Vec<_> = fields
        .iter()
        .map(|f| {
            if f.is_spread {
                let macro_name = spread_macro_ident(f.ty);
                let spread_alias = Ident::new(&format!("__spread{spread_idx}"), Span::call_site());
                spread_idx += 1;
                quote! { $($self_mod)* :: #spread_alias :: #macro_name!(@impls $base, $($self_mod)* :: #spread_alias); }
            } else {
                let part_name = Ident::new(&format!("__Part{regular_idx}"), Span::call_site());
                regular_idx += 1;
                quote! {
                    impl ::autoresolve::ResolveFrom<$base> for $($self_mod)* :: #part_name {
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
/// - `#[spread]` fields: binds the field to a local and delegates to path-based macro call
/// - Regular fields: calls `$store.store_value($name.field)`
fn insert_body_for(fields: &[BaseField<'_>]) -> TokenStream {
    let mut spread_idx = 0usize;
    let stmts: Vec<_> = fields
        .iter()
        .map(|f| {
            let ident = f.ident;
            if f.is_spread {
                let macro_name = spread_macro_ident(f.ty);
                let spread_alias = Ident::new(&format!("__spread{spread_idx}"), Span::call_site());
                spread_idx += 1;
                quote! {
                    {
                        let __spread = $name.#ident;
                        $($self_mod)* :: #spread_alias :: #macro_name!(@insert $store, __spread, $($self_mod)* :: #spread_alias);
                    }
                }
            } else {
                quote! { $store.store_value($name.#ident); }
            }
        })
        .collect();

    quote! { #(#stmts)* }
}

fn generate_scoped(
    mod_ident: &Ident,
    mod_vis: &syn::Visibility,
    mod_attrs: &[&syn::Attribute],
    other_items: &[&Item],
    struct_name: &Ident,
    clean_struct: &TokenStream,
    fields: &[BaseField<'_>],
    parent: &syn::Path,
) -> syn::Result<TokenStream> {
    let parent_struct = &parent.segments.last().expect("parent path validated during parsing").ident;
    let parent_mod_outer = parent_module_path_for_outer(parent);
    let parent_mod_inner = parent_module_path_for_inner(parent);

    let reexports = regular_reexports(fields);
    let friendly = friendly_reexports(fields);
    let spread_re = spread_reexports(fields);

    // For regular fields: generate ResolveFrom<Self> (inside the module, no macro calls).
    let regular_impls: Vec<_> = fields
        .iter()
        .filter(|f| !f.is_spread)
        .map(|f| {
            let ty = f.ty;
            quote! {
                impl ::autoresolve::ResolveFrom<#struct_name> for #ty {
                    type Inputs = ::autoresolve::ResolutionDepsEnd;

                    fn new(_: ::autoresolve::ResolutionDepsEnd) -> Self {
                        unreachable!("scoped root types are inserted by BaseType::insert_into")
                    }
                }
            }
        })
        .collect();

    // Definition-site calls use module-path-based macro invocations.
    let qualified_name = quote! { #mod_ident::#struct_name };

    // Propagate parent's root types into this scope (path-based).
    let parent_propagation = quote! { #parent_mod_outer::#parent_struct!(@impls #qualified_name, #parent_mod_outer); };

    // For #[spread] fields: invoke their @impls arm (path-based).
    let spread_impls: Vec<_> = fields
        .iter()
        .filter(|f| f.is_spread)
        .enumerate()
        .map(|(i, f)| {
            let macro_name = spread_macro_ident(f.ty);
            let spread_alias = Ident::new(&format!("__spread{i}"), Span::call_site());
            quote! { #mod_ident::#spread_alias::#macro_name!(@impls #qualified_name, #mod_ident::#spread_alias); }
        })
        .collect();

    let field_idents: Vec<_> = fields.iter().map(|f| f.ident).collect();

    // Insertion logic: spread fields use path-based @insert, regular fields use store_value.
    let mut spread_insert_idx = 0usize;
    let insert_stmts: Vec<_> = fields
        .iter()
        .map(|f| {
            let ident = f.ident;
            if f.is_spread {
                let macro_name = spread_macro_ident(f.ty);
                let spread_alias = Ident::new(&format!("__spread{spread_insert_idx}"), Span::call_site());
                spread_insert_idx += 1;
                quote! { #mod_ident::#spread_alias::#macro_name!(@insert __store, #ident, #mod_ident::#spread_alias); }
            } else {
                quote! { __store.store_value(#ident); }
            }
        })
        .collect();

    let own_impls_body = impls_body_for(fields);
    let own_insert_body = insert_body_for(fields);
    let canaries = canary_refs(fields, mod_ident);
    let mangled_macro_name = Ident::new(&format!("__autoresolve_{mod_ident}"), Span::call_site());

    Ok(quote! {
        #(#mod_attrs)*
        #mod_vis mod #mod_ident {
            #(#other_items)*

            #clean_struct

            #(#reexports)*

            #(#friendly)*

            #(#regular_impls)*

            #(#spread_re)*

            #[doc(hidden)]
            pub(crate) use #parent_mod_inner as __parent;

            #[doc(hidden)]
            #[macro_export]
            macro_rules! #mangled_macro_name {
                (@impls $base:path, $($self_mod:tt)*) => {
                    $($self_mod)* :: __parent :: #parent_struct!(@impls $base, $($self_mod)* :: __parent);
                    #own_impls_body
                };
                (@insert $store:ident, $name:ident, $($self_mod:tt)*) => {
                    #own_insert_body
                };
            }
            #[doc(hidden)]
            pub use #mangled_macro_name as #struct_name;
        }

        // ScopedUnder impl (outside module, parent path resolves at the definition site).
        impl ::autoresolve::ScopedUnder for #mod_ident::#struct_name {
            type Parent = #parent;
        }

        // Definition-site: propagate parent + spread types into this scope.
        #parent_propagation
        #(#spread_impls)*

        impl ::autoresolve::BaseType<#mod_ident::#struct_name> for #mod_ident::#struct_name {
            fn insert_into(self, __store: &mut impl ::autoresolve::ResolverStore<#mod_ident::#struct_name>) {
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
        let attr = TokenStream::new();
        let input = quote! {
            mod app_base {
                struct Base {
                    #[spread]
                    builtins: builtins_mod::Builtins,
                    telemetry: Telemetry,
                }
            }
        };
        let result = base(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    #[test]
    fn primary_base_all_regular() {
        let attr = TokenStream::new();
        let input = quote! {
            mod base_mod {
                struct Base {
                    scheduler: Scheduler,
                    clock: Clock,
                }
            }
        };
        let result = base(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    #[test]
    fn scoped_base() {
        let attr = quote! { scoped(base_mod::Base) };
        let input = quote! {
            mod scoped_mod {
                struct ScopedRoots {
                    request_context: RequestContext,
                }
            }
        };
        let result = base(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    #[test]
    fn error_on_empty_module() {
        let attr = TokenStream::new();
        let input = quote! {
            mod empty {}
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }

    #[test]
    fn error_on_no_body() {
        let attr = TokenStream::new();
        let input = quote! {
            mod empty;
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }

    #[test]
    fn error_on_generics() {
        let attr = TokenStream::new();
        let input = quote! {
            mod base_mod {
                struct Base<T> {
                    field: T,
                }
            }
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }

    #[test]
    fn error_on_empty_struct() {
        let attr = TokenStream::new();
        let input = quote! {
            mod base_mod {
                struct Base {}
            }
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }

    #[test]
    fn scoped_base_with_spread() {
        let attr = quote! { scoped(base_mod::Base) };
        let input = quote! {
            mod scoped_mod {
                struct ScopedRoots {
                    #[spread]
                    builtins: builtins_mod::Builtins,
                    request_context: RequestContext,
                }
            }
        };
        let result = base(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    #[test]
    fn error_on_invalid_attribute() {
        let attr = quote! { something_wrong };
        let input = quote! {
            mod base_mod {
                struct Base {
                    field: Field,
                }
            }
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }
}
