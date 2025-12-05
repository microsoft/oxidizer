// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![allow(clippy::too_many_lines, reason = "Complex generated macro logic")]

use std::collections::HashMap;

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::parse::Parser;
use syn::{Attribute, Fields, FieldsNamed, ItemStruct, Path, Type, Visibility, parse2};

/// Fundle bundle procedural macro implementation
pub fn bundle(_attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let input: ItemStruct = parse2(item)?;

    let struct_name = &input.ident;
    let builder_name = Ident::new(&format!("{struct_name}Builder"), struct_name.span());

    let Fields::Named(FieldsNamed { named: fields, .. }) = &input.fields else {
        return Ok(syn::Error::new_spanned(&input, "fundle::bundle only supports structs with named fields").to_compile_error());
    };

    // Collect field information
    let field_info: Vec<_> = fields.iter().collect();
    let field_names: Vec<_> = field_info
        .iter()
        .map(|f| {
            f.ident
                .as_ref()
                .expect("internal error: named field without identifier (this should be impossible after validation)")
        })
        .collect();
    let field_types: Vec<_> = field_info.iter().map(|f| &f.ty).collect();

    // Parse forward attributes
    let mut forward_info = Vec::new();
    for (i, field) in field_info.iter().enumerate() {
        if let Some(forward_types) = parse_forward_attribute(&field.attrs)? {
            forward_info.push((i, field_names[i], forward_types));
        }
    }

    // Generate type parameters (uppercase field names)
    let type_params: Vec<_> = field_names
        .iter()
        .map(|name| Ident::new(&name.to_string().to_uppercase(), name.span()))
        .collect();

    // Count occurrences of each type
    let mut type_counts = HashMap::new();
    for field_type in &field_types {
        let type_string = quote!(#field_type).to_string();
        *type_counts.entry(type_string).or_insert(0) += 1;
    }

    // Generate original struct without forward attributes
    let filtered_fields = field_info.iter().map(|field| {
        let field_name = &field.ident;
        let field_type = &field.ty;
        let field_vis = &field.vis;

        // Filter out forward attributes
        let filtered_attrs: Vec<_> = field.attrs.iter().filter(|attr| !attr.path().is_ident("forward")).collect();

        quote! {
            #(#filtered_attrs)*
            #field_vis #field_name: #field_type
        }
    });

    let struct_vis = &input.vis;
    let struct_attrs = &input.attrs;
    let original_struct = quote! {
        #(#struct_attrs)*
        #[allow(non_camel_case_types, non_snake_case)]
        #struct_vis struct #struct_name {
            #(#filtered_fields),*
        }
    };

    // Generate builder struct
    let builder_struct = generate_builder_struct(struct_vis, &builder_name, &field_names, &field_types, &type_params);

    // Generate Default impl for builder
    let default_impl = generate_default_impl(&builder_name, &field_names, &type_params);

    // Generate build method for original struct
    let struct_build_method = generate_struct_build_method(struct_name, &builder_name, &type_params);

    // Generate setter methods
    let setter_impls = generate_setter_impls(&builder_name, &field_names, &field_types, &type_params);

    // Generate AsRef impls for unique types
    let as_ref_impls = generate_as_ref_impls(&builder_name, &field_names, &field_types, &type_params, &type_counts);

    // Generate build method
    let build_impl = generate_build_impl(&builder_name, struct_name, &field_names, &type_params);

    // Generate forwarded AsRef implementations (split into struct and builder parts)
    let (forwarded_struct_as_ref_impls, forwarded_builder_as_ref_impls) =
        generate_forwarded_as_ref_impls(struct_name, &builder_name, &type_params, &forward_info);

    // Generate Export trait implementations
    let export_impls = generate_export_impls(struct_name, &field_types, &field_names);

    // Generate Export trait implementations for builder variants
    let builder_export_impls = generate_builder_export_impls(&builder_name, &field_names, &field_types, &type_params);

    // Generate Writer and Reader trait implementations
    let reader_writer_impls = generate_reader_writer_impls(&builder_name, &type_params);

    // Generate read() toggle method
    let read_toggle_impl = generate_read_toggle(&builder_name, &field_names, &type_params);

    // Generate getter methods for Read mode
    let reader_getters = generate_reader_getters(&builder_name, &field_names, &field_types, &type_params);

    // Generate AsRef implementations for unique field types on the main struct
    let main_struct_as_ref_impls = field_names
        .iter()
        .zip(field_types.iter())
        .filter_map(|(field_name, field_type)| {
            let type_string = quote!(#field_type).to_string();

            // Only generate AsRef for types that appear exactly once
            if type_counts.get(&type_string) != Some(&1) {
                return None;
            }

            Some(quote! {
                #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
                impl ::std::convert::AsRef<#field_type> for #struct_name {
                    fn as_ref(&self) -> &#field_type {
                        &self.#field_name
                    }
                }
            })
        })
        .collect::<Vec<_>>();

    // Generate the select macro
    let select_macro = generate_select_macro(struct_name, &builder_name, &field_names, &field_types, &type_params);

    // Create module name (underscore-prefixed struct name)
    let module_name = Ident::new(&format!("_{struct_name}"), struct_name.span());

    let expanded = quote! {
        #original_struct

        #struct_build_method

        #(#main_struct_as_ref_impls)*

        #(#forwarded_struct_as_ref_impls)*

        #export_impls

        #[allow(non_snake_case)]
        mod #module_name {
            use super::*;

            #builder_struct

            #default_impl

            #reader_writer_impls

            #read_toggle_impl

            #(#setter_impls)*

            #(#reader_getters)*

            #(#as_ref_impls)*

            #(#forwarded_builder_as_ref_impls)*

            #(#builder_export_impls)*

            #build_impl
        }

        #struct_vis use #module_name::#builder_name;

        #select_macro
    };

    Ok(expanded)
}

