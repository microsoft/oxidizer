// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Result, parse2};

pub fn redacted_debug_impl(input: TokenStream) -> Result<TokenStream> {
    let input: DeriveInput = parse2(input)?;
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics data_privacy::RedactedDebug for #name #ty_generics #where_clause {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "Converting from u64 to usize, value is known to be <= 128"
            )]
            fn fmt(&self, engine: &data_privacy::RedactionEngine, output: &mut std::fmt::Formatter<'_>) -> core::fmt::Result {
                let v = self.as_declassified();

                let mut local_buf = [0u8; 128];
                let amount = {
                    let mut cursor = std::io::Cursor::new(&mut local_buf[..]);
                    if std::io::Write::write_fmt(&mut cursor, format_args!("{v:?}")).is_ok() {
                        cursor.position() as usize
                    } else {
                        local_buf.len() + 1 // force fallback case on write errors
                    }
                };

                if amount <= local_buf.len() {
                    // SAFETY: We know the buffer contains valid UTF-8 because the Debug impl can only write valid UTF-8.
                    let s = unsafe { core::str::from_utf8_unchecked(&local_buf[..amount]) };

                    engine.redact(&self.data_class(), s, output)
                } else {
                    // If the value is too large to fit in the buffer, we fall back to using the Debug format directly.
                    engine.redact(&self.data_class(), format!("{v:?}"), output)
                }
            }
        }
    })
}

pub fn redacted_display_impl(input: TokenStream) -> Result<TokenStream> {
    let input: DeriveInput = parse2(input)?;

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let expanded = quote! {
        impl #impl_generics data_privacy::RedactedDisplay for #name #ty_generics #where_clause {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "Converting from u64 to usize, value is known to be <= 128"
            )]
            fn fmt(&self, engine: &data_privacy::RedactionEngine, output: &mut std::fmt::Formatter) -> core::fmt::Result {
                let v = self.as_declassified();

                let mut local_buf = [0u8; 128];
                let amount = {
                    let mut cursor = std::io::Cursor::new(&mut local_buf[..]);
                    if std::io::Write::write_fmt(&mut cursor, format_args!("{v}")).is_ok() {
                        cursor.position() as usize
                    } else {
                        local_buf.len() + 1 // force fallback case on write errors
                    }
                };

                if amount <= local_buf.len() {
                    // SAFETY: We know the buffer contains valid UTF-8 because the Display impl can only write valid UTF-8.
                    let s = unsafe { core::str::from_utf8_unchecked(&local_buf[..amount]) };

                    engine.redact(&self.data_class(), s, output)
                } else {
                    // If the value is too large to fit in the buffer, we fall back to using the Display format directly.
                    engine.redact(&self.data_class(), format!("{v}"), output)
                }
            }
        }
    };

    Ok(expanded)
}

pub fn redacted_to_string_impl(input: TokenStream) -> Result<TokenStream> {
    let input: DeriveInput = parse2(input)?;

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let expanded = quote! {
        impl #impl_generics data_privacy::RedactedToString for #name #ty_generics #where_clause {
            fn to_string(&self, engine: &data_privacy::RedactionEngine) -> String {
                let v = self.as_declassified();
                let mut output = String::new();
                _ = engine.redact(&self.data_class(), v.to_string(), &mut output);
                output
           }
        }
    };

    Ok(expanded)
}

pub fn classified_debug_impl(input: TokenStream) -> Result<TokenStream> {
    let input: DeriveInput = parse2(input)?;

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let expanded = quote! {
        impl #impl_generics core::fmt::Debug for #name #ty_generics #where_clause {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_fmt(format_args!(
                    "<CLASSIFIED:{}/{}>",
                    data_privacy::Classified::data_class(self).taxonomy(),
                    data_privacy::Classified::data_class(self).name()
                ))
            }
        }
    };

    Ok(expanded)
}

#[cfg(test)]
mod test {
    use crate::derive::*;
    use insta::assert_snapshot;
    use quote::quote;

    #[test]
    fn redacted_debug() {
        let input = quote! {
            struct EmailAddress(String);
        };

        let result = redacted_debug_impl(input);
        let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
        let pretty = prettyplease::unparse(&result_file);

        assert_snapshot!(pretty);
    }

    #[test]
    fn redacted_display() {
        let input = quote! {
            struct EmailAddress(String);
        };

        let result = redacted_display_impl(input);
        let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
        let pretty = prettyplease::unparse(&result_file);

        assert_snapshot!(pretty);
    }

    #[test]
    fn redacted_to_string() {
        let input = quote! {
            struct EmailAddress(String);
        };

        let result = redacted_to_string_impl(input);
        let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
        let pretty = prettyplease::unparse(&result_file);

        assert_snapshot!(pretty);
    }

    #[test]
    fn classified_debug() {
        let input = quote! {
            struct EmailAddress(String);
        };

        let result = classified_debug_impl(input);
        let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
        let pretty = prettyplease::unparse(&result_file);

        assert_snapshot!(pretty);
    }
}
