// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::ext::IdentExt as _;
use syn::{Data, DeriveInput, Error, Fields, LitByteStr, Path};

use crate::attrs::{DefaultValue, parse_container, parse_field};
use crate::hygiene::{fresh_ident, used_identifiers};
use crate::roots::resolve_de_root;
use crate::shared::{
    Ctx, NamedPlan, TuplePlan, container_name, field_seed, is_phantom_data, missing_value_expr, reject_conflicting_deserialize_modes,
    skip_default_expr, transparent_deserialize_target, transparent_other_expr, tuple_missing_value_expr, validate_transparent_container,
    with_seed_def,
};

pub(crate) fn expand_deserialize(input: &DeriveInput, root_path: &Path) -> syn::Result<TokenStream2> {
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "internity::DeserializeIn cannot be derived for generic types",
        ));
    }

    let container = parse_container(&input.attrs)?;
    let root = resolve_de_root(&container, root_path);
    let ident = &input.ident;
    let used = used_identifiers(input);
    let interner = fresh_ident(&used, &format!("__InternityInternerFor{}", ident.unraw()));
    let deserializer = fresh_ident(&used, &format!("__InternityDeserializerFor{}", ident.unraw()));

    let Data::Struct(data) = &input.data else {
        return Err(Error::new_spanned(
            input,
            "internity::DeserializeIn can only be derived for structs",
        ));
    };

    if container.default.is_some() && !matches!(&data.fields, Fields::Named(_) | Fields::Unnamed(_)) {
        return Err(Error::new_spanned(
            ident,
            "serde container `default` can only be used on structs that have fields",
        ));
    }
    validate_transparent_container(input, &container)?;
    if let Some(span) = container.from {
        return Err(Error::new(
            span,
            "serde `from` changes how a value is deserialized, which internity's `DeserializeIn` cannot honor",
        ));
    }
    if let Some(span) = container.try_from {
        return Err(Error::new(
            span,
            "serde `try_from` changes how a value is deserialized, which internity's `DeserializeIn` cannot honor",
        ));
    }

    let ctx = Ctx {
        root: &root,
        container: &container,
        ident,
        interner: &interner,
        hygiene: &used,
    };

    let body = match &data.fields {
        Fields::Named(fields) => expand_named(&ctx, &fields.named)?,
        Fields::Unnamed(fields) => expand_tuple(&ctx, &fields.unnamed)?,
        Fields::Unit => expand_unit(&ctx),
    };

    let serde = quote!(#root::__private::serde);

    Ok(quote! {
        #[doc(hidden)]
        #[allow(non_camel_case_types, non_upper_case_globals, unused_qualifications)]
        const _: () = {
            #[automatically_derived]
            impl<'de, #interner> #root::DeserializeIn<'de, #interner> for #ident
            where
                #interner: #root::__private::Lexicon + ?Sized,
            {
                fn deserialize_in<#deserializer>(
                    __internity_interner: &mut #interner,
                    __deserializer: #deserializer,
                ) -> ::core::result::Result<Self, #deserializer::Error>
                where
                    #deserializer: #serde::Deserializer<'de>,
                {
                    #body
                }
            }
        };
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "cohesive field-identifier and struct visitor codegen kept together"
)]
pub(crate) fn expand_named(
    ctx: &Ctx<'_>,
    fields: &syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
) -> syn::Result<TokenStream2> {
    let Ctx {
        root,
        container,
        ident,
        interner,
        hygiene,
    } = *ctx;
    let serde = quote!(#root::__private::serde);

    let mut plans = Vec::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        let attrs = parse_field(&field.attrs)?;
        reject_conflicting_deserialize_modes(field, &attrs)?;
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

    let struct_name = container_name(ident, container.rename.as_deref());
    let expecting = container.expecting.clone().unwrap_or_else(|| format!("struct {ident}"));

    // Seed definitions for `with` / `deserialize_with` fields.
    let with_seed_defs: Vec<TokenStream2> = plans
        .iter()
        .filter_map(|plan| {
            plan.with_seed
                .as_ref()
                .and_then(|name| with_seed_def(root, name, &plan.ty, &plan.attrs))
        })
        .collect();

    // Inside visitor methods the interner is reached through `self`.
    let interner_access = quote!(self.__internity_interner);

    // Container-`default` scratch binding, emitted only when a present field needs it.
    let container_default_binding = quote!(__container_default);
    let needs_container_default = container.default.is_some() && plans.iter().any(|plan| !plan.attrs.skip && plan.attrs.default.is_none());
    let container_default_stmt = if needs_container_default {
        let ctor = match container.default.as_ref().expect("guarded by needs_container_default") {
            DefaultValue::Trait => quote!(<#ident as ::core::default::Default>::default()),
            DefaultValue::Path(path) => quote!(#path()),
        };
        quote!(let #container_default_binding: #ident = #ctor;)
    } else {
        quote!()
    };

    // `transparent` structs deserialize their single transparent field directly.
    if container.transparent {
        let mut wire = plans.iter().filter(|plan| transparent_deserialize_target(plan));
        let (Some(target), None) = (wire.next(), wire.next()) else {
            return Err(Error::new_spanned(
                ident,
                "serde `transparent` requires exactly one field that is not skipped, defaulted, or PhantomData",
            ));
        };
        let top_access = quote!(__internity_interner);
        let seed = field_seed(root, interner, &target.ty, &target.attrs, target.with_seed.as_ref(), &top_access);
        let target_ident = &target.ident;
        let others = plans.iter().filter(|plan| !core::ptr::eq(*plan, target)).map(|plan| {
            let fident = &plan.ident;
            let value = transparent_other_expr(plan);
            quote!(#fident: #value)
        });
        return Ok(quote! {
            #(#with_seed_defs)*
            let _ = &__internity_interner;
            let #target_ident = #serde::de::DeserializeSeed::deserialize(#seed, __deserializer)?;
            ::core::result::Result::Ok(#ident { #target_ident, #(#others,)* })
        });
    }

    // Fields that participate in the map/seq/identifier surface (non-skipped).
    let wire: Vec<&NamedPlan> = plans.iter().filter(|plan| !plan.attrs.skip).collect();
    let deny = container.deny_unknown_fields;

    let field = fresh_ident(hygiene, &format!("__InternityFieldFor{}", ident.unraw()));
    let field_visitor = fresh_ident(hygiene, &format!("__InternityFieldVisitorFor{}", ident.unraw()));
    let visitor = fresh_ident(hygiene, &format!("__InternityVisitorFor{}", ident.unraw()));

    let wire_variants: Vec<syn::Ident> = (0..wire.len())
        .map(|index| fresh_ident(hygiene, &format!("__field{index}")))
        .collect();
    let index_lits: Vec<_> = (0..wire.len() as u64).map(proc_macro2::Literal::u64_unsuffixed).collect();

    // (accepted name, variant) pairs, including aliases, for string/byte matching.
    let mut accepted_names: Vec<String> = Vec::new();
    let mut accepted_variants: Vec<syn::Ident> = Vec::new();
    for (plan, variant) in wire.iter().zip(&wire_variants) {
        accepted_names.push(plan.wire_name.clone());
        accepted_variants.push(variant.clone());
        for alias in &plan.attrs.aliases {
            accepted_names.push(alias.clone());
            accepted_variants.push(variant.clone());
        }
    }
    let accepted_names_bytes: Vec<LitByteStr> = accepted_names
        .iter()
        .map(|name| LitByteStr::new(name.as_bytes(), proc_macro2::Span::call_site()))
        .collect();

    let ignore_variant = (!deny).then(|| quote!(__ignore,));
    let unknown_str = if deny {
        quote!(_ => return ::core::result::Result::Err(<__E as #serde::de::Error>::unknown_field(__v, __FIELDS)))
    } else {
        quote!(_ => #field::__ignore)
    };
    let unknown_bytes = if deny {
        quote! {
            _ => return match ::core::str::from_utf8(__v) {
                ::core::result::Result::Ok(__s) =>
                    ::core::result::Result::Err(<__E as #serde::de::Error>::unknown_field(__s, __FIELDS)),
                ::core::result::Result::Err(_) =>
                    ::core::result::Result::Err(<__E as #serde::de::Error>::custom("unknown field")),
            }
        }
    } else {
        quote!(_ => #field::__ignore)
    };
    let unknown_u64 = if deny {
        quote! {
            _ => return ::core::result::Result::Err(
                <__E as #serde::de::Error>::invalid_value(
                    #serde::de::Unexpected::Unsigned(__v),
                    &"a valid field index",
                ),
            )
        }
    } else {
        quote!(_ => #field::__ignore)
    };

    let slot_decls = wire.iter().map(|plan| {
        let slot = &plan.slot;
        quote!(let mut #slot = ::core::option::Option::None;)
    });
    let map_arms = wire.iter().zip(&wire_variants).map(|(plan, variant)| {
        let slot = &plan.slot;
        let name = &plan.wire_name;
        let seed = field_seed(root, interner, &plan.ty, &plan.attrs, plan.with_seed.as_ref(), &interner_access);
        quote! {
            #field::#variant => {
                if ::core::option::Option::is_some(&#slot) {
                    return ::core::result::Result::Err(
                        <__M::Error as #serde::de::Error>::duplicate_field(#name),
                    );
                }
                #slot = ::core::option::Option::Some(__map.next_value_seed(#seed)?);
            }
        }
    });
    let map_ignore_arm = (!deny).then(|| {
        quote! {
            #field::__ignore => {
                let _ = __map.next_value::<#serde::de::IgnoredAny>()?;
            }
        }
    });

    // Bindings extracted from the map slots, in declaration order.
    let map_bindings = plans.iter().map(|plan| {
        let binding = &plan.binding;
        if plan.attrs.skip {
            let value = skip_default_expr(&plan.attrs);
            return quote!(let #binding = #value;);
        }
        let slot = &plan.slot;
        let name = &plan.wire_name;
        let missing = quote!(return ::core::result::Result::Err(<__M::Error as #serde::de::Error>::missing_field(#name)));
        let fallback = missing_value_expr(&plan.ident, &plan.attrs, container, &container_default_binding, &missing);
        quote! {
            let #binding = match #slot {
                ::core::option::Option::Some(__v) => __v,
                ::core::option::Option::None => #fallback,
            };
        }
    });

    // Bindings extracted from an ordered sequence, in declaration order.
    let mut seq_index: usize = 0;
    let seq_bindings = plans
        .iter()
        .map(|plan| {
            let binding = &plan.binding;
            if plan.attrs.skip {
                let value = skip_default_expr(&plan.attrs);
                return quote!(let #binding = #value;);
            }
            let seed = field_seed(root, interner, &plan.ty, &plan.attrs, plan.with_seed.as_ref(), &interner_access);
            let this = proc_macro2::Literal::usize_unsuffixed(seq_index);
            seq_index += 1;
            let missing = quote!(return ::core::result::Result::Err(<__A::Error as #serde::de::Error>::invalid_length(#this, &self)));
            let fallback = missing_value_expr(&plan.ident, &plan.attrs, container, &container_default_binding, &missing);
            quote! {
                let #binding = match __seq.next_element_seed(#seed)? {
                    ::core::option::Option::Some(__v) => __v,
                    ::core::option::Option::None => #fallback,
                };
            }
        })
        .collect::<Vec<_>>();

    // After the ordered fields are read, any further sequence element is a
    // length mismatch: reject it explicitly so the arity contract holds for
    // every `SeqAccess`, not only formats that pre-check tuple/struct length.
    let seq_trailing = proc_macro2::Literal::usize_unsuffixed(seq_index + 1);
    let seq_trailing_guard = quote! {
        if ::core::option::Option::is_some(&__seq.next_element::<#serde::de::IgnoredAny>()?) {
            return ::core::result::Result::Err(<__A::Error as #serde::de::Error>::invalid_length(#seq_trailing, &self));
        }
    };

    let field_idents: Vec<&syn::Ident> = plans.iter().map(|plan| &plan.ident).collect();
    let bindings: Vec<&syn::Ident> = plans.iter().map(|plan| &plan.binding).collect();

    Ok(quote! {
        #(#with_seed_defs)*

        #[allow(non_camel_case_types)]
        enum #field {
            #(#wire_variants,)*
            #ignore_variant
        }

        struct #field_visitor;
        impl<'de> #serde::de::Visitor<'de> for #field_visitor {
            type Value = #field;
            fn expecting(&self, __f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                __f.write_str("field identifier")
            }
            fn visit_u64<__E: #serde::de::Error>(self, __v: u64) -> ::core::result::Result<Self::Value, __E> {
                ::core::result::Result::Ok(match __v {
                    #(#index_lits => #field::#wire_variants,)*
                    #unknown_u64,
                })
            }
            fn visit_str<__E: #serde::de::Error>(self, __v: &str) -> ::core::result::Result<Self::Value, __E> {
                ::core::result::Result::Ok(match __v {
                    #(#accepted_names => #field::#accepted_variants,)*
                    #unknown_str,
                })
            }
            fn visit_bytes<__E: #serde::de::Error>(self, __v: &[u8]) -> ::core::result::Result<Self::Value, __E> {
                ::core::result::Result::Ok(match __v {
                    #(#accepted_names_bytes => #field::#accepted_variants,)*
                    #unknown_bytes,
                })
            }
        }
        impl<'de> #serde::Deserialize<'de> for #field {
            fn deserialize<__D>(__d: __D) -> ::core::result::Result<Self, __D::Error>
            where
                __D: #serde::Deserializer<'de>,
            {
                __d.deserialize_identifier(#field_visitor)
            }
        }

        struct #visitor<'a, #interner: #root::__private::Lexicon + ?Sized> {
            __internity_interner: &'a mut #interner,
        }

        impl<'de, 'a, #interner: #root::__private::Lexicon + ?Sized> #serde::de::Visitor<'de> for #visitor<'a, #interner> {
            type Value = #ident;

            fn expecting(&self, __f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                __f.write_str(#expecting)
            }

            fn visit_seq<__A>(mut self, mut __seq: __A) -> ::core::result::Result<Self::Value, __A::Error>
            where
                __A: #serde::de::SeqAccess<'de>,
            {
                #container_default_stmt
                #(#seq_bindings)*
                #seq_trailing_guard
                ::core::result::Result::Ok(#ident { #(#field_idents: #bindings,)* })
            }

            fn visit_map<__M>(mut self, mut __map: __M) -> ::core::result::Result<Self::Value, __M::Error>
            where
                __M: #serde::de::MapAccess<'de>,
            {
                #(#slot_decls)*
                while let ::core::option::Option::Some(__key) = __map.next_key::<#field>()? {
                    match __key {
                        #(#map_arms)*
                        #map_ignore_arm
                    }
                }
                #container_default_stmt
                #(#map_bindings)*
                ::core::result::Result::Ok(#ident { #(#field_idents: #bindings,)* })
            }
        }

        // `__FIELDS` lists every accepted key (primary names plus `#[serde(alias
        // = "...")]` names), matching the visitor's accepted-name set so
        // schema-aware deserializers and `unknown_field` diagnostics see aliases.
        const __FIELDS: &[&str] = &[#(#accepted_names,)*];
        __deserializer.deserialize_struct(
            #struct_name,
            __FIELDS,
            #visitor { __internity_interner },
        )
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "cohesive field-identifier and tuple visitor codegen kept together"
)]
pub(crate) fn expand_tuple(
    ctx: &Ctx<'_>,
    fields: &syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
) -> syn::Result<TokenStream2> {
    let Ctx {
        root,
        container,
        ident,
        interner,
        hygiene,
    } = *ctx;
    let serde = quote!(#root::__private::serde);

    let mut plans = Vec::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        let attrs = parse_field(&field.attrs)?;
        reject_conflicting_deserialize_modes(field, &attrs)?;
        let with_seed = attrs
            .with
            .as_ref()
            .map(|_| fresh_ident(hygiene, &format!("__InternityWithSeed{index}For{}", ident.unraw())));
        let serialize_with_adapter = attrs
            .serialize_with
            .as_ref()
            .map(|_| fresh_ident(hygiene, &format!("__InternitySerializeWith{index}For{}", ident.unraw())));
        plans.push(TuplePlan {
            index: syn::Index::from(index),
            ty: field.ty.clone(),
            attrs,
            with_seed,
            serialize_with_adapter,
            binding: fresh_ident(hygiene, &format!("__value{index}")),
        });
    }

    let with_seed_defs: Vec<TokenStream2> = plans
        .iter()
        .filter_map(|plan| {
            plan.with_seed
                .as_ref()
                .and_then(|name| with_seed_def(root, name, &plan.ty, &plan.attrs))
        })
        .collect();

    let total = plans.len();
    let wire_count = plans.iter().filter(|plan| !plan.attrs.skip).count();
    let interner_access = quote!(self.__internity_interner);

    if container.default.is_none() {
        let mut defaulted = None;
        for (index, plan) in plans.iter().enumerate() {
            if plan.attrs.skip {
                continue;
            }
            if plan.attrs.default.is_some() {
                defaulted.get_or_insert(index);
            } else if let Some(previous) = defaulted {
                return Err(Error::new_spanned(
                    &fields[index],
                    format!("field must have #[serde(default)] because previous field {previous} has #[serde(default)]"),
                ));
            }
        }
    }

    let expecting = container.expecting.clone().unwrap_or_else(|| format!("tuple struct {ident}"));
    let visitor = fresh_ident(hygiene, &format!("__InternityVisitorFor{}", ident.unraw()));
    let struct_name = container_name(ident, container.rename.as_deref());

    let container_default_binding = quote!(__container_default);
    let needs_container_default = container.default.is_some() && plans.iter().any(|plan| !plan.attrs.skip && plan.attrs.default.is_none());
    let container_default_stmt = if needs_container_default {
        let ctor = match container.default.as_ref().expect("guarded by needs_container_default") {
            DefaultValue::Trait => quote!(<#ident as ::core::default::Default>::default()),
            DefaultValue::Path(path) => quote!(#path()),
        };
        quote!(let #container_default_binding: #ident = #ctor;)
    } else {
        quote!()
    };

    if container.transparent {
        let plan = plans.first().expect("transparent tuple validation requires one field");
        if plan.attrs.skip || plan.attrs.default.is_some() || is_phantom_data(&plan.ty) {
            return Err(Error::new_spanned(
                ident,
                "serde `transparent` requires exactly one field that is not skipped, defaulted, or PhantomData",
            ));
        }
        let top_access = quote!(__internity_interner);
        let seed = field_seed(root, interner, &plan.ty, &plan.attrs, plan.with_seed.as_ref(), &top_access);
        return Ok(quote! {
            #(#with_seed_defs)*
            let _ = &__internity_interner;
            let __value0 = #serde::de::DeserializeSeed::deserialize(#seed, __deserializer)?;
            ::core::result::Result::Ok(#ident(__value0))
        });
    }

    let mut seq_index: usize = 0;
    let seq_bindings = plans
        .iter()
        .map(|plan| {
            let binding = &plan.binding;
            if plan.attrs.skip {
                let value = skip_default_expr(&plan.attrs);
                return quote!(let #binding = #value;);
            }
            let seed = field_seed(root, interner, &plan.ty, &plan.attrs, plan.with_seed.as_ref(), &interner_access);
            let this = proc_macro2::Literal::usize_unsuffixed(seq_index);
            seq_index += 1;
            let hard_missing = quote!(return ::core::result::Result::Err(
                <__A::Error as #serde::de::Error>::invalid_length(#this, &self)
            ));
            let missing = tuple_missing_value_expr(&plan.index, &plan.attrs, container, &container_default_binding, &hard_missing);
            quote! {
                let #binding = match __seq.next_element_seed(#seed)? {
                    ::core::option::Option::Some(__v) => __v,
                    ::core::option::Option::None => #missing,
                };
            }
        })
        .collect::<Vec<_>>();

    // Reject sequence elements beyond the tuple's arity so every `SeqAccess`
    // enforces the length, not only formats that pre-check it.
    let seq_trailing = proc_macro2::Literal::usize_unsuffixed(seq_index + 1);
    let seq_trailing_guard = quote! {
        if ::core::option::Option::is_some(&__seq.next_element::<#serde::de::IgnoredAny>()?) {
            return ::core::result::Result::Err(<__A::Error as #serde::de::Error>::invalid_length(#seq_trailing, &self));
        }
    };

    let bindings: Vec<&syn::Ident> = plans.iter().map(|plan| &plan.binding).collect();

    // A single-field tuple struct is a newtype and additionally accepts the
    // newtype representation (unless its sole field is skipped).
    let is_newtype = total == 1 && wire_count == 1;
    let newtype = is_newtype.then(|| {
        let plan = &plans[0];
        let seed = field_seed(root, interner, &plan.ty, &plan.attrs, plan.with_seed.as_ref(), &interner_access);
        quote! {
            fn visit_newtype_struct<__D>(mut self, __d: __D) -> ::core::result::Result<Self::Value, __D::Error>
            where
                __D: #serde::Deserializer<'de>,
            {
                let __value0 = #serde::de::DeserializeSeed::deserialize(#seed, __d)?;
                ::core::result::Result::Ok(#ident(__value0))
            }
        }
    });

    let entry = if is_newtype {
        quote! {
            __deserializer.deserialize_newtype_struct(#struct_name, #visitor { __internity_interner })
        }
    } else {
        quote! {
            __deserializer.deserialize_tuple_struct(#struct_name, #wire_count, #visitor { __internity_interner })
        }
    };

    Ok(quote! {
        #(#with_seed_defs)*

        struct #visitor<'a, #interner: #root::__private::Lexicon + ?Sized> {
            __internity_interner: &'a mut #interner,
        }

        impl<'de, 'a, #interner: #root::__private::Lexicon + ?Sized> #serde::de::Visitor<'de> for #visitor<'a, #interner> {
            type Value = #ident;

            fn expecting(&self, __f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                __f.write_str(#expecting)
            }

            #newtype

            fn visit_seq<__A>(mut self, mut __seq: __A) -> ::core::result::Result<Self::Value, __A::Error>
            where
                __A: #serde::de::SeqAccess<'de>,
            {
                #container_default_stmt
                #(#seq_bindings)*
                #seq_trailing_guard
                ::core::result::Result::Ok(#ident(#(#bindings,)*))
            }
        }

        #entry
    })
}

pub(crate) fn expand_unit(ctx: &Ctx<'_>) -> TokenStream2 {
    let Ctx {
        root,
        container,
        ident,
        hygiene,
        ..
    } = *ctx;
    let serde = quote!(#root::__private::serde);
    let expecting = container.expecting.clone().unwrap_or_else(|| format!("unit struct {ident}"));
    let struct_name = container_name(ident, container.rename.as_deref());
    let visitor = fresh_ident(hygiene, &format!("__InternityVisitorFor{}", ident.unraw()));

    quote! {
        struct #visitor;

        impl<'de> #serde::de::Visitor<'de> for #visitor {
            type Value = #ident;

            fn expecting(&self, __f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                __f.write_str(#expecting)
            }

            fn visit_unit<__E>(self) -> ::core::result::Result<Self::Value, __E>
            where
                __E: #serde::de::Error,
            {
                ::core::result::Result::Ok(#ident)
            }
        }

        let _ = __internity_interner;
        __deserializer.deserialize_unit_struct(#struct_name, #visitor)
    }
}