#[cfg_attr(test, mutants::skip)]
fn generate_builder_struct(
    _vis: &Visibility,
    builder_name: &Ident,
    field_names: &[&Ident],
    field_types: &[&Type],
    type_params: &[Ident],
) -> proc_macro2::TokenStream {
    let builder_fields = field_names.iter().zip(field_types.iter()).map(|(name, ty)| {
        quote! { #name: ::std::option::Option<#ty> }
    });

    let phantom_types = type_params.iter().map(|param| quote!(#param));

    quote! {
        #[allow(non_camel_case_types, dead_code, non_snake_case, clippy::items_after_statements)]
        pub struct #builder_name<RW, #(#type_params),*> {
            #(#builder_fields,)*
            _phantom: ::std::marker::PhantomData<(RW, #(#phantom_types),*)>,
        }
    }
}

fn generate_struct_build_method(struct_name: &Ident, builder_name: &Ident, type_params: &[Ident]) -> proc_macro2::TokenStream {
    let not_set_params = type_params.iter().map(|_| quote!(::fundle::NotSet));

    quote! {
        #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
        impl #struct_name {
            pub fn builder() -> #builder_name<::fundle::Write, #(#not_set_params),*> {
                #builder_name::default()
            }
        }
    }
}

#[cfg_attr(test, mutants::skip)]
fn generate_default_impl(builder_name: &Ident, field_names: &[&Ident], type_params: &[Ident]) -> proc_macro2::TokenStream {
    let not_set_params = type_params.iter().map(|_| quote!(::fundle::NotSet));
    let none_fields = field_names.iter().map(|name| quote!(#name: ::std::option::Option::None));

    quote! {
        #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
        impl ::std::default::Default for #builder_name<::fundle::Write, #(#not_set_params),*> {
            fn default() -> Self {
                Self {
                    #(#none_fields,)*
                    _phantom: ::std::marker::PhantomData,
                }
            }
        }
    }
}

#[expect(clippy::cognitive_complexity, reason = "Complex builder generation logic")]
#[cfg_attr(test, mutants::skip)]
fn generate_setter_impls(
    builder_name: &Ident,
    field_names: &[&Ident],
    field_types: &[&Type],
    type_params: &[Ident],
) -> Vec<proc_macro2::TokenStream> {
    let mut impls = Vec::new();

    for (i, (field_name, field_type)) in field_names.iter().zip(field_types.iter()).enumerate() {
        // Create type parameter list with current one as NotSet, others as generic
        let impl_params: Vec<_> = type_params
            .iter()
            .enumerate()
            .map(|(j, param)| if i == j { quote!(::fundle::NotSet) } else { quote!(#param) })
            .collect();

        // Create type parameter list with current one as Set, others as generic
        let return_params: Vec<_> = type_params
            .iter()
            .enumerate()
            .map(|(j, param)| if i == j { quote!(::fundle::Set) } else { quote!(#param) })
            .collect();

        // Other type parameters for the impl
        let other_params: Vec<_> = type_params
            .iter()
            .enumerate()
            .filter_map(|(j, param)| (i != j).then_some(param))
            .collect();

        // Try setter
        let try_method_name = Ident::new(&format!("{field_name}_try"), field_name.span());
        let try_async_method_name = Ident::new(&format!("{field_name}_try_async"), field_name.span());
        let async_method_name = Ident::new(&format!("{field_name}_async"), field_name.span());

        // Field assignments for rebuilding (using read.field instead of self.field)
        let field_assignments_from_read: Vec<_> = field_names
            .iter()
            .enumerate()
            .map(|(j, name)| {
                if i == j {
                    quote!(#name: ::std::option::Option::Some(#field_name))
                } else {
                    quote!(#name: read.#name)
                }
            })
            .collect();

        // Regular setter
        let setter = quote! {
            #[allow(non_camel_case_types, non_snake_case)]
            impl<#(#other_params),*> #builder_name<::fundle::Write, #(#impl_params),*> {
                pub fn #field_name(self, f: impl ::std::ops::Fn(&<Self as ::fundle::Writer>::Reader) -> #field_type) -> #builder_name<::fundle::Write, #(#return_params),*> {
                    let read = self.read();
                    let #field_name = f(&read);
                    #builder_name {
                        #(#field_assignments_from_read,)*
                        _phantom: ::std::marker::PhantomData,
                    }
                }

                pub fn #try_method_name<R: ::std::error::Error>(self, f: impl ::std::ops::Fn(&<Self as ::fundle::Writer>::Reader) -> ::std::result::Result<#field_type, R>) -> ::std::result::Result<#builder_name<::fundle::Write, #(#return_params),*>, R> {
                    let read = self.read();
                    let #field_name = f(&read)?;
                    ::std::result::Result::Ok(#builder_name {
                        #(#field_assignments_from_read,)*
                        _phantom: ::std::marker::PhantomData,
                    })
                }

                pub async fn #try_async_method_name<F, R: ::std::error::Error>(self, f: F) -> ::std::result::Result<#builder_name<::fundle::Write, #(#return_params),*>, R>
                where
                    F: AsyncFn(&<Self as ::fundle::Writer>::Reader) -> ::std::result::Result<#field_type, R>,
                {
                    let read = self.read();
                    let #field_name = f(&read).await?;
                    ::std::result::Result::Ok(#builder_name {
                        #(#field_assignments_from_read,)*
                        _phantom: ::std::marker::PhantomData,
                    })
                }

                pub async fn #async_method_name<F>(self, f: F) -> #builder_name<::fundle::Write, #(#return_params),*>
                where
                    F: AsyncFn(&<Self as ::fundle::Writer>::Reader) -> #field_type,
                {
                    let read = self.read();
                    let #field_name = f(&read).await;
                    #builder_name {
                        #(#field_assignments_from_read,)*
                        _phantom: ::std::marker::PhantomData,
                    }
                }
            }
        };

        impls.extend([setter]);
    }

    impls
}

#[cfg_attr(test, mutants::skip)]
fn generate_as_ref_impls(
    builder_name: &Ident,
    field_names: &[&Ident],
    field_types: &[&Type],
    type_params: &[Ident],
    #[expect(clippy::used_underscore_binding, reason = "Parameter used conditionally")] _type_counts: &HashMap<String, usize>,
) -> Vec<proc_macro2::TokenStream> {
    let mut impls = Vec::new();

    for (i, (field_name, field_type)) in field_names.iter().zip(field_types.iter()).enumerate() {
        let type_string = quote!(#field_type).to_string();

        // Only generate AsRef for types that appear exactly once
        if _type_counts.get(&type_string) == Some(&1) {
            // Create type parameter list with current one as Set, others as generic
            let impl_params: Vec<_> = type_params
                .iter()
                .enumerate()
                .map(|(j, param)| if i == j { quote!(::fundle::Set) } else { quote!(#param) })
                .collect();

            // Other type parameters for the impl
            let other_params: Vec<_> = type_params
                .iter()
                .enumerate()
                .filter_map(|(j, param)| (i != j).then_some(param))
                .collect();

            let as_ref_impl = quote! {
                #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
                impl<RW, #(#other_params),*> ::std::convert::AsRef<#field_type> for #builder_name<RW, #(#impl_params),*> {
                    fn as_ref(&self) -> &#field_type {
                        self.#field_name.as_ref().unwrap()
                    }
                }
            };

            impls.push(as_ref_impl);
        }
    }

    impls
}

#[cfg_attr(test, mutants::skip)]
fn generate_build_impl(
    builder_name: &Ident,
    struct_name: &Ident,
    field_names: &[&Ident],
    type_params: &[Ident],
) -> proc_macro2::TokenStream {
    let set_params: Vec<_> = type_params.iter().map(|_| quote!(::fundle::Set)).collect();
    let field_moves: Vec<_> = field_names.iter().map(|name| quote!(#name: self.#name.unwrap())).collect();

    quote! {
        #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
        impl #builder_name<::fundle::Write, #(#set_params),*> {
            pub fn build(self) -> #struct_name {
                #struct_name {
                    #(#field_moves),*
                }
            }
        }
    }
}

#[cfg_attr(test, mutants::skip)]
fn parse_forward_attribute(attrs: &[Attribute]) -> syn::Result<Option<Vec<Path>>> {
    for attr in attrs {
        if attr.path().is_ident("forward")
            && let Ok(meta_list) = attr.meta.require_list()
        {
            let tokens = &meta_list.tokens;
            // Parse as a comma-separated list using syn's punctuated parsing
            let parser = syn::punctuated::Punctuated::<Path, syn::Token![,]>::parse_terminated;
            let punctuated = parser.parse2(tokens.clone()).map_err(|e| {
                syn::Error::new_spanned(
                    attr,
                    format!("fundle::bundle #[forward(...)] attribute must contain valid type paths: {e}"),
                )
            })?;

            if punctuated.is_empty() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "fundle::bundle #[forward(...)] attribute cannot be empty",
                ));
            }

            let forward_types: Vec<Path> = punctuated.into_iter().collect();
            return Ok(Some(forward_types));
        }
    }
    Ok(None)
}

#[cfg_attr(test, mutants::skip)]
fn generate_forwarded_as_ref_impls(
    struct_name: &Ident,
    builder_name: &Ident,
    type_params: &[Ident],
    forward_info: &[(usize, &Ident, Vec<Path>)],
) -> (Vec<proc_macro2::TokenStream>, Vec<proc_macro2::TokenStream>) {
    let mut struct_impls = Vec::new();
    let mut builder_impls = Vec::new();

    // Generate AsRef impls for the final struct (all fields Set)
    for (_, field_name, forward_types) in forward_info {
        for forward_type in forward_types {
            let as_ref_impl = quote! {
                #[allow(non_camel_case_types, non_snake_case)]
                impl ::std::convert::AsRef<#forward_type> for #struct_name {
                    fn as_ref(&self) -> &#forward_type {
                        self.#field_name.as_ref()
                    }
                }
            };
            struct_impls.push(as_ref_impl);
        }
    }

    // Generate AsRef impls for the builder when the forwarded field is Set
    for (field_idx, field_name, forward_types) in forward_info {
        for forward_type in forward_types {
            // Create type parameter list with the forwarded field as Set, others as generic
            let impl_params: Vec<_> = type_params
                .iter()
                .enumerate()
                .map(|(j, param)| if *field_idx == j { quote!(::fundle::Set) } else { quote!(#param) })
                .collect();

            // Other type parameters for the impl (exclude the forwarded field's param)
            let other_params: Vec<_> = type_params
                .iter()
                .enumerate()
                .filter_map(|(j, param)| (*field_idx != j).then_some(param))
                .collect();

            let as_ref_impl = quote! {
                #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
                impl<RW, #(#other_params),*> ::std::convert::AsRef<#forward_type> for #builder_name<RW, #(#impl_params),*> {
                    fn as_ref(&self) -> &#forward_type {
                        self.#field_name.as_ref().unwrap().as_ref()
                    }
                }
            };
            builder_impls.push(as_ref_impl);
        }
    }

    (struct_impls, builder_impls)
}

