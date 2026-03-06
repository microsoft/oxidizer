use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, Ident, ItemStruct, Type};

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
    Scoped(Ident),
}

fn parse_mode(attr: TokenStream) -> syn::Result<BaseMode> {
    if attr.is_empty() {
        return Ok(BaseMode::Primary);
    }

    // Expect: `scoped(Ident)`
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

    let parent: Ident = syn::parse2(group.stream())?;
    Ok(BaseMode::Scoped(parent))
}

pub fn base(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let mode = parse_mode(attr)?;

    let item_struct: ItemStruct = syn::parse2(item)?;

    if !item_struct.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_struct.generics,
            "#[base] does not support generic structs",
        ));
    }

    let named_fields = match &item_struct.fields {
        Fields::Named(named) => &named.named,
        _ => {
            return Err(syn::Error::new_spanned(&item_struct, "#[base] requires a struct with named fields"));
        }
    };

    if named_fields.is_empty() {
        return Err(syn::Error::new_spanned(&item_struct, "#[base] requires at least one field"));
    }

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

    // Validate: #[spread] fields must have a single-segment type path (used as macro name).
    for f in &fields {
        if f.is_spread {
            if let Type::Path(tp) = f.ty {
                if tp.path.segments.len() != 1 || tp.qself.is_some() {
                    return Err(syn::Error::new_spanned(
                        f.ty,
                        "#[spread] field type must be a single identifier (the composite macro name)",
                    ));
                }
            }
        }
    }

    // Re-emit the struct, stripping #[spread] attributes from fields.
    let struct_vis = &item_struct.vis;
    let struct_attrs: Vec<_> = item_struct.attrs.iter().collect();
    let struct_name = &item_struct.ident;
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

    match mode {
        BaseMode::Primary => generate_primary(struct_name, &clean_struct, &fields),
        BaseMode::Scoped(parent) => generate_scoped(&clean_struct, &fields, &parent),
    }
}

fn generate_primary(struct_name: &Ident, clean_struct: &TokenStream, fields: &[BaseField<'_>]) -> syn::Result<TokenStream> {
    // For #[spread] fields: invoke the composite's @impls arm.
    let spread_impls: Vec<_> = fields
        .iter()
        .filter(|f| f.is_spread)
        .map(|f| {
            let ty = f.ty;
            quote! { #ty!(@impls #struct_name); }
        })
        .collect();

    // For regular fields: generate ResolveFrom<Base> impls.
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

    // Destructure field names for the BaseType impl.
    let field_idents: Vec<_> = fields.iter().map(|f| f.ident).collect();

    // Insertion logic: spread fields use composite @insert, regular fields use direct insert.
    let insert_stmts: Vec<_> = fields
        .iter()
        .map(|f| {
            let ident = f.ident;
            if f.is_spread {
                let ty = f.ty;
                quote! { #ty!(@insert __resolver, #ident); }
            } else {
                quote! { __resolver.insert(#ident); }
            }
        })
        .collect();

    Ok(quote! {
        #clean_struct

        #(#spread_impls)*

        #(#regular_impls)*

        impl ::autoresolve::BaseType for #struct_name {
            fn into_resolver(self) -> ::autoresolve::Resolver<Self> {
                let Self { #(#field_idents),* } = self;
                let mut __resolver = ::autoresolve::Resolver::<Self>::new_empty();
                #(#insert_stmts)*
                __resolver
            }
        }
    })
}

fn generate_scoped(clean_struct: &TokenStream, fields: &[BaseField<'_>], parent: &Ident) -> syn::Result<TokenStream> {
    // For scoped mode, #[spread] is not supported — each field is a direct root type.
    for f in fields {
        if f.is_spread {
            return Err(syn::Error::new_spanned(f.ty, "#[spread] is not supported in #[base(scoped(...))]"));
        }
    }

    // Generate ResolveFrom<Parent> for each field type.
    let impls: Vec<_> = fields
        .iter()
        .map(|f| {
            let ty = f.ty;
            quote! {
                impl ::autoresolve::ResolveFrom<#parent> for #ty {
                    type Inputs = ::autoresolve::ResolutionDepsEnd;

                    fn new(_: ::autoresolve::ResolutionDepsEnd) -> Self {
                        unreachable!("scoped root types are pre-inserted into the scoped resolver")
                    }
                }
            }
        })
        .collect();

    Ok(quote! {
        #clean_struct

        #(#impls)*
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
        let attr = TokenStream::new();
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
        let attr = quote! { scoped(Base) };
        let input = quote! {
            struct ScopedRoots {
                request_context: RequestContext,
            }
        };
        let result = base(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    #[test]
    fn error_on_empty_struct() {
        let attr = TokenStream::new();
        let input = quote! {
            struct Empty {}
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }

    #[test]
    fn error_on_generics() {
        let attr = TokenStream::new();
        let input = quote! {
            struct Base<T> {
                field: T,
            }
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }

    #[test]
    fn error_on_tuple_struct() {
        let attr = TokenStream::new();
        let input = quote! {
            struct Base(Scheduler, Clock);
        };
        let result = base(attr, input);
        assert!(result.is_err());
    }

    #[test]
    fn error_on_scoped_with_spread() {
        let attr = quote! { scoped(Base) };
        let input = quote! {
            struct ScopedRoots {
                #[spread]
                builtins: Builtins,
            }
        };
        let result = base(attr, input);
        assert!(result.is_err());
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
}
