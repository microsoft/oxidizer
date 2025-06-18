// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::spanned::Spanned;
use syn::{
    Attribute, Fields, FieldsNamed, Generics, Ident, ItemStruct, Path, Type, TypePath,
    TypeReference, parse_quote,
};

use crate::api::options::{FieldOptions, StructCategory, StructOptions};
use crate::syn_helpers::extract_inner_generic_type;

pub fn core_struct(attr: TokenStream, item: ItemStruct) -> super::Result<TokenStream> {
    let struct_options = StructOptions::parse(attr)?;

    match struct_options.category {
        StructCategory::BehavioralType => behavioral_type(item, struct_options),
        StructCategory::ValueObject => value_object(item, struct_options),
        StructCategory::DataTransferObject => data_transfer_object(item, struct_options),
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "Convention for syn-based code"
)]
fn behavioral_type(item: ItemStruct, _options: StructOptions) -> super::Result<TokenStream> {
    // For behavioral types we simply check the input but do not replace it - as long as it
    // conforms to expectations, we pass it through. Future versions may perform more processing.
    validate_is_public(&item)?;
    visit_fields(&item, |_| Ok(()))?; // As long as the fields are accessible, all is well.

    Ok(item.to_token_stream())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "Convention for syn-based code"
)]
fn value_object(item: ItemStruct, _options: StructOptions) -> super::Result<TokenStream> {
    // For value objects we simply check the input but do not replace it - as long as it
    // conforms to expectations, we pass it through. Future versions may perform more processing.
    validate_is_public(&item)?;
    visit_fields(&item, |_| Ok(()))?; // As long as the fields are accessible, all is well.

    Ok(item.to_token_stream())
}

#[derive(Debug)]
struct DtoNames {
    full: Ident,
    inner: Ident,
    builder: Ident,
}

impl DtoNames {
    fn new(ident: &Ident) -> Self {
        Self {
            full: ident.clone(),
            inner: format_ident!("{}Inner", ident),
            builder: format_ident!("{}Builder", ident),
        }
    }
}

#[derive(Debug)]
struct Dto {
    names: DtoNames,

    generics: Generics,

    // The attributes that were on the input struct, except for the one that triggered this macro.
    // All of them will be copy-pasted onto the "full" generated type. Most importantly, this is
    // where all the documentation comments on the type go (they become attributes).
    struct_attrs: Vec<Attribute>,

    // Guaranteed to be sorted by field name to ensure that everything we do with the fields has
    // a consistent order that does not depend on the order they had in the template type.
    sorted_fields: Vec<DtoField>,
}

#[derive(Debug)]
struct DtoField {
    ident: Ident,

    is_optional: bool,

    // The field when emitted in the "full" generated type,
    // used for serialization (if enabled) and for the public getter.
    as_full_field: TokenStream,

    // The field when emitted in the "inner" generated type used for deserialization (if enabled).
    // Value is always present, even if deserialization is disabled.
    as_inner_field: TokenStream,

    // The field when received via constructor parameters in the builder, used for receiving
    // mandatory fields. Value always present, even if this is not a mandatory field.
    as_ctor_parameter: TokenStream,

    // The assignment from a field of the inner type to a field of the full type, used during
    // deserialization (if enabled). Value always present, even if deserialization is disabled.
    assign_full_from_inner: TokenStream,

    // The assignment from a ctor parameter to a field of the builder, used during construction
    // if this is a mandatory field. Value always present, even if this is not a mandatory field.
    assign_builder_from_ctor: TokenStream,

    // A public getter implementation used to read the value from the field.
    getter: TokenStream,
}

impl Dto {
    fn parse(item: &ItemStruct, struct_options: &StructOptions) -> super::Result<Self> {
        let struct_attrs = item.attrs.clone();
        let mut fields = Vec::new();

        for attr in &item.attrs {
            if attr.path().is_ident("doc") {
                // Doc attributes are what doc comments are transformed into, so they are fine.
                continue;
            }

            // We do not support any custom attributes - everything must be configured via the
            // public API attribute. There is intentionally no out of band customization support
            // as this macro is intended to be exhaustive and we want to prevent snowflake types.
            return Err(syn::Error::new(
                attr.span(),
                "You cannot apply custom attributes to data transfer objects that are part of the public API.",
            ));
        }

        validate_is_public(item)?;

        visit_fields(item, |field| {
            fields.push(DtoField::parse(field, struct_options)?);
            Ok(())
        })?;

        // We guarantee that these are sorted, so we can rely on the order when generating code.
        fields.sort_unstable_by(|a, b| a.ident.cmp(&b.ident));

        Ok(Self {
            struct_attrs,
            sorted_fields: fields,
            generics: item.generics.clone(),
            names: DtoNames::new(&item.ident),
        })
    }
}

#[derive(Debug, Eq, PartialEq)]
enum DtoFieldKind {
    StaticRef,
    Option,
    Vec,
    String,
    Other,
}

impl DtoFieldKind {
    fn parse(raw: &Type) -> super::Result<Self> {
        match raw {
            Type::Path(TypePath { path, .. }) => Self::parse_from_path(path),
            Type::Reference(reference) => Self::parse_from_reference(reference),
            _ => Err(syn::Error::new(
                raw.span(),
                "field has unsupported type - only owned types and shared 'static references are supported in data transfer object fields",
            )),
        }
    }

