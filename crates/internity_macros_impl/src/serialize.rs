// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, quote};
use syn::ext::IdentExt as _;
use syn::{Data, DeriveInput, Error, Fields, Path};

use crate::attrs::{parse_container, parse_field};
use crate::hygiene::{fresh_ident, used_identifiers};
use crate::roots::resolve_se_root;
use crate::shared::{
    NamedPlan, SerializeCtx, TuplePlan, container_name, is_phantom_data, reject_conflicting_serialize_modes, reject_skip_serializing_if,
    serialize_direct_call, serialize_field_expr, serialize_with_adapter_def, transparent_serialize_target, validate_transparent_container,
};

pub(crate) fn expand_serialize(input: &DeriveInput, root_path: &Path) -> syn::Result<TokenStream2> {
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "internity::SerializeIn cannot be derived for generic types",
        ));
    }

    let container = parse_container(&input.attrs)?;
    let root = resolve_se_root(&container, root_path);
    let ident = &input.ident;
    let used = used_identifiers(input);
    let reader = fresh_ident(&used, &format!("__InternityReaderFor{}", ident.unraw()));
    let serializer = fresh_ident(&used, &format!("__InternitySerializerFor{}", ident.unraw()));

    let Data::Struct(data) = &input.data else {
        return Err(Error::new_spanned(input, "internity::SerializeIn can only be derived for structs"));
    };
    validate_transparent_container(input, &container)?;
    if let Some(span) = container.into {
        return Err(Error::new(
            span,
            "serde `into` changes how a value is serialized, which internity's `SerializeIn` cannot honor",
        ));
    }

    let ctx = SerializeCtx {
        root: &root,
        container: &container,
        ident,
        hygiene: &used,
    };
    let body = match &data.fields {
        Fields::Named(fields) => expand_serialize_named(&ctx, &fields.named)?,
        Fields::Unnamed(fields) => expand_serialize_tuple(&ctx, &fields.unnamed)?,
        Fields::Unit => expand_serialize_unit(&ctx),
    };
    let serde = quote!(#root::__private::serde);

    Ok(quote! {
        #[doc(hidden)]
        #[allow(non_camel_case_types, non_upper_case_globals, unused_qualifications)]
        const _: () = {
            #[automatically_derived]
            impl<#reader> #root::SerializeIn<#reader> for #ident
            where
                #reader: #root::__private::Reader + ?Sized,
            {
                fn serialize_in<#serializer>(
                    &self,
                    __reader: &#reader,
                    __serializer: #serializer,
                ) -> ::core::result::Result<#serializer::Ok, #serializer::Error>
                where
                    #serializer: #serde::Serializer,
                {
                    #body
                }
            }
        };
    })
}

pub(crate) fn expand_serialize_named(
    ctx: &SerializeCtx<'_>,
    fields: &syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
) -> syn::Result<TokenStream2> {
    let SerializeCtx {
        root,
        container,
        ident,
        hygiene,
    } = *ctx;
    let serde = quote!(#root::__private::serde);
    let mut plans = Vec::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        let attrs = parse_field(&field.attrs)?;
        let fident = field.ident.clone().expect("named field");
        let natural = fident.unraw().to_string();
        let wire_name = attrs
            .rename
            .clone()
            .unwrap_or_else(|| container.rename_all.map_or_else(|| natural.clone(), |rule| rule.apply(&natural)));
        let serialize_wire_name = attrs.serialize_rename.clone().unwrap_or_else(|| {
            container
                .serialize_rename_all
                .map_or_else(|| natural.clone(), |rule| rule.apply(&natural))
        });
        let with_seed = attrs
            .with
            .as_ref()
            .map(|_| fresh_ident(hygiene, &format!("__InternityWithSeed{index}For{}", ident.unraw())));
        let serialize_with_adapter = attrs
            .serialize_with
            .as_ref()
            .map(|_| fresh_ident(hygiene, &format!("__InternitySerializeWith{index}For{}", ident.unraw())));
        plans.push(NamedPlan {
            ident: fident,
            ty: field.ty.clone(),
            attrs,
            wire_name,
            serialize_wire_name,
            with_seed,
            serialize_with_adapter,
            binding: fresh_ident(hygiene, &format!("__internity_value_{index}")),
            slot: fresh_ident(hygiene, &format!("__internity_slot_{index}")),
        });
    }

    for plan in &plans {
        reject_skip_serializing_if(&plan.ident, &plan.attrs)?;
        reject_conflicting_serialize_modes(&plan.ident, &plan.attrs)?;
    }

    if container.transparent {
        let mut transparent = plans.iter().filter(|plan| transparent_serialize_target(plan));
        let (Some(target), None) = (transparent.next(), transparent.next()) else {
            return Err(Error::new_spanned(
                ident,
                "serde `transparent` requires exactly one field that is not skipped serializing or PhantomData",
            ));
        };
        let access = target.ident.to_token_stream();
        let call = serialize_direct_call(root, &serde, &access, &target.attrs);
        return Ok(call);
    }

    let adapter_defs = plans.iter().filter(|plan| !plan.attrs.skip_serializing).filter_map(|plan| {
        plan.serialize_with_adapter.as_ref().and_then(|adapter| {
            plan.attrs
                .serialize_with
                .as_ref()
                .map(|path| serialize_with_adapter_def(root, adapter, &plan.ty, path))
        })
    });
    let fields: Vec<&NamedPlan> = plans.iter().filter(|plan| !plan.attrs.skip_serializing).collect();
    let field_count = fields.len();
    let struct_name = container_name(ident, container.serialize_rename.as_deref());
    let serialize_fields = fields.iter().map(|plan| {
        let name = &plan.serialize_wire_name;
        let access = plan.ident.to_token_stream();
        let expr = serialize_field_expr(root, &access, &plan.attrs, plan.serialize_with_adapter.as_ref());
        quote! {
            #serde::ser::SerializeStruct::serialize_field(&mut __state, #name, &#expr)?;
        }
    });

    Ok(quote! {
        #(#adapter_defs)*
        let mut __state = #serde::ser::Serializer::serialize_struct(__serializer, #struct_name, #field_count)?;
        #(#serialize_fields)*
        #serde::ser::SerializeStruct::end(__state)
    })
}

