use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Fields, Item, ItemMod, ItemStruct, Path, Type};

pub fn composite(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let item_mod: ItemMod = syn::parse2(item)?;

    let (_brace, items) = item_mod
        .content
        .as_ref()
        .ok_or_else(|| syn::Error::new_spanned(&item_mod, "#[composite] requires a module with a body (not `mod name;`)"))?;

    // Find exactly one struct with named fields.
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
                "#[composite] module must contain a struct with named fields",
            ));
        }
        _ => {
            return Err(syn::Error::new_spanned(
                &item_mod,
                "#[composite] module must contain exactly one struct with named fields",
            ));
        }
    };

    if !the_struct.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &the_struct.generics,
            "#[composite] does not support generic structs",
        ));
    }

    let named_fields = match &the_struct.fields {
        Fields::Named(named) => &named.named,
        _ => unreachable!("guarded by filter above"),
    };

    if named_fields.is_empty() {
        return Err(syn::Error::new_spanned(the_struct, "#[composite] requires at least one field"));
    }

    let struct_name = &the_struct.ident;
    let mod_name = &item_mod.ident;
    let mod_vis = &item_mod.vis;
    let mod_attrs = &item_mod.attrs;

    // The module path for generated `$crate::<path>::__PartN` references.
    // The user must provide it via `#[composite(path::to::module)]`.
    if attr.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[composite] requires a module path argument, e.g. #[composite(my_module)]",
        ));
    }
    let macro_mod_path: Path = syn::parse2(attr)?;

    // Validate field types are paths (required for re-export generation).
    for field in named_fields.iter() {
        if !matches!(&field.ty, Type::Path(_)) {
            return Err(syn::Error::new_spanned(&field.ty, "#[composite] field types must be type paths"));
        }
    }

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

    // Hidden re-exports so the generated macro can reference field types via `$crate::mod::__PartN`.
    // Also public re-exports of each field type using its last path segment, so that consumers
    // who glob-import the module (e.g., `pub use builtins::*`) get the field types available.
    let reexports: Vec<_> = named_fields
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let field_ty = &field.ty;
            let part_name = syn::Ident::new(&format!("__Part{i}"), field_ty.span());
            quote! {
                #[doc(hidden)]
                pub use #field_ty as #part_name;
            }
        })
        .collect();

    let friendly_reexports: Vec<_> = named_fields
        .iter()
        .filter_map(|field| {
            if let Type::Path(ty_path) = &field.ty {
                let last_seg = &ty_path.path.segments.last()?.ident;
                Some(quote! {
                    pub use #ty_path as #last_seg;
                })
            } else {
                None
            }
        })
        .collect();

    // Body of the generated macro's `@impls` arm: for each field, emit a `ResolveFrom` impl
    // using `$crate::<macro_mod_path>::__PartN` so consumers don't need direct dependencies on
    // the field types' crates.
    let impls_body: Vec<_> = named_fields
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let part_name = syn::Ident::new(&format!("__Part{i}"), field.ty.span());
            quote! {
                impl ::autoresolve::ResolveFrom<$base> for $crate::#macro_mod_path::#part_name {
                    type Inputs = ::autoresolve::ResolutionDepsEnd;

                    fn new(_: ::autoresolve::ResolutionDepsEnd) -> Self {
                        unreachable!("composite parts are pre-inserted into the resolver")
                    }
                }
            }
        })
        .collect();

    // Body of the generated macro's `@insert` arm: move each field into the store.
    let insert_body: Vec<_> = named_fields
        .iter()
        .map(|field| {
            let field_name = field.ident.as_ref().expect("guarded by named-fields check above");
            quote! {
                $store.store_value($name.#field_name);
            }
        })
        .collect();

    // Canary references that validate the module path is correct at the definition site.
    // Uses `crate::<path>::__PartN` (not `$crate`, which is only available inside macro_rules).
    let canary_refs: Vec<_> = named_fields
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let part_name = syn::Ident::new(&format!("__Part{i}"), field.ty.span());
            quote! {
                _ = std::mem::size_of::<crate::#macro_mod_path::#part_name>();
            }
        })
        .collect();

    let generated = quote! {
        #(#mod_attrs)*
        #mod_vis mod #mod_name {
            #(#items)*

            #(#composite_part_impls)*

            #(#reexports)*

            #(#friendly_reexports)*

            #[doc(hidden)]
            #[macro_export]
            macro_rules! #struct_name {
                (@impls $base:ident) => {
                    #(#impls_body)*
                };
                (@insert $store:ident, $name:ident) => {
                    #(#insert_body)*
                };
            }
        }

        const _: fn() = || {
            #(#canary_refs)*
        };
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
        let attr = quote! { builtins };
        let input = quote! {
            mod builtins {
                struct Builtins {
                    scheduler: Scheduler,
                    clock: Clock,
                }
            }
        };
        let result = composite(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    #[test]
    fn single_field_composite() {
        let attr = quote! { wrapper };
        let input = quote! {
            mod wrapper {
                struct Wrapper {
                    inner: Inner,
                }
            }
        };
        let result = composite(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    #[test]
    fn nested_path_composite() {
        let attr = quote! { nested::deep::builtins };
        let input = quote! {
            mod builtins {
                struct Builtins {
                    scheduler: Scheduler,
                    clock: Clock,
                }
            }
        };
        let result = composite(attr, input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    #[test]
    fn error_missing_path() {
        let input = quote! {
            mod foo {
                struct Foo {
                    x: X,
                }
            }
        };
        let result = composite(TokenStream::new(), input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires a module path argument"));
    }

    #[test]
    fn error_invalid_attr() {
        let attr = quote! { 42 };
        let input = quote! {
            mod foo {
                struct Foo {
                    x: X,
                }
            }
        };
        let result = composite(attr, input);
        assert!(result.is_err());
    }

    #[test]
    fn error_no_body() {
        let attr = quote! { foo };
        let input = quote! {
            mod foo;
        };
        let result = composite(attr, input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("module with a body"));
    }

    #[test]
    fn error_no_struct() {
        let attr = quote! { foo };
        let input = quote! {
            mod foo {
                const X: i32 = 1;
            }
        };
        let result = composite(attr, input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must contain a struct"));
    }

    #[test]
    fn error_generic_struct() {
        let attr = quote! { foo };
        let input = quote! {
            mod foo {
                struct Foo<T> {
                    x: T,
                }
            }
        };
        let result = composite(attr, input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not support generic structs"));
    }

    #[test]
    fn error_empty_fields() {
        let attr = quote! { foo };
        let input = quote! {
            mod foo {
                struct Foo {}
            }
        };
        let result = composite(attr, input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least one field"));
    }
}