    fn parse_from_path(path: &Path) -> super::Result<Self> {
        let Some(first_segment) = path.segments.first() else {
            // Unsure what situation could bring us here - figure out how to handle this
            // when we first encounter it, if it is even possible in legit scenarios.
            return Err(syn::Error::new(
                path.span(),
                "field has unsupported type - no path segments found in type path",
            ));
        };

        Ok(match first_segment.ident.to_string().as_str() {
            "Option" if !first_segment.arguments.is_empty() => Self::Option,
            "Vec" if !first_segment.arguments.is_empty() => Self::Vec,
            "String" => Self::String,
            _ => Self::Other,
        })
    }

    fn parse_from_reference(reference: &TypeReference) -> super::Result<Self> {
        if reference.mutability.is_some() {
            return Err(syn::Error::new(
                reference.span(),
                "field has unsupported type - exclusive (mut) references are not supported in data transfer object fields",
            ));
        }

        if reference
            .lifetime
            .as_ref()
            .is_none_or(|lifetime| lifetime.ident != "static")
        {
            return Err(syn::Error::new(
                reference.span(),
                "field has unsupported type - only shared 'static references are supported in data transfer object fields",
            ));
        }

        Ok(Self::StaticRef)
    }
}

impl DtoField {
    fn parse(field: &syn::Field, struct_options: &StructOptions) -> super::Result<Self> {
        let field_ident = field.ident.as_ref().unwrap();

        // We accept fields with two categories of types:
        // * Type::Path which is an owned and potentially generic type like `A::B<C>`.
        // * Type::Reference which is a reference type like `&'a A::B<C>`.
        //     We only support shared (non-mut) references with 'static lifetime.
        //     Use of references is not compatible with the "config" flag because data with
        //     static lifetime cannot be loaded from config (nothing would own the values).
        //
        // We only support one layer of reference, with the type of the referenced item being
        // a Type::Path. When we process it, we therefore reduce the reference-ness down to a bool.
        let field_kind = DtoFieldKind::parse(&field.ty)?;

        if field_kind == DtoFieldKind::StaticRef && struct_options.config {
            return Err(syn::Error::new(
                field.span(),
                "data transfer objects that contain references are not compatible with the 'config' flag",
            ));
        }

        // If the field type is Option<T> then we treat it as a special case: it is optional
        // and has a default value of None even if the field is not marked as optional.
        // We only support the form "Option<T>" and not "std::option::Option<T>
        let field_is_option = field_kind == DtoFieldKind::Option;

        // Fields can be optional even if they are not Option (if an attribute says so).
        let mut field_is_optional = field_is_option;

        // We copy-paste any field documentation to the getters/setters in the generated types.
        let field_doc_attrs = field
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("doc"))
            .collect::<Vec<_>>();

        let mut field_options = FieldOptions::default();

        for attr in &field.attrs {
            if attr.path().is_ident("doc") {
                // Doc attributes are what doc comments are transformed into, so they are fine.
                continue;
            }

            // We do not support any custom attributes - everything must be configured via
            // our own #[field]. There is intentionally no out of band customization support as
            // this macro is intended to be exhaustive and we want to prevent snowflake types.
            if !attr.path().is_ident("field") {
                return Err(syn::Error::new(
                    attr.span(),
                    "Only the #[field] attribute may be used in #[oxidizer_api_lifecycle::api] structs.",
                ));
            }

            field_options = FieldOptions::parse(attr)?;
        }

        if field_options.optional {
            // A field can be marked optional for multiple reasons:
            // * It is an Option<T> field.
            // * It is a regular field marked as optional via #[field(optional)].
            field_is_optional = true;
        }