#[cfg_attr(test, mutants::skip)]
fn generate_export_impls(struct_name: &Ident, field_types: &[&Type], field_names: &[&Ident]) -> proc_macro2::TokenStream {
    // Export all types, not just unique ones
    let num_exports = field_types.len();

    // Generate Exports implementation
    let exports_impl = quote! {
        impl ::fundle::exports::Exports for #struct_name {
            const NUM_EXPORTS: usize = #num_exports;
        }
    };

    // Generate Export<N> implementations for each type
    let export_impls: Vec<_> = field_types
        .iter()
        .enumerate()
        .zip(field_names.iter())
        .map(|((index, ty), field_name)| {
            quote! {
                #[allow(clippy::items_after_statements)]
                impl ::fundle::exports::Export<#index> for #struct_name {
                    type T = #ty;

                    fn get(&self) -> &Self::T {
                        &self.#field_name
                    }
                }
            }
        })
        .collect();

    quote! {
        #exports_impl

        #(#export_impls)*
    }
}

#[cfg_attr(test, mutants::skip)]
fn generate_builder_export_impls(
    builder_name: &Ident,
    field_names: &[&Ident],
    field_types: &[&Type],
    type_params: &[Ident],
) -> Vec<proc_macro2::TokenStream> {
    let mut impls = Vec::new();

    // For each field, generate Export impl only for that specific field when it's Set
    for (field_idx, (field_name, field_type)) in field_names.iter().zip(field_types.iter()).enumerate() {
        // Create type parameter list with this field as Set, others as generic
        let impl_params: Vec<_> = type_params
            .iter()
            .enumerate()
            .map(|(j, param)| if field_idx == j { quote!(::fundle::Set) } else { quote!(#param) })
            .collect();

        // Other type parameters for the impl (exclude this field's param)
        let other_params: Vec<_> = type_params
            .iter()
            .enumerate()
            .filter_map(|(j, param)| (field_idx != j).then_some(param))
            .collect();

        let export_impl = quote! {
            #[allow(non_camel_case_types, non_snake_case)]
            impl<RW, #(#other_params),*> ::fundle::exports::Export<#field_idx> for #builder_name<RW, #(#impl_params),*> {
                type T = #field_type;

                fn get(&self) -> &Self::T {
                    self.#field_name.as_ref().unwrap()
                }
            }
        };
        impls.push(export_impl);
    }

    impls
}

#[expect(clippy::too_many_lines, reason = "Generated macro with many patterns")]
#[cfg_attr(test, mutants::skip)]
fn generate_select_macro(
    struct_name: &Ident,
    builder_name: &Ident,
    field_names: &[&Ident],
    field_types: &[&Type],
    _type_params: &[Ident],
) -> proc_macro2::TokenStream {
    let macro_name = struct_name;
    let num_fields = field_names.len();

    // Count occurrences of each type
    let mut type_counts = std::collections::HashMap::new();
    for field_type in field_types {
        let type_string = quote!(#field_type).to_string();
        *type_counts.entry(type_string).or_insert(0) += 1;
    }

    // Generate type parameters for the Select struct
    let select_type_params = (1..=num_fields)
        .map(|i| Ident::new(&format!("T{i}"), proc_macro2::Span::call_site()))
        .collect::<Vec<_>>();

    // Generate concrete AsRef implementations only for unique field types when builder has it Set
    let builder_as_ref_impls = field_names
        .iter()
        .enumerate()
        .zip(field_types.iter())
        .filter_map(|((field_idx, _field_name), field_type)| {
            let type_string = quote!(#field_type).to_string();

            // Only generate AsRef for types that appear exactly once
            if type_counts.get(&type_string) != Some(&1) {
                return None;
            }

            // Create type parameter list for the impl where this field is Set, others are generic
            let impl_type_params = select_type_params
                .iter()
                .enumerate()
                .map(|(param_idx, param)| {
                    if param_idx == field_idx {
                        quote!(::fundle::Set)
                    } else {
                        quote!(#param)
                    }
                })
                .collect::<Vec<_>>();

            // Other type parameters for the impl generics (exclude the field being Set)
            let other_type_params = select_type_params
                .iter()
                .enumerate()
                .filter_map(|(param_idx, param)| (param_idx != field_idx).then_some(param))
                .collect::<Vec<_>>();

            Some(quote! {
                impl<'a, RW, #(#other_type_params),*> ::std::convert::AsRef<#field_type>
                    for Select<'a, RW, #(#impl_type_params),*>
                where
                    #builder_name<RW, #(#impl_type_params),*>: ::std::convert::AsRef<#field_type>,
                {
                    fn as_ref(&self) -> &#field_type {
                        self.builder.as_ref()
                    }
                }
            })
        })
        .collect::<Vec<_>>();

    // Generate individual verification patterns for each field
    let verification_patterns = field_names
        .iter()
        .enumerate()
        .map(|(field_idx, field_name)| {
            // Create generic type params (exclude the Set field)
            let generic_params = select_type_params
                .iter()
                .enumerate()
                .filter_map(|(param_idx, param)| (param_idx != field_idx).then_some(param))
                .collect::<Vec<_>>();

            // Create type params where this field is Set, others are generic
            let verification_params = select_type_params
                .iter()
                .enumerate()
                .map(|(param_idx, param)| {
                    if param_idx == field_idx {
                        quote!(::fundle::Set)
                    } else {
                        quote!(#param)
                    }
                })
                .collect::<Vec<_>>();

            // Generate a specific macro pattern for this field
            quote! {
                (verify_field $builder_var:ident #field_name) => {
                    {
                        fn verify_exists<RW, #(#generic_params),*>(_: &#builder_name<RW, #(#verification_params),*>) {}
                        verify_exists($builder_var);
                    }
                };
            }
        })
        .collect::<Vec<_>>();

    quote! {
        #[allow(unused_macros, snake_case)]
        macro_rules! #macro_name {
            // Verification patterns for each field
            #(#verification_patterns)*

            // Main select pattern
            (select($builder_var:ident) => $($forward_type:ident($forward_field:ident)),* $(,)?) => {
                {
                    // Generate compile-time verification blocks for each specified field
                    $(
                        #macro_name!(verify_field $builder_var $forward_field);
                    )*

                    #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
                    struct Select<'a, RW, #(#select_type_params),*> {
                        builder: &'a #builder_name<RW, #(#select_type_params),*>,
                        $($forward_type: &'a $forward_type,)*
                    }

                    #(#builder_as_ref_impls)*

                    $(
                        #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
                        impl<'a, RW, #(#select_type_params),*> ::std::convert::AsRef<$forward_type>
                            for Select<'a, RW, #(#select_type_params),*>
                        {
                            fn as_ref(&self) -> &$forward_type {
                                self.$forward_type
                            }
                        }
                    )*

                    Select {
                        builder: &$builder_var,
                        $($forward_type: $builder_var.$forward_field(),)*
                    }
                }
            };
        }
    }
}

#[cfg_attr(test, mutants::skip)]
fn generate_reader_writer_impls(builder_name: &Ident, type_params: &[Ident]) -> proc_macro2::TokenStream {
    quote! {
        #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
        impl<#(#type_params),*> ::fundle::Writer for #builder_name<::fundle::Write, #(#type_params),*> {
            type Reader = #builder_name<::fundle::Read, #(#type_params),*>;
        }

        #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
        impl<#(#type_params),*> ::fundle::Reader for #builder_name<::fundle::Read, #(#type_params),*> {
            type Writer = #builder_name<::fundle::Write, #(#type_params),*>;
        }
    }
}

#[cfg_attr(test, mutants::skip)]
fn generate_read_toggle(builder_name: &Ident, field_names: &[&Ident], type_params: &[Ident]) -> proc_macro2::TokenStream {
    let field_assignments = field_names.iter().map(|name| {
        quote! { #name: self.#name }
    });

    quote! {
        #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
        impl<#(#type_params),*> #builder_name<::fundle::Write, #(#type_params),*> {
            pub fn read(self) -> #builder_name<::fundle::Read, #(#type_params),*> {
                #builder_name {
                    #(#field_assignments,)*
                    _phantom: ::std::marker::PhantomData,
                }
            }
        }
    }
}

#[cfg_attr(test, mutants::skip)]
fn generate_reader_getters(
    builder_name: &Ident,
    field_names: &[&Ident],
    field_types: &[&Type],
    type_params: &[Ident],
) -> Vec<proc_macro2::TokenStream> {
    let mut impls = Vec::new();

    for (i, (field_name, field_type)) in field_names.iter().zip(field_types.iter()).enumerate() {
        // Create type parameter list with current one as Set, others as generic
        let impl_params: Vec<_> = type_params
            .iter()
            .enumerate()
            .map(|(j, param)| if i == j { quote!(::fundle::Set) } else { quote!(#param) })
            .collect();

        // Other type parameters for the impl (exclude current field's param)
        let other_params: Vec<_> = type_params
            .iter()
            .enumerate()
            .filter_map(|(j, param)| (i != j).then_some(param))
            .collect();

        let getter = quote! {
            #[allow(non_camel_case_types, non_snake_case)]
            impl<#(#other_params),*> #builder_name<::fundle::Read, #(#impl_params),*> {
                pub fn #field_name(&self) -> &#field_type {
                    self.#field_name.as_ref().unwrap()
                }
            }
        };

        impls.push(getter);
    }

    impls
}