pub(crate) fn expand_serialize_tuple(
    ctx: &SerializeCtx<'_>,
    fields: &syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
) -> syn::Result<TokenStream2> {
    let SerializeCtx {
        root,
        container,
        ident,
        hygiene,
    } = *ctx;
    let serde = quote!(#root::__private::serde);
    let mut plans = Vec::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        let attrs = parse_field(&field.attrs)?;
        let serialize_with_adapter = attrs
            .serialize_with
            .as_ref()
            .map(|_| fresh_ident(hygiene, &format!("__InternitySerializeWith{index}For{}", ident.unraw())));
        let with_seed = attrs
            .with
            .as_ref()
            .map(|_| fresh_ident(hygiene, &format!("__InternityWithSeed{index}For{}", ident.unraw())));
        plans.push(TuplePlan {
            index: syn::Index::from(index),
            ty: field.ty.clone(),
            attrs,
            with_seed,
            serialize_with_adapter,
            binding: fresh_ident(hygiene, &format!("__value{index}")),
        });
    }

    for plan in &plans {
        reject_skip_serializing_if(&plan.ty, &plan.attrs)?;
        reject_conflicting_serialize_modes(&plan.ty, &plan.attrs)?;
    }

    if container.transparent {
        let plan = plans.first().expect("transparent tuple validation requires one field");
        if plan.attrs.skip_serializing || is_phantom_data(&plan.ty) {
            return Err(Error::new_spanned(
                ident,
                "serde `transparent` requires exactly one field that is not skipped serializing or PhantomData",
            ));
        }
        let access = plan.index.to_token_stream();
        let call = serialize_direct_call(root, &serde, &access, &plan.attrs);
        return Ok(call);
    }

    let adapter_defs = plans.iter().filter(|plan| !plan.attrs.skip_serializing).filter_map(|plan| {
        plan.serialize_with_adapter.as_ref().and_then(|adapter| {
            plan.attrs
                .serialize_with
                .as_ref()
                .map(|path| serialize_with_adapter_def(root, adapter, &plan.ty, path))
        })
    });
    let fields: Vec<&TuplePlan> = plans.iter().filter(|plan| !plan.attrs.skip_serializing).collect();
    let field_count = fields.len();
    let struct_name = container_name(ident, container.serialize_rename.as_deref());

    if plans.len() == 1 && field_count == 1 {
        let plan = fields[0];
        let access = plan.index.to_token_stream();
        let expr = serialize_field_expr(root, &access, &plan.attrs, plan.serialize_with_adapter.as_ref());
        return Ok(quote! {
            #(#adapter_defs)*
            #serde::ser::Serializer::serialize_newtype_struct(__serializer, #struct_name, &#expr)
        });
    }

    let serialize_fields = fields.iter().map(|plan| {
        let access = plan.index.to_token_stream();
        let expr = serialize_field_expr(root, &access, &plan.attrs, plan.serialize_with_adapter.as_ref());
        quote! {
            #serde::ser::SerializeTupleStruct::serialize_field(&mut __state, &#expr)?;
        }
    });

    Ok(quote! {
        #(#adapter_defs)*
        let mut __state = #serde::ser::Serializer::serialize_tuple_struct(__serializer, #struct_name, #field_count)?;
        #(#serialize_fields)*
        #serde::ser::SerializeTupleStruct::end(__state)
    })
}

pub(crate) fn expand_serialize_unit(ctx: &SerializeCtx<'_>) -> TokenStream2 {
    let SerializeCtx {
        root, container, ident, ..
    } = *ctx;
    let serde = quote!(#root::__private::serde);
    let struct_name = container_name(ident, container.serialize_rename.as_deref());
    quote!(#serde::ser::Serializer::serialize_unit_struct(__serializer, #struct_name))
}