        let full_fields_builder_args = if field_is_optional {
            // If the field is an Option<T>, the builder will just take the T as input.
            // Otherwise, the builder takes the regular type as input.
            if field_is_option {
                quote! { #[builder(setter(strip_option), default)] }
            } else {
                quote! { #[builder(default)] }
            }
        } else {
            // Mandatory fields are already set in the builder constructor, so suppress the
            // setter generation for them.
            quote! { #[builder(setter(custom))] }
        };

        let serde_default = if field_is_optional && struct_options.config {
            quote! { #[serde(default)] }
        } else {
            TokenStream::new()
        };

        let field_type_raw = &field.ty;

        let as_full_field = quote! {
            #(#field_doc_attrs)*
            #full_fields_builder_args
            #serde_default
            #field_ident: #field_type_raw,
        };

        let as_ctor_parameter = quote! { #field_ident: #field_type_raw };

        let as_inner_field = quote! {
            #serde_default
            #field_ident: #field_type_raw,
        };

        let assign_full_from_inner = quote! { #field_ident: inner.#field_ident, };

        let assign_builder_from_ctor = quote! { #field_ident: Some(#field_ident), };

        let getter_fn = emit_getter_fn(field, &field_options, &field_kind);

        let getter = quote! {
            #(#field_doc_attrs)*
            #getter_fn
        };

        Ok(Self {
            ident: field_ident.clone(),
            is_optional: field_is_optional,
            as_full_field,
            as_inner_field,
            as_ctor_parameter,
            assign_full_from_inner,
            assign_builder_from_ctor,
            getter,
        })
    }
}

fn emit_getter_fn(
    field: &syn::Field,
    options: &FieldOptions,
    field_kind: &DtoFieldKind,
) -> TokenStream {
    let field_ident = &field.ident;
    let field_type_raw = &field.ty;

    match field_kind {
        // &T -> &T
        DtoFieldKind::StaticRef => quote! {
            pub fn #field_ident(&self) -> #field_type_raw {
                self.#field_ident
            }
        },
        // Option<T>
        // * if T is a reference (== can be copied, as we do not support mut references)
        //      -> Option<T>
        // * if `#[field(copy)]` is specified
        //      -> Option<T>
        // * Else
        //      -> Option<&T>
        DtoFieldKind::Option => {
            let inner_type = extract_inner_generic_type(field_type_raw)
                .expect("we already verified the type is Option<T> so it must have a T");

            if options.copy || matches!(inner_type, Type::Reference(_)) {
                quote! {
                    pub fn #field_ident(&self) -> #field_type_raw {
                        self.#field_ident
                    }
                }
            } else {
                quote! {
                    pub fn #field_ident(&self) -> Option<&#inner_type> {
                        self.#field_ident.as_ref()
                    }
                }
            }
        }
        // Vec<T> -> &[T]
        DtoFieldKind::Vec => {
            let inner_type = extract_inner_generic_type(field_type_raw)
                .expect("we already verified the type is Vec<T> so it must have a T");

            quote! {
                pub fn #field_ident(&self) -> &[#inner_type] {
                    self.#field_ident.as_slice()
                }
            }
        }
        // String -> &str
        DtoFieldKind::String => quote! {
            pub fn #field_ident(&self) -> &str {
                self.#field_ident.as_str()
            }
        },
        // This is the default case, for types that we have no specialization for.
        // Depending on #[field(copy)] presence this can be:
        // * T -> &T
        // * T -> T
        DtoFieldKind::Other => {
            if options.copy {
                quote! {
                    pub fn #field_ident(&self) -> #field_type_raw {
                        self.#field_ident
                    }
                }
            } else {
                quote! {
                    pub fn #field_ident(&self) -> &#field_type_raw {
                        &self.#field_ident
                    }
                }
            }
        }
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "Convention for syn-based code"
)]
fn data_transfer_object(item: ItemStruct, options: StructOptions) -> super::Result<TokenStream> {
    let dto = Dto::parse(&item, &options)?;

    let full = emit_full(&dto, &options);
    let builder_impl = emit_builder_struct_impl(&dto, &options);
    let deserialize_impl = emit_deserialize_trait_impl(&dto, &options);

    Ok(quote! {
        #full
        #builder_impl
        #deserialize_impl
    })
}

fn emit_full(dto: &Dto, options: &StructOptions) -> TokenStream {
    let full_ident = &dto.names.full;

    let mut full_derives = Vec::new();
    full_derives.push(quote! { ::derive_builder::Builder });
    full_derives.push(quote! { ::std::fmt::Debug });

    if options.config {
        full_derives.push(quote! { ::serde::Serialize });
    }

    let full_config_attrs = if options.config {
        quote! { #[::oxidizer_config::traverse] }
    } else {
        TokenStream::new()
    };

    let full_attrs = &dto.struct_attrs;
    let full_fields = dto
        .sorted_fields
        .iter()
        .map(|x| &x.as_full_field)
        .collect::<Vec<_>>();

    let (impl_generics, type_generics, where_clause) = dto.generics.split_for_impl();

    let getters = dto
        .sorted_fields
        .iter()
        .map(|x| &x.getter)
        .collect::<Vec<_>>();

    quote! {
        #(#full_attrs)*
        #full_config_attrs
        #[derive(#(#full_derives),*)]
        #[builder(custom_constructor, build_fn(private, name = "build_core", error = "::oxidizer_api_lifecycle::__private::BuilderCoreError"))]
        pub struct #full_ident #impl_generics #where_clause {
            #(#full_fields)*
        }

        impl #impl_generics #full_ident #type_generics #where_clause {
            #(#getters)*
        }
    }
}

