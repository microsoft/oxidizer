// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::{Span, TokenStream};
use syn::parse::Parser;
use syn::{Attribute, Ident};

#[derive(Debug, Eq, PartialEq)]
pub enum StructCategory {
    BehavioralType,
    ValueObject,
    DataTransferObject,
}

#[derive(Debug)]
pub struct StructOptions {
    pub category: StructCategory,
    pub config: bool,
    pub no_validation: bool,
}

impl StructOptions {
    pub fn parse(attr: TokenStream) -> super::Result<Self> {
        let mut no_validation = false;
        let mut config = false;
        let mut category: Option<StructCategory> = None;

        syn::meta::parser(|meta| {
            fn verify_category_not_already_set(
                category: Option<&StructCategory>,
            ) -> Result<(), syn::Error> {
                match category {
                    Some(_) => Err(syn::Error::new(
                        Span::call_site(),
                        "type category specified multiple times",
                    )),
                    None => Ok(()),
                }
            }

            let Some(arg) = meta.path.get_ident().map(Ident::to_string) else {
                return Err(meta.error("unsupported argument"));
            };

            match arg.as_str() {
                "no_validation" => no_validation = true,
                "config" => config = true,
                "behavioral" => {
                    verify_category_not_already_set(category.as_ref())?;
                    category = Some(StructCategory::BehavioralType);
                }
                "dto" => {
                    verify_category_not_already_set(category.as_ref())?;
                    category = Some(StructCategory::DataTransferObject);
                }
                "value" => {
                    verify_category_not_already_set(category.as_ref())?;
                    category = Some(StructCategory::ValueObject);
                }
                _ => return Err(meta.error("unsupported argument")),
            }

            Ok(())
        })
        .parse2(attr)?;

        let category = category.ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                "'oxidizer_api_lifecycle::api' on structs must specify a type category (behavioral, dto, or value)",
            )
        })?;

        let instance = Self {
            category,
            config,
            no_validation,
        };

        instance.validate()
    }

    fn validate(self) -> super::Result<Self> {
        match self.category {
            StructCategory::BehavioralType => {
                if self.config {
                    return Err(syn::Error::new(
                        Span::call_site(),
                        "Behavioral types cannot have the 'config' flag in the oxidizer_api_lifecycle::api attribute.",
                    ));
                }

                if self.no_validation {
                    return Err(syn::Error::new(
                        Span::call_site(),
                        "Behavioral types cannot have the 'no_validation' flag in the oxidizer_api_lifecycle::api attribute.",
                    ));
                }
            }
            StructCategory::ValueObject => {
                if self.config {
                    return Err(syn::Error::new(
                        Span::call_site(),
                        "Value object types cannot have the 'config' flag in the oxidizer_api_lifecycle::api attribute.",
                    ));
                }

                if self.no_validation {
                    return Err(syn::Error::new(
                        Span::call_site(),
                        "Value object types cannot have the 'no_validation' flag in the oxidizer_api_lifecycle::api attribute.",
                    ));
                }
            }
            StructCategory::DataTransferObject => {}
        }

        Ok(self)
    }
}

#[derive(Debug)]
pub struct FieldOptions {
    pub optional: bool,
    pub copy: bool,
}

impl FieldOptions {
    pub fn parse(attr: &Attribute) -> super::Result<Self> {
        let mut optional = false;
        let mut copy = false;

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("optional") {
                optional = true;
                Ok(())
            } else if meta.path.is_ident("copy") {
                copy = true;
                Ok(())
            } else {
                Err(meta.error("unsupported property"))
            }
        })?;

        Ok(Self { optional, copy })
    }
}

#[expect(
    clippy::derivable_impls,
    reason = "Deliberate to be explicit and avoid accidental defaults"
)]
impl Default for FieldOptions {
    fn default() -> Self {
        Self {
            optional: false,
            copy: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn struct_options_parse() {
        let options = StructOptions::parse(quote! { behavioral }).unwrap();
        assert_eq!(options.category, StructCategory::BehavioralType);
        assert!(!options.config);
        assert!(!options.no_validation);

        let options = StructOptions::parse(quote! { value }).unwrap();
        assert_eq!(options.category, StructCategory::ValueObject);
        assert!(!options.config);
        assert!(!options.no_validation);

        let options = StructOptions::parse(quote! { dto }).unwrap();
        assert_eq!(options.category, StructCategory::DataTransferObject);
        assert!(!options.config);
        assert!(!options.no_validation);

        let options = StructOptions::parse(quote! { dto, config, no_validation }).unwrap();
        assert_eq!(options.category, StructCategory::DataTransferObject);
        assert!(options.config);
        assert!(options.no_validation);
    }

    #[test]
    fn struct_options_invalid() {
        // Must specify at least a category.
        StructOptions::parse(TokenStream::new()).unwrap_err();

        // Nonsense property.
        StructOptions::parse(quote! { whatever }).unwrap_err();

        // Not a single ident, which is (at least for now) not supported.
        StructOptions::parse(quote! { multiple::idents::in::here }).unwrap_err();

        // Pick one!
        StructOptions::parse(quote! { behavioral, dto, value }).unwrap_err();

        // Not valid for behavioral type.
        StructOptions::parse(quote! { behavioral, config }).unwrap_err();

        // Not valid for behavioral type.
        StructOptions::parse(quote! { behavioral, no_validation }).unwrap_err();

        // Not valid for behavioral type.
        StructOptions::parse(quote! { no_validation }).unwrap_err();

        // Not valid for value object.
        StructOptions::parse(quote! { value, config }).unwrap_err();

        // Not valid for value object.
        StructOptions::parse(quote! { value, no_validation }).unwrap_err();
    }

    #[test]
    fn field_options_parse() {
        let attr = parse_quote! { #[field(optional)] };
        let options = FieldOptions::parse(&attr).unwrap();

        assert!(options.optional);
        assert!(!options.copy);

        let attr = parse_quote! { #[field(copy)] };
        let options = FieldOptions::parse(&attr).unwrap();

        assert!(!options.optional);
        assert!(options.copy);
    }

    #[test]
    fn field_options_default() {
        let attr = parse_quote! { #[field()] };
        let options = FieldOptions::parse(&attr).unwrap();
        assert!(!options.optional);
        assert!(!options.copy);

        let explicit_default = FieldOptions::default();
        assert_eq!(options.optional, explicit_default.optional);
        assert_eq!(options.copy, explicit_default.copy);
    }

    #[test]
    fn field_options_invalid() {
        // Nonsense property.
        let attr = parse_quote! { #[field(whatever)] };
        FieldOptions::parse(&attr).unwrap_err();
    }
}