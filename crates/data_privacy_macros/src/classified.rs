// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parse;
use syn::spanned::Spanned;
use syn::{Fields, ItemStruct, Path, parse2};

type SynResult<T> = Result<T, syn::Error>;

struct MacroArgs {
    data_class: Path,
}

impl MacroArgs {
    pub fn parse(attr_args: TokenStream) -> SynResult<Self> {
        if attr_args.is_empty() {
            Err(syn::Error::new(
                attr_args.span(),
                "classified attribute requires a taxonomy and data class name argument",
            ))
        } else {
            parse2(attr_args)
        }
    }
}

impl Parse for MacroArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let data_class: Path = input.parse()?;
        Ok(Self { data_class })
    }
}

pub fn classified_impl(attr_args: TokenStream, item: TokenStream) -> SynResult<TokenStream> {
    let macro_args = MacroArgs::parse(attr_args)?;
    let input: ItemStruct = parse2(item)?;

    let struct_name = &input.ident;
    let data_privacy_path = quote!(data_privacy);
    let data_class = macro_args.data_class;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let unnamed_fields = match &input.fields {
        Fields::Unnamed(unnamed_fields) => unnamed_fields,

        Fields::Named(fields) => {
            return Err(syn::Error::new_spanned(fields, "Named fields aren't supported"));
        }

        Fields::Unit => return Err(syn::Error::new_spanned(input, "Unit structs aren't supported")),
    };

    let field_count = unnamed_fields.unnamed.len();

    if field_count != 1 {
        return Err(syn::Error::new_spanned(unnamed_fields, "Tuple struct must have exactly one field"));
    }

    let field_type = &unnamed_fields.unnamed[0].ty;

    Ok(quote! {
        #input

        impl #impl_generics #struct_name #ty_generics #where_clause {
            /// Exfiltrates the payload, allowing it to be used outside the classified context.
            ///
            /// Exfiltration should be done with caution, as it may expose sensitive information.
            ///
            /// # Returns
            /// The original payload.
            #[must_use]
            fn declassify(self) -> #field_type {
                self.0
            }

            /// Provides a reference to the declassified payload, allowing read access without ownership transfer.
            ///
            /// Exfiltration should be done with caution, as it may expose sensitive information.
            ///
            /// # Returns
            /// A reference to the original payload.
            #[must_use]
            fn as_declassified(&self) -> &#field_type {
                &self.0
            }

            /// Provides a mutable reference to the declassified payload, allowing write access without ownership transfer.
            ///
            /// Exfiltration should be done with caution, as it may expose sensitive information.
            ///
            /// # Returns
            /// A mutable reference to the original payload.
            #[must_use]
            fn as_declassified_mut(&mut self) -> &mut #field_type {
                &mut self.0
            }

            /// Returns the data class of the payload.
            #[must_use]
            fn data_class(&self) -> #data_privacy_path::DataClass {
                #data_class.data_class()
            }
        }

        impl #impl_generics #data_privacy_path::Classified for #struct_name #ty_generics #where_clause {
            type Payload = #field_type;

            fn declassify(self) -> Self::Payload {
                Self::declassify(self)
            }

            fn as_declassified(&self) -> &Self::Payload {
                Self::as_declassified(self)
            }

            fn as_declassified_mut(&mut self) -> &mut Self::Payload {
                Self::as_declassified_mut(self)
            }

            fn data_class(&self) -> #data_privacy_path::DataClass {
                Self::data_class(self)
            }
        }

        impl #impl_generics core::fmt::Debug for #struct_name #ty_generics #where_clause {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_fmt(::core::format_args!("<CLASSIFIED:{}>", self.data_class()))
            }
        }

        impl #impl_generics core::fmt::Display for #struct_name #ty_generics #where_clause {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_fmt(::core::format_args!("<CLASSIFIED:{}>", self.data_class()))
            }
        }

        impl #impl_generics core::ops::Deref for #struct_name #ty_generics #where_clause {
            type Target = core::convert::Infallible;

            fn deref(&self) -> &Self::Target {
                todo!()
            }
        }

        impl #impl_generics core::ops::DerefMut for #struct_name #ty_generics #where_clause {
            fn deref_mut(&mut self) -> &mut Self::Target {
                todo!()
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    #[test]
    fn test_classified_impl_empty_args() {
        let attr_args = quote! {};
        let input = quote! {
            pub struct EmailAddress(String);
        };

        let result = classified_impl(attr_args, input);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("classified attribute requires a taxonomy and data class name")
        );
    }

    #[test]
    fn test_classified_impl_random_args() {
        let attr_args = quote! {Foo::Bar Baz};
        let input = quote! {
            struct EmailAddress(String);
        };

        let result = classified_impl(attr_args, input);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("unexpected token"));
    }

    #[test]
    fn test_classified_impl_named_fields() {
        let attr_args = quote! { ExampleTaxonomy::PersonallyIdentifiableInformation };
        let input = quote! {
            struct EmailAddress { x : String }
        };

        let result = classified_impl(attr_args, input);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Named fields aren't supported"));
    }

    #[test]
    fn test_classified_impl_no_fields() {
        let attr_args = quote! { ExampleTaxonomy::PersonallyIdentifiableInformation };
        let input = quote! {
            struct EmailAddress;
        };

        let result = classified_impl(attr_args, input);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Unit structs aren't supported"));
    }

    #[test]
    fn test_classified_impl_too_many_fields() {
        let attr_args = quote! { ExampleTaxonomy::PersonallyIdentifiableInformation };
        let input = quote! {
            struct EmailAddress(String, i32);
        };

        let result = classified_impl(attr_args, input);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Tuple struct must have exactly one field"));
    }

    #[test]
    fn test_success() {
        let attr_args = quote! { ExampleTaxonomy::PersonallyIdentifiableInformation };
        let input = quote! {
            struct EmailAddress(String);
        };

        let result = classified_impl(attr_args, input);
        let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
        let pretty = prettyplease::unparse(&result_file);

        assert_snapshot!(pretty);
    }
}