fn emit_builder_struct_impl(dto: &Dto, options: &StructOptions) -> TokenStream {
    let full_ident = &dto.names.full;
    let builder_ident = &dto.names.builder;

    let (impl_generics, type_generics, where_clause) = dto.generics.split_for_impl();

    let build_fn = if options.no_validation {
        quote! {
            #[allow(clippy::unnecessary_wraps)] // Builder pattern always assumes fallible builds, if not today then tomorrow.
            pub fn build(&mut self) -> ::std::result::Result<#full_ident #type_generics, ::oxidizer_api_lifecycle::validation::Error> {
                // The only reason build_core() might return an error is if a mandatory field
                // is not set. This should never happen because all mandatory fields are set via
                // the ctor parameters. If it happens, we have a logic error in the builder code.
                Ok(self.build_core()
                    .expect("impossible - all mandatory fields are specified via ctor parameters"))
            }
        }
    } else {
        quote! {
            pub fn build(&mut self) -> ::std::result::Result<#full_ident #type_generics, ::oxidizer_api_lifecycle::validation::Error> {
                // The only reason build_core() might return an error is if a mandatory field
                // is not set. This should never happen because all mandatory fields are set via
                // the ctor parameters. If it happens, we have a logic error in the builder code.
                ::oxidizer_api_lifecycle::validation::__private::Validate::validate(self.build_core()
                    .expect("impossible - all mandatory fields are specified via ctor parameters"))
            }
        }
    };

    let mandatory_fields = dto
        .sorted_fields
        .iter()
        .filter(|x| !x.is_optional)
        .collect::<Vec<_>>();

    let ctor_params = mandatory_fields
        .iter()
        .map(|x| &x.as_ctor_parameter)
        .collect::<Vec<_>>();

    let assign_builder_from_ctor = mandatory_fields
        .iter()
        .map(|x| &x.assign_builder_from_ctor)
        .collect::<Vec<_>>();

    quote! {
        impl #impl_generics #builder_ident #type_generics #where_clause {
            #build_fn

            #[allow(clippy::needless_update)] // If all fields are mandatory.
            pub fn new(#(#ctor_params),*) -> Self {
                Self {
                    #(#assign_builder_from_ctor)*
                    ..Self::create_empty()
                }
            }
        }
    }
}

fn emit_deserialize_trait_impl(dto: &Dto, options: &StructOptions) -> TokenStream {
    if !options.config {
        // A deserializer implementation exists only to facilitate loading from config,
        // so skip this if config functionality is not enabled.
        return TokenStream::new();
    }

    let full_ident = &dto.names.full;
    let inner_ident = &dto.names.inner;

    let inner_fields = dto
        .sorted_fields
        .iter()
        .map(|x| &x.as_inner_field)
        .collect::<Vec<_>>();

    let assign_full_from_inner = dto
        .sorted_fields
        .iter()
        .map(|x| &x.assign_full_from_inner)
        .collect::<Vec<_>>();

    let (_, type_generics, where_clause) = dto.generics.split_for_impl();

    // The inner type inherits the generics from the outer (impl) scope.
    let inner_def = quote! {
        #[derive(::std::fmt::Debug, ::serde::Deserialize)]
        struct #inner_ident #type_generics {
            #(#inner_fields)*
        }
    };

    let deserialize_ret = if options.no_validation {
        quote! {
            Ok(full)
        }
    } else {
        quote! {
            ::oxidizer_api_lifecycle::validation::__private::Validate::validate(full)
                .map_err(::serde::de::Error::custom)
        }
    };

    // We need to merge the 'de lifetime in front of the template type's generics.
    let params_with_lifetime = {
        let mut params = dto.generics.params.clone();
        params.insert(0, parse_quote!('de));
        params
    };

    let generics_with_lifetime_prefix = Generics {
        params: params_with_lifetime,
        ..dto.generics.clone()
    };

    let (impl_generics_with_lifetime_prefix, _, _) = generics_with_lifetime_prefix.split_for_impl();

    quote! {
        impl #impl_generics_with_lifetime_prefix ::serde::de::Deserialize<'de> for #full_ident #type_generics #where_clause {
            fn deserialize<D>(deserializer: D) -> ::std::result::Result<#full_ident #type_generics, D::Error>
            where
                D: ::serde::de::Deserializer<'de>,
            {
                #inner_def

                let inner = #inner_ident::deserialize(deserializer)?;

                let full = #full_ident {
                    #(#assign_full_from_inner)*
                };

                #deserialize_ret
            }
        }
    }
}

fn validate_is_public(item: &ItemStruct) -> super::Result<()> {
    if !matches!(item.vis, syn::Visibility::Public(..)) {
        return Err(syn::Error::new(
            item.span(),
            "Structs with #[oxidizer_api_lifecycle::api] must be public.",
        ));
    }

    Ok(())
}

