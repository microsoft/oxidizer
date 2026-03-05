use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, ItemStruct};

pub fn composite(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new_spanned(attr, "#[composite] does not take any arguments"));
    }

    let item_struct: ItemStruct = syn::parse2(item)?;

    if !item_struct.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_struct.generics,
            "#[composite] does not support generic structs",
        ));
    }

    let named_fields = match &item_struct.fields {
        Fields::Named(named) => &named.named,
        _ => {
            return Err(syn::Error::new_spanned(
                &item_struct,
                "#[composite] requires a struct with named fields",
            ));
        }
    };

    if named_fields.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_struct,
            "#[composite] requires at least one field",
        ));
    }

    let struct_name = &item_struct.ident;

    // Generate `impl CompositePart<N> for Struct` for each field.
    let composite_part_impls: Vec<_> = named_fields
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let field_ty = &field.ty;
            quote! {
                impl ::autoresolve::CompositePart<#i> for #struct_name {
                    type Part = #field_ty;
                }
            }
        })
        .collect();

    // Body of the generated macro's `@impls` arm: for each field, emit a `ResolveFrom` impl
    // using an associated-type projection to avoid naming the field type directly.
    let impls_body: Vec<_> = named_fields
        .iter()
        .enumerate()
        .map(|(i, _)| {
            quote! {
                impl ::autoresolve::ResolveFrom<$base> for <$self_ty as ::autoresolve::CompositePart<#i>>::Part {
                    type Inputs = ::autoresolve::ResolutionDepsEnd;

                    fn new(_: ::autoresolve::ResolutionDepsEnd) -> Self {
                        unreachable!("composite parts are pre-inserted into the resolver")
                    }
                }
            }
        })
        .collect();

    // Body of the generated macro's `@insert` arm: move each field into the resolver.
    let insert_body: Vec<_> = named_fields
        .iter()
        .map(|field| {
            let field_name = field.ident.as_ref().expect("guarded by named-fields check above");
            quote! {
                $resolver.insert($name.#field_name);
            }
        })
        .collect();

    let generated = quote! {
        #item_struct

        #(#composite_part_impls)*

        #[doc(hidden)]
        #[macro_export]
        macro_rules! #struct_name {
            (@impls $base:ident, $self_ty:ty) => {
                #(#impls_body)*
            };
            (@insert $resolver:ident, $name:ident) => {
                #(#insert_body)*
            };
        }
    };

    Ok(generated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pretty_print(tokens: TokenStream) -> String {
        let file = syn::parse2::<syn::File>(tokens).unwrap_or_else(|e| panic!("Failed to parse generated code: {e}"));
        prettyplease::unparse(&file)
    }

    #[test]
    fn basic_composite() {
        let input = quote! {
            struct Builtins {
                scheduler: Scheduler,
                clock: Clock,
            }
        };
        let result = composite(TokenStream::new(), input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    #[test]
    fn single_field_composite() {
        let input = quote! {
            struct Wrapper {
                inner: Inner,
            }
        };
        let result = composite(TokenStream::new(), input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    #[test]
    fn error_empty_attr() {
        let attr = quote! { something };
        let input = quote! {
            struct Foo {
                x: X,
            }
        };
        let result = composite(attr, input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not take any arguments"));
    }

    #[test]
    fn error_generic_struct() {
        let input = quote! {
            struct Foo<T> {
                x: T,
            }
        };
        let result = composite(TokenStream::new(), input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not support generic structs"));
    }

    #[test]
    fn error_tuple_struct() {
        let input = quote! {
            struct Foo(X, Y);
        };
        let result = composite(TokenStream::new(), input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("named fields"));
    }

    #[test]
    fn error_unit_struct() {
        let input = quote! {
            struct Foo;
        };
        let result = composite(TokenStream::new(), input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("named fields"));
    }

    #[test]
    fn error_empty_fields() {
        let input = quote! {
            struct Foo {}
        };
        let result = composite(TokenStream::new(), input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least one field"));
    }
}
