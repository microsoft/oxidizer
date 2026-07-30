// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashSet;

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::ext::IdentExt as _;
use syn::{Data, DeriveInput, Error, Fields, Path, Type};

use crate::attrs::{ContainerAttrs, DefaultValue, FieldAttrs};

/// Shared expansion context: the resolved `internity::de` root, parsed container
/// attributes, the target type name, and the generated interner generic name.
#[derive(Clone, Copy)]
pub(crate) struct Ctx<'a> {
    pub(crate) root: &'a Path,
    pub(crate) container: &'a ContainerAttrs,
    pub(crate) ident: &'a syn::Ident,
    pub(crate) interner: &'a syn::Ident,
    pub(crate) hygiene: &'a HashSet<String>,
}

/// Shared serialization expansion context.
#[derive(Clone, Copy)]
pub(crate) struct SerializeCtx<'a> {
    pub(crate) root: &'a Path,
    pub(crate) container: &'a ContainerAttrs,
    pub(crate) ident: &'a syn::Ident,
    pub(crate) hygiene: &'a HashSet<String>,
}

/// The seed used to deserialize one field's value.
///
/// * `via_serde` fields use the stateless [`DeserializeSeed`] wrapper.
/// * `with` / `deserialize_with` fields use a generated seed calling the named
///   function; its unit-struct name is `with_seed`.
/// * all other fields thread the interner through [`DeserializeInSeed`], reaching
///   it through `interner_access` (e.g. `self.__internity_interner` inside a
///   visitor, or `__internity_interner` at the top level).
pub(crate) fn field_seed(
    root: &Path,
    interner: &syn::Ident,
    ty: &Type,
    attrs: &FieldAttrs,
    with_seed: Option<&syn::Ident>,
    interner_access: &TokenStream2,
) -> TokenStream2 {
    if attrs.via_serde {
        quote!(#root::DeserializeSeed::<#ty>::new())
    } else if let Some(with_seed) = with_seed {
        quote!(#with_seed)
    } else {
        quote!(#root::DeserializeInSeed::<#ty, #interner>::new(&mut *#interner_access))
    }
}

/// The generated unit-struct seed definition for a `with` / `deserialize_with`
/// field, or `None` when the field uses no custom function.
pub(crate) fn with_seed_def(root: &Path, name: &syn::Ident, ty: &Type, attrs: &FieldAttrs) -> Option<TokenStream2> {
    let path = attrs.with.as_ref()?;
    let serde = quote!(#root::__private::serde);
    Some(quote! {
        struct #name;
        impl<'de> #serde::de::DeserializeSeed<'de> for #name {
            type Value = #ty;
            fn deserialize<__D>(self, __d: __D) -> ::core::result::Result<Self::Value, __D::Error>
            where
                __D: #serde::Deserializer<'de>,
            {
                #path(__d)
            }
        }
    })
}

/// The expression producing a field's value when it is absent from the input:
/// its own `default`, else the container `default`'s field, else a hard error
/// built by `missing`.
pub(crate) fn missing_value_expr(
    ident: &syn::Ident,
    attrs: &FieldAttrs,
    container: &ContainerAttrs,
    container_default_binding: &TokenStream2,
    missing: &TokenStream2,
) -> TokenStream2 {
    match &attrs.default {
        Some(DefaultValue::Trait) => quote!(::core::default::Default::default()),
        Some(DefaultValue::Path(path)) => quote!(#path()),
        None if container.default.is_some() => quote!(#container_default_binding.#ident),
        None => missing.clone(),
    }
}

pub(crate) fn tuple_missing_value_expr(
    index: &syn::Index,
    attrs: &FieldAttrs,
    container: &ContainerAttrs,
    container_default_binding: &TokenStream2,
    missing: &TokenStream2,
) -> TokenStream2 {
    match &attrs.default {
        Some(default) => default_value_expr(default),
        None if container.default.is_some() => quote!(#container_default_binding.#index),
        None => missing.clone(),
    }
}

/// The value used for a `#[serde(skip)]` field: its own `default = "path"` when
/// present, otherwise `Default::default()`. Container `default` never applies to
/// skipped fields, matching Serde.
pub(crate) fn skip_default_expr(attrs: &FieldAttrs) -> TokenStream2 {
    if let Some(DefaultValue::Path(path)) = &attrs.default {
        quote!(#path())
    } else {
        quote!(::core::default::Default::default())
    }
}

pub(crate) fn default_value_expr(default: &DefaultValue) -> TokenStream2 {
    match default {
        DefaultValue::Trait => quote!(::core::default::Default::default()),
        DefaultValue::Path(path) => quote!(#path()),
    }
}

pub(crate) fn container_name(ident: &syn::Ident, rename: Option<&str>) -> String {
    rename.map_or_else(|| ident.unraw().to_string(), str::to_owned)
}

pub(crate) fn is_phantom_data(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    path.qself.is_none() && path.path.segments.last().is_some_and(|segment| segment.ident == "PhantomData")
}

pub(crate) fn transparent_deserialize_target(plan: &NamedPlan) -> bool {
    !plan.attrs.skip && plan.attrs.default.is_none() && !is_phantom_data(&plan.ty)
}

pub(crate) fn transparent_serialize_target(plan: &NamedPlan) -> bool {
    !plan.attrs.skip_serializing && !is_phantom_data(&plan.ty)
}

pub(crate) fn transparent_other_expr(plan: &NamedPlan) -> TokenStream2 {
    if plan.attrs.skip {
        skip_default_expr(&plan.attrs)
    } else if let Some(default) = &plan.attrs.default {
        default_value_expr(default)
    } else {
        quote!(::core::default::Default::default())
    }
}

pub(crate) fn validate_transparent_container(input: &DeriveInput, container: &ContainerAttrs) -> syn::Result<()> {
    if !container.transparent {
        return Ok(());
    }
    let Data::Struct(data) = &input.data else {
        return Err(Error::new_spanned(input, "serde `transparent` is supported only for structs"));
    };
    match &data.fields {
        Fields::Named(_) => Ok(()),
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => Ok(()),
        Fields::Unnamed(_) | Fields::Unit => Err(Error::new_spanned(
            &input.ident,
            "serde `transparent` requires a single-field tuple struct or a struct with exactly one transparent field",
        )),
    }
}

pub(crate) struct NamedPlan {
    pub(crate) ident: syn::Ident,
    pub(crate) ty: Type,
    pub(crate) attrs: FieldAttrs,
    pub(crate) wire_name: String,
    pub(crate) serialize_wire_name: String,
    pub(crate) with_seed: Option<syn::Ident>,
    pub(crate) serialize_with_adapter: Option<syn::Ident>,
    pub(crate) binding: syn::Ident,
    pub(crate) slot: syn::Ident,
}

pub(crate) struct TuplePlan {
    pub(crate) index: syn::Index,
    pub(crate) ty: Type,
    pub(crate) attrs: FieldAttrs,
    pub(crate) with_seed: Option<syn::Ident>,
    pub(crate) serialize_with_adapter: Option<syn::Ident>,
    pub(crate) binding: syn::Ident,
}

pub(crate) fn serialize_with_adapter_def(root: &Path, name: &syn::Ident, ty: &Type, path: &Path) -> TokenStream2 {
    let serde = quote!(#root::__private::serde);
    quote! {
        struct #name<'a>(&'a #ty);
        impl<'a> #serde::Serialize for #name<'a> {
            fn serialize<__S>(&self, __serializer: __S) -> ::core::result::Result<__S::Ok, __S::Error>
            where
                __S: #serde::Serializer,
            {
                #path(self.0, __serializer)
            }
        }
    }
}

/// Rejects a field that requests both `#[internity(via_serde)]` (deserialize
/// through ordinary Serde) and a custom `deserialize_with`/`with` deserializer:
/// the two disagree on how to deserialize the field. This validation is
/// deserialize-directional — the serialize expander does not enforce it — so a
/// field pairing `via_serde` with a deserialize-only `deserialize_with` still
/// produces a valid `SerializeIn` schema.
pub(crate) fn reject_conflicting_deserialize_modes<T: quote::ToTokens>(spanned: T, attrs: &FieldAttrs) -> syn::Result<()> {
    if attrs.via_serde && attrs.with.is_some() {
        return Err(Error::new_spanned(
            &spanned,
            "`#[internity(via_serde)]` and serde `deserialize_with`/`with` are mutually exclusive for `DeserializeIn`",
        ));
    }
    if attrs.skip && (attrs.via_serde || attrs.with.is_some()) {
        return Err(Error::new_spanned(
            &spanned,
            "serde `skip`/`skip_deserializing` cannot be combined with a custom deserializer mode \
             (`via_serde`/`deserialize_with`/`with`)",
        ));
    }
    Ok(())
}

/// The serialize counterpart of [`reject_conflicting_deserialize_modes`]:
/// `#[internity(via_serde)]` serializes through ordinary Serde, so a custom
/// `serialize_with`/`with` serializer contradicts it. This validation is
/// serialize-directional so a deserialize-only `deserialize_with` never trips it.
pub(crate) fn reject_conflicting_serialize_modes<T: quote::ToTokens>(spanned: T, attrs: &FieldAttrs) -> syn::Result<()> {
    if attrs.via_serde && attrs.serialize_with.is_some() {
        return Err(Error::new_spanned(
            spanned,
            "`#[internity(via_serde)]` and serde `serialize_with`/`with` are mutually exclusive for `SerializeIn`",
        ));
    }
    Ok(())
}

/// Rejects serde `skip_serializing_if` on a field that `SerializeIn` would emit.
/// A runtime skip predicate cannot be honored without diverging from the type's
/// ordinary Serde wire schema, so — like `flatten`/`borrow` — it is refused rather
/// than silently ignored. Fields already dropped via `skip_serializing` are exempt.
pub(crate) fn reject_skip_serializing_if<T: quote::ToTokens>(spanned: T, attrs: &FieldAttrs) -> syn::Result<()> {
    if attrs.skip_serializing_if && !attrs.skip_serializing {
        return Err(Error::new_spanned(
            spanned,
            "internity::SerializeIn does not support serde `skip_serializing_if`: a runtime skip \
             predicate would diverge from the type's ordinary Serde wire schema. Remove the \
             attribute, or use `#[serde(skip_serializing)]` to always omit the field.",
        ));
    }
    Ok(())
}

pub(crate) fn serialize_field_expr(root: &Path, access: &TokenStream2, attrs: &FieldAttrs, adapter: Option<&syn::Ident>) -> TokenStream2 {
    if let Some(adapter) = adapter {
        quote!(#adapter(&self.#access))
    } else if attrs.via_serde {
        quote!(&self.#access)
    } else {
        quote!(#root::__private::SerializeInWith::new(&self.#access, __reader))
    }
}

pub(crate) fn serialize_direct_call(root: &Path, serde: &TokenStream2, access: &TokenStream2, attrs: &FieldAttrs) -> TokenStream2 {
    if let Some(path) = &attrs.serialize_with {
        quote!(#path(&self.#access, __serializer))
    } else if attrs.via_serde {
        quote!(#serde::Serialize::serialize(&self.#access, __serializer))
    } else {
        quote!(#root::__private::SerializeIn::serialize_in(&self.#access, __reader, __serializer))
    }
}