fn visit_fields<V>(item: &ItemStruct, mut field_visitor: V) -> super::Result<()>
where
    V: FnMut(&syn::Field) -> super::Result<()>,
{
    if let Fields::Named(FieldsNamed { named: fields, .. }) = &item.fields {
        for field in fields {
            if field.vis != syn::Visibility::Inherited {
                return Err(syn::Error::new(
                    field.span(),
                    "All fields must be private in #[oxidizer_api_lifecycle::api] structs.",
                ));
            }

            field_visitor(field)?;
        }
    } else {
        return Err(syn::Error::new(
            item.span(),
            "You can only use #[oxidizer_api_lifecycle::api] on structs if they have named fields.",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::parse_quote;
    use syn::punctuated::Punctuated;

    use super::*;

    #[test]
    fn behavioral() {
        let input = parse_quote! {
            /// Yolo wowza.
            pub struct Foo {
                length: usize,
            }
        };

        let meta = quote! { behavioral };

        let result = core_struct(meta, input).unwrap();

        let expected = quote! {
            /// Yolo wowza.
            pub struct Foo {
                length: usize,
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn value_object() {
        let input = parse_quote! {
            /// Yolo wowza.
            pub struct Foo {
                length: usize,
            }
        };

        let meta = quote! { value };

        let result = core_struct(meta, input).unwrap();

        let expected = quote! {
            /// Yolo wowza.
            pub struct Foo {
                length: usize,
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn dto_with_config_with_validate() {
        let input = parse_quote! {
            /// Yolo wowza.
            pub struct Foo {
                /// This is a doc comment.
                length: usize,

                #[field(optional)]
                timeout_seconds: usize,

                name: Option<String>,
            }
        };

        let meta = quote! { dto, config };

        let result = core_struct(meta, input).unwrap();

        let expected = quote! {
            /// Yolo wowza.
            #[::oxidizer_config::traverse]
            #[derive(::derive_builder::Builder, ::std::fmt::Debug, ::serde::Serialize)]
            #[builder(custom_constructor, build_fn(private, name = "build_core", error = "::oxidizer_api_lifecycle::__private::BuilderCoreError"))]
            pub struct Foo {
                /// This is a doc comment.
                #[builder(setter(custom))]
                length: usize,

                #[builder(setter(strip_option), default)]
                #[serde(default)]
                name: Option<String>,

                #[builder(default)]
                #[serde(default)]
                timeout_seconds: usize,
            }

            impl Foo {
                /// This is a doc comment.
                pub fn length(&self) -> &usize {
                    &self.length
                }

                pub fn name(&self) -> Option<&String> {
                    self.name.as_ref()
                }

                pub fn timeout_seconds(&self) -> &usize {
                    &self.timeout_seconds
                }
            }

            impl FooBuilder {
                pub fn build(&mut self) -> ::std::result::Result<Foo, ::oxidizer_api_lifecycle::validation::Error> {
                    ::oxidizer_api_lifecycle::validation::__private::Validate::validate(self.build_core()
                        .expect("impossible - all mandatory fields are specified via ctor parameters"))
                }

                #[allow(clippy::needless_update)] // If all fields are mandatory.
                pub fn new(length: usize) -> Self {
                    Self {
                        length: Some(length),
                        ..Self::create_empty()
                    }
                }
            }

            impl<'de> ::serde::de::Deserialize<'de> for Foo {
                fn deserialize<D>(deserializer: D) -> ::std::result::Result<Foo, D::Error>
                where
                    D: ::serde::de::Deserializer<'de>,
                {
                    #[derive(::std::fmt::Debug, ::serde::Deserialize)]
                    struct FooInner {
                        length: usize,

                        #[serde(default)]
                        name: Option<String>,

                        #[serde(default)]
                        timeout_seconds: usize,
                    }

                    let inner = FooInner::deserialize(deserializer)?;

                    let full = Foo {
                        length: inner.length,
                        name: inner.name,
                        timeout_seconds: inner.timeout_seconds,
                    };

                    ::oxidizer_api_lifecycle::validation::__private::Validate::validate(full)
                        .map_err(::serde::de::Error::custom)
                }
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn dto_with_config_no_validate() {
        let input = parse_quote! {
            /// Yolo wowza.
            pub struct Foo {
                /// This is a doc comment.
                length: usize,

                #[field(optional)]
                timeout_seconds: usize,

                name: Option<String>,
            }
        };

        let meta = quote! { dto, config, no_validation };

        let result = core_struct(meta, input).unwrap();

        let expected = quote! {
            /// Yolo wowza.
            #[::oxidizer_config::traverse]
            #[derive(::derive_builder::Builder, ::std::fmt::Debug, ::serde::Serialize)]
            #[builder(custom_constructor, build_fn(private, name = "build_core", error = "::oxidizer_api_lifecycle::__private::BuilderCoreError"))]
            pub struct Foo {
                /// This is a doc comment.
                #[builder(setter(custom))]
                length: usize,

                #[builder(setter(strip_option), default)]
                #[serde(default)]
                name: Option<String>,

                #[builder(default)]
                #[serde(default)]
                timeout_seconds: usize,
            }

            impl Foo {
                /// This is a doc comment.
                pub fn length(&self) -> &usize {
                    &self.length
                }

                pub fn name(&self) -> Option<&String> {
                    self.name.as_ref()
                }

                pub fn timeout_seconds(&self) -> &usize {
                    &self.timeout_seconds
                }
            }

            impl FooBuilder {
                #[allow(clippy::unnecessary_wraps)]
                pub fn build(&mut self) -> ::std::result::Result<Foo, ::oxidizer_api_lifecycle::validation::Error> {
                    Ok(self.build_core()
                        .expect("impossible - all mandatory fields are specified via ctor parameters"))
                }

                #[allow(clippy::needless_update)] // If all fields are mandatory.
                pub fn new(length: usize) -> Self {
                    Self {
                        length: Some(length),
                        ..Self::create_empty()
                    }
                }
            }

            impl<'de> ::serde::de::Deserialize<'de> for Foo {
                fn deserialize<D>(deserializer: D) -> ::std::result::Result<Foo, D::Error>
                where
                    D: ::serde::de::Deserializer<'de>,
                {
                    #[derive(::std::fmt::Debug, ::serde::Deserialize)]
                    struct FooInner {
                        length: usize,

                        #[serde(default)]
                        name: Option<String>,

                        #[serde(default)]
                        timeout_seconds: usize,
                    }

                    let inner = FooInner::deserialize(deserializer)?;

                    let full = Foo {
                        length: inner.length,
                        name: inner.name,
                        timeout_seconds: inner.timeout_seconds,
                    };

                    Ok(full)
                }
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn dto_no_config_with_validate() {
        let input = parse_quote! {
            /// Yolo wowza.
            pub struct Foo {
                /// This is a doc comment.
                length: usize,

                #[field(optional)]
                timeout_seconds: usize,

                name: Option<String>,
            }
        };

        let meta = quote! { dto };

        let result = core_struct(meta, input).unwrap();

        let expected = quote! {
            /// Yolo wowza.
            #[derive(::derive_builder::Builder, ::std::fmt::Debug)]
            #[builder(custom_constructor, build_fn(private, name = "build_core", error = "::oxidizer_api_lifecycle::__private::BuilderCoreError"))]
            pub struct Foo {
                /// This is a doc comment.
                #[builder(setter(custom))]
                length: usize,

                #[builder(setter(strip_option), default)]
                name: Option<String>,

                #[builder(default)]
                timeout_seconds: usize,
            }

            impl Foo {
                /// This is a doc comment.
                pub fn length(&self) -> &usize {
                    &self.length
                }

                pub fn name(&self) -> Option<&String> {
                    self.name.as_ref()
                }

                pub fn timeout_seconds(&self) -> &usize {
                    &self.timeout_seconds
                }
            }

            impl FooBuilder {
                pub fn build(&mut self) -> ::std::result::Result<Foo, ::oxidizer_api_lifecycle::validation::Error> {
                    ::oxidizer_api_lifecycle::validation::__private::Validate::validate(self.build_core()
                        .expect("impossible - all mandatory fields are specified via ctor parameters"))
                }

                #[allow(clippy::needless_update)] // If all fields are mandatory.
                pub fn new(length: usize) -> Self {
                    Self {
                        length: Some(length),
                        ..Self::create_empty()
                    }
                }
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn dto_no_config_no_validate() {
        let input = parse_quote! {
            /// Yolo wowza.
            pub struct Foo {
                /// This is a doc comment.
                #[field(copy)]
                length: usize,

                #[field(optional, copy)]
                timeout_seconds: usize,

                name: Option<String>,

                title: Option<&'static str>,
            }
        };

        let meta = quote! { dto, no_validation };

        let result = core_struct(meta, input).unwrap();

        let expected = quote! {
            /// Yolo wowza.
            #[derive(::derive_builder::Builder, ::std::fmt::Debug)]
            #[builder(custom_constructor, build_fn(private, name = "build_core", error = "::oxidizer_api_lifecycle::__private::BuilderCoreError"))]
            pub struct Foo {
                /// This is a doc comment.
                #[builder(setter(custom))]
                length: usize,

                #[builder(setter(strip_option), default)]
                name: Option<String>,

                #[builder(default)]
                timeout_seconds: usize,

                #[builder(setter(strip_option), default)]
                title: Option<&'static str>,
            }

            impl Foo {
                /// This is a doc comment.
                pub fn length(&self) -> usize {
                    self.length
                }

                pub fn name(&self) -> Option<&String> {
                    self.name.as_ref()
                }

                pub fn timeout_seconds(&self) -> usize {
                    self.timeout_seconds
                }

                pub fn title(&self) -> Option<&'static str> {
                    self.title
                }
            }

            impl FooBuilder {
                #[allow(clippy::unnecessary_wraps)]
                pub fn build(&mut self) -> ::std::result::Result<Foo, ::oxidizer_api_lifecycle::validation::Error> {
                    Ok(self.build_core()
                        .expect("impossible - all mandatory fields are specified via ctor parameters"))
                }

                #[allow(clippy::needless_update)] // If all fields are mandatory.
                pub fn new(length: usize) -> Self {
                    Self {
                        length: Some(length),
                        ..Self::create_empty()
                    }
                }
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn dto_no_config_no_validate_with_generics() {
        let input = parse_quote! {
            pub struct Foo<A: Clone, B>
            where
                B: Display
            {
                mandatory: Vec<A>,
                optional: Option<Vec<B>>,
            }
        };

        let meta = quote! { dto, no_validation };

        let result = core_struct(meta, input).unwrap();

        let expected = quote! {
            #[derive(::derive_builder::Builder, ::std::fmt::Debug)]
            #[builder(custom_constructor, build_fn(private, name = "build_core", error = "::oxidizer_api_lifecycle::__private::BuilderCoreError"))]
            pub struct Foo<A: Clone, B>
            where
                B: Display
            {
                #[builder(setter(custom))]
                mandatory: Vec<A>,

                #[builder(setter(strip_option), default)]
                optional: Option< Vec<B> >,
            }

            impl<A: Clone, B> Foo<A, B>
            where
                B: Display
            {
                pub fn mandatory(&self) -> &[A] {
                    self.mandatory.as_slice()
                }

                pub fn optional(&self) -> Option< &Vec<B> > {
                    self.optional.as_ref()
                }
            }

            impl<A: Clone, B> FooBuilder<A, B>
            where
                B: Display
            {
                #[allow(clippy::unnecessary_wraps)]
                pub fn build(&mut self) -> ::std::result::Result<Foo<A, B>, ::oxidizer_api_lifecycle::validation::Error> {
                    Ok(self.build_core()
                        .expect("impossible - all mandatory fields are specified via ctor parameters"))
                }

                #[allow(clippy::needless_update)] // If all fields are mandatory.
                pub fn new(mandatory: Vec<A>) -> Self {
                    Self {
                        mandatory: Some(mandatory),
                        ..Self::create_empty()
                    }
                }
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn dto_with_config_with_validate_with_generics() {
        let input = parse_quote! {
            pub struct Foo<A: Clone, B>
            where
                B: Display
            {
                mandatory: Vec<A>,
                optional: Option<Vec<B>>,
            }
        };

        let meta = quote! { dto, config };

        let result = core_struct(meta, input).unwrap();

        let expected = quote! {
            #[::oxidizer_config::traverse]
            #[derive(::derive_builder::Builder, ::std::fmt::Debug, ::serde::Serialize)]
            #[builder(custom_constructor, build_fn(private, name = "build_core", error = "::oxidizer_api_lifecycle::__private::BuilderCoreError"))]
            pub struct Foo<A: Clone, B>
            where
                B: Display
            {
                #[builder(setter(custom))]
                mandatory: Vec<A>,

                #[builder(setter(strip_option), default)]
                #[serde(default)]
                optional: Option< Vec<B> >,
            }

            impl<A: Clone, B> Foo<A, B>
            where
                B: Display
            {
                pub fn mandatory(&self) -> &[A] {
                    self.mandatory.as_slice()
                }

                pub fn optional(&self) -> Option< &Vec<B> > {
                    self.optional.as_ref()
                }
            }

            impl<A: Clone, B> FooBuilder<A, B>
            where
                B: Display
            {
                pub fn build(&mut self) -> ::std::result::Result<Foo<A, B>, ::oxidizer_api_lifecycle::validation::Error> {
                    ::oxidizer_api_lifecycle::validation::__private::Validate::validate(self.build_core()
                        .expect("impossible - all mandatory fields are specified via ctor parameters"))
                }

                #[allow(clippy::needless_update)] // If all fields are mandatory.
                pub fn new(mandatory: Vec<A>) -> Self {
                    Self {
                        mandatory: Some(mandatory),
                        ..Self::create_empty()
                    }
                }
            }

            impl<'de, A: Clone, B> ::serde::de::Deserialize<'de> for Foo<A, B>
            where
                B: Display
            {
                fn deserialize<D>(deserializer: D) -> ::std::result::Result<Foo<A, B>, D::Error>
                where
                    D: ::serde::de::Deserializer<'de>,
                {
                    #[derive(::std::fmt::Debug, ::serde::Deserialize)]
                    struct FooInner<A, B> {
                        mandatory: Vec<A>,

                        #[serde(default)]
                        optional: Option< Vec<B> >,
                    }

                    let inner = FooInner::deserialize(deserializer)?;

                    let full = Foo {
                        mandatory: inner.mandatory,
                        optional: inner.optional,
                    };

                    ::oxidizer_api_lifecycle::validation::__private::Validate::validate(full)
                        .map_err(::serde::de::Error::custom)
                }
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn dto_with_custom_serde_on_field_fails() {
        let input = parse_quote! {
            pub struct Foo {
                #[serde(default)]
                length: usize,

                #[field(optional)]
                timeout_seconds: usize,

                name: Option<String>,
            }
        };

        let meta = quote! { dto, no_validation };

        core_struct(meta, input).unwrap_err();
    }

    #[test]
    fn dto_with_custom_serde_on_type_fails() {
        let input = parse_quote! {
            #[serde(rename_all="whatever")]
            pub struct Foo {
                length: usize,

                #[field(optional)]
                timeout_seconds: usize,

                name: Option<String>,
            }
        };

        let meta = quote! { dto, no_validation };

        core_struct(meta, input).unwrap_err();
    }

    #[test]
    fn nonsense_in_attr() {
        let input = parse_quote! {
            pub struct Foo {
                length: usize,
            }
        };

        let meta = quote! { behavioral, some, nonsense };

        core_struct(meta, input).unwrap_err();
    }

    #[test]
    fn not_public() {
        let input = parse_quote! {
            pub(crate) struct Foo {
                length: usize,
            }
        };

        let meta = quote! { behavioral };

        core_struct(meta, input).unwrap_err();
    }

    #[test]
    fn reference_with_config() {
        let input = parse_quote! {
            pub struct Foo {
                name: &'static str,
            }
        };

        let meta = quote! { dto, config };

        core_struct(meta, input).unwrap_err();
    }

    #[test]
    fn unnamed_fields() {
        let input = parse_quote! {
            pub struct Foo(usize);
        };

        let meta = quote! { behavioral };

        core_struct(meta, input).unwrap_err();
    }

    #[test]
    fn reference_in_dto() {
        let input = parse_quote! {
            pub struct Foo {
                length: &usize,
            }
        };

        let meta = quote! { dto, no_validation };

        core_struct(meta, input).unwrap_err();
    }

    #[test]
    fn public_fields() {
        let input: ItemStruct = parse_quote! {
            pub struct Foo {
                pub length: usize,
            }
        };

        let meta = quote! { behavioral };
        core_struct(meta, input.clone()).unwrap_err();

        let meta = quote! { value_object };
        core_struct(meta, input.clone()).unwrap_err();

        let meta = quote! { dto };
        core_struct(meta, input).unwrap_err();
    }

    #[test]
    fn dto_field_kind_parse() {
        let input = parse_quote! { Option<String> };
        let result = DtoFieldKind::parse(&input).unwrap();
        assert_eq!(result, DtoFieldKind::Option);

        let input = parse_quote! { Vec<String> };
        let result = DtoFieldKind::parse(&input).unwrap();
        assert_eq!(result, DtoFieldKind::Vec);

        let input = parse_quote! { String };
        let result = DtoFieldKind::parse(&input).unwrap();
        assert_eq!(result, DtoFieldKind::String);

        let input = parse_quote! { &'static str };
        let result = DtoFieldKind::parse(&input).unwrap();
        assert_eq!(result, DtoFieldKind::StaticRef);

        let input = parse_quote! { usize };
        let result = DtoFieldKind::parse(&input).unwrap();
        assert_eq!(result, DtoFieldKind::Other);

        let input = parse_quote! { &'static mut usize };
        DtoFieldKind::parse(&input).unwrap_err();

        let input = parse_quote! { &mut usize };
        DtoFieldKind::parse(&input).unwrap_err();

        let input = parse_quote! { &usize };
        DtoFieldKind::parse(&input).unwrap_err();

        let input = Type::Path(TypePath {
            qself: None,
            path: Path {
                leading_colon: None,
                segments: Punctuated::default(),
            },
        });
        DtoFieldKind::parse(&input).unwrap_err();

        // We do not today support arrays because it is not clear how to wire this up
        // to e.g. configuration module. If it ever becomes necessary, we can potentially
        // relax this constraint for in-memory-only DTOs. Review when someone actually needs arrays.
        let input = parse_quote! { [u8; 32] };
        DtoFieldKind::parse(&input).unwrap_err();
    }

    #[test]
    fn emit_getter_fn_other_no_copy() {
        let field = parse_quote! {
            length: usize
        };
        let field_options = FieldOptions::default();
        let field_kind = DtoFieldKind::Other;

        let result = emit_getter_fn(&field, &field_options, &field_kind);

        let expected = quote! {
            pub fn length(&self) -> &usize {
                &self.length
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn emit_getter_fn_other_copy() {
        let field = parse_quote! {
            length: usize
        };
        let field_options = FieldOptions {
            copy: true,
            optional: false,
        };
        let field_kind = DtoFieldKind::Other;

        let result = emit_getter_fn(&field, &field_options, &field_kind);

        let expected = quote! {
            pub fn length(&self) -> usize {
                self.length
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn emit_getter_fn_ref() {
        let field = parse_quote! {
            name: &'static str
        };
        let field_options = FieldOptions::default();
        let field_kind = DtoFieldKind::StaticRef;

        let result = emit_getter_fn(&field, &field_options, &field_kind);

        let expected = quote! {
            pub fn name(&self) -> &'static str {
                self.name
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn emit_getter_fn_option() {
        let field = parse_quote! {
            length: Option<usize>
        };
        let field_options = FieldOptions::default();
        let field_kind = DtoFieldKind::Option;

        let result = emit_getter_fn(&field, &field_options, &field_kind);

        let expected = quote! {
            pub fn length(&self) -> Option<&usize> {
                self.length.as_ref()
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn emit_getter_fn_option_ref() {
        let field = parse_quote! {
            title: Option<&'static str>
        };
        let field_options = FieldOptions::default();
        let field_kind = DtoFieldKind::Option;

        let result = emit_getter_fn(&field, &field_options, &field_kind);

        let expected = quote! {
            pub fn title(&self) -> Option<&'static str> {
                self.title
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn emit_getter_fn_option_copy() {
        let field = parse_quote! {
            length: Option<usize>
        };
        let field_options = FieldOptions {
            copy: true,
            optional: false,
        };
        let field_kind = DtoFieldKind::Option;

        let result = emit_getter_fn(&field, &field_options, &field_kind);

        let expected = quote! {
            pub fn length(&self) -> Option<usize> {
                self.length
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn emit_getter_fn_vec() {
        let field = parse_quote! {
            lengths: Vec<usize>
        };
        let field_options = FieldOptions::default();
        let field_kind = DtoFieldKind::Vec;

        let result = emit_getter_fn(&field, &field_options, &field_kind);

        let expected = quote! {
            pub fn lengths(&self) -> &[usize] {
                self.lengths.as_slice()
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn emit_getter_fn_string() {
        let field = parse_quote! {
            name: String
        };
        let field_options = FieldOptions::default();
        let field_kind = DtoFieldKind::String;

        let result = emit_getter_fn(&field, &field_options, &field_kind);

        let expected = quote! {
            pub fn name(&self) -> &str {
                self.name.as_str()
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }
}