// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use pretty_assertions::assert_eq;

use super::*;

#[test]
fn test_tuple_only_ohno_core() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct OnlyCoreError(#[error] OhnoCore);
    };

    let result = impl_error_derive(&input).unwrap();
    let ast: syn::File = syn::parse2(result).expect("not a valid tokenstream");
    let result_string = prettyplease::unparse(&ast);

    let expected_impls: &[syn::ItemImpl] = &[
        parse_quote! {
            impl OnlyCoreError {
                #[doc = r" Creates a new error with default message."]
                pub(crate) fn new() -> Self {
                    Self(ohno::OhnoCore::default())
                }

                #[doc = r" Creates a new error with a specified error."]
                pub(crate) fn caused_by(error: impl Into<Box<dyn std::error::Error + Send + Sync >>) -> Self {
                    Self(ohno::OhnoCore::from(error))
                }
            }
        },
        parse_quote! {
            impl From<std::convert::Infallible> for OnlyCoreError {
                fn from(_: std::convert::Infallible) -> Self {
                    unreachable!("Infallible should never be converted to Error")
                }
            }
        },
        parse_quote! {
            impl std::fmt::Display for OnlyCoreError {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    self.0.format_error(f, stringify!(OnlyCoreError), None)
                }
            }
        },
        parse_quote! {
            impl std::error::Error for OnlyCoreError {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    self.0.source()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorSpan for OnlyCoreError {
                fn add_error_span(&mut self, trace: ohno::SpanInfo) {
                    self.0.add_error_span(trace);
                }
            }
        },
        parse_quote! {
            impl ::core::fmt::Debug for OnlyCoreError {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    f.debug_tuple(stringify!(OnlyCoreError))
                        .field(&self.0)
                        .finish()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorExt for OnlyCoreError {
                fn message(&self) -> String {
                    self.0.format_message(stringify!(OnlyCoreError), None)
                }
                fn backtrace(&self) -> &std::backtrace::Backtrace {
                    self.0.backtrace()
                }
            }
        },
    ];

    let streams = expected_impls.iter();
    let expected = quote! {
        #(#streams)*
    };

    let ast: syn::File = syn::parse2(expected).expect("not a valid tokenstream");
    let expected = prettyplease::unparse(&ast);
    assert_eq!(expected, result_string);
}

#[test]
fn test_tuple_struct_expansion() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct TupleError(String, #[error] OhnoCore);
    };

    let result = impl_error_derive(&input).unwrap();
    let ast: syn::File = syn::parse2(result).expect("not a valid tokenstream");
    let result_string = prettyplease::unparse(&ast);

    let expected_impls: &[syn::ItemImpl] = &[
        parse_quote! {
            impl TupleError {
                #[doc = r" Creates a new error with custom fields and default message."]
                pub(crate) fn new(param_0: impl Into<String>) -> Self {
                    Self(param_0.into(), ohno::OhnoCore::default())
                }

                #[doc = r" Creates a new error with custom fields and a specified error."]
                pub(crate) fn caused_by(param_0: impl Into<String>, error: impl Into<Box<dyn std::error::Error + Send + Sync >>) -> Self {
                    Self(param_0.into(), ohno::OhnoCore::from(error))
                }
            }
        },
        parse_quote! {
            impl From<std::convert::Infallible> for TupleError {
                fn from(_: std::convert::Infallible) -> Self {
                    unreachable!("Infallible should never be converted to Error")
                }
            }
        },
        parse_quote! {
            impl std::fmt::Display for TupleError {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    self.1.format_error(f, stringify!(TupleError), None)
                }
            }
        },
        parse_quote! {
            impl std::error::Error for TupleError {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    self.1.source()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorSpan for TupleError {
                fn add_error_span(&mut self, trace: ohno::SpanInfo) {
                    self.1.add_error_span(trace);
                }
            }
        },
        parse_quote! {
            impl ::core::fmt::Debug for TupleError {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    f.debug_tuple(stringify!(TupleError))
                        .field(&self.0)
                        .field(&self.1)
                        .finish()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorExt for TupleError {
                fn message(&self) -> String {
                    self.1.format_message(stringify!(TupleError), None)
                }
                fn backtrace(&self) -> &std::backtrace::Backtrace {
                    self.1.backtrace()
                }
            }
        },
    ];

    let streams = expected_impls.iter();
    let expected = quote! {
        #(#streams)*
    };

    let ast: syn::File = syn::parse2(expected).expect("not a valid tokenstream");
    let expected = prettyplease::unparse(&ast);
    assert_eq!(expected, result_string);
}

#[test]
fn test_ohno_core_first_position_tuple() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct FirstPositionTupleError(#[error] OhnoCore, String, u32);
    };

    let result = impl_error_derive(&input).unwrap();
    let ast: syn::File = syn::parse2(result).expect("not a valid tokenstream");
    let result_string = prettyplease::unparse(&ast);

    let expected_impls: &[syn::ItemImpl] = &[
        parse_quote! {
            impl FirstPositionTupleError {
                #[doc = r" Creates a new error with custom fields and default message."]
                pub(crate) fn new(param_1: impl Into<String>, param_2: impl Into<u32>) -> Self {
                    Self(ohno::OhnoCore::default(), param_1.into(), param_2.into())
                }

                #[doc = r" Creates a new error with custom fields and a specified error."]
                pub(crate) fn caused_by(param_1: impl Into<String>, param_2: impl Into<u32>, error: impl Into<Box<dyn std::error::Error + Send + Sync >>) -> Self {
                    Self(ohno::OhnoCore::from(error), param_1.into(), param_2.into())
                }
            }
        },
        parse_quote! {
            impl From<std::convert::Infallible> for FirstPositionTupleError {
                fn from(_: std::convert::Infallible) -> Self {
                    unreachable!("Infallible should never be converted to Error")
                }
            }
        },
        parse_quote! {
            impl std::fmt::Display for FirstPositionTupleError {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    self.0.format_error(f, stringify!(FirstPositionTupleError), None)
                }
            }
        },
        parse_quote! {
            impl std::error::Error for FirstPositionTupleError {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    self.0.source()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorSpan for FirstPositionTupleError {
                fn add_error_span(&mut self, trace: ohno::SpanInfo) {
                    self.0.add_error_span(trace);
                }
            }
        },
        parse_quote! {
            impl ::core::fmt::Debug for FirstPositionTupleError {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    f.debug_tuple(stringify!(FirstPositionTupleError))
                        .field(&self.0)
                        .field(&self.1)
                        .field(&self.2)
                        .finish()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorExt for FirstPositionTupleError {
                fn message(&self) -> String {
                    self.0.format_message(stringify!(FirstPositionTupleError), None)
                }
                fn backtrace(&self) -> &std::backtrace::Backtrace {
                    self.0.backtrace()
                }
            }
        },
    ];

    let streams = expected_impls.iter();
    let expected = quote! {
        #(#streams)*
    };

    let ast: syn::File = syn::parse2(expected).expect("not a valid tokenstream");
    let expected = prettyplease::unparse(&ast);
    assert_eq!(expected, result_string);
}

#[test]
fn test_ohno_core_middle_position_tuple() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct MiddlePositionTupleError(String, #[error] OhnoCore, u32);
    };

    let result = impl_error_derive(&input).unwrap();
    let ast: syn::File = syn::parse2(result).expect("not a valid tokenstream");
    let result_string = prettyplease::unparse(&ast);

    let expected_impls: &[syn::ItemImpl] = &[
        parse_quote! {
            impl MiddlePositionTupleError {
                #[doc = r" Creates a new error with custom fields and default message."]
                pub(crate) fn new(param_0: impl Into<String>, param_2: impl Into<u32>) -> Self {
                    Self(param_0.into(), ohno::OhnoCore::default(), param_2.into())
                }

                #[doc = r" Creates a new error with custom fields and a specified error."]
                pub(crate) fn caused_by(param_0: impl Into<String>, param_2: impl Into<u32>, error: impl Into<Box<dyn std::error::Error + Send + Sync >>) -> Self {
                    Self(param_0.into(), ohno::OhnoCore::from(error), param_2.into())
                }
            }
        },
        parse_quote! {
            impl From<std::convert::Infallible> for MiddlePositionTupleError {
                fn from(_: std::convert::Infallible) -> Self {
                    unreachable!("Infallible should never be converted to Error")
                }
            }
        },
        parse_quote! {
            impl std::fmt::Display for MiddlePositionTupleError {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    self.1.format_error(f, stringify!(MiddlePositionTupleError), None)
                }
            }
        },
        parse_quote! {
            impl std::error::Error for MiddlePositionTupleError {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    self.1.source()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorSpan for MiddlePositionTupleError {
                fn add_error_span(&mut self, trace: ohno::SpanInfo) {
                    self.1.add_error_span(trace);
                }
            }
        },
        parse_quote! {
            impl ::core::fmt::Debug for MiddlePositionTupleError {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    f.debug_tuple(stringify!(MiddlePositionTupleError))
                        .field(&self.0)
                        .field(&self.1)
                        .field(&self.2)
                        .finish()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorExt for MiddlePositionTupleError {
                fn message(&self) -> String {
                    self.1.format_message(stringify!(MiddlePositionTupleError), None)
                }
                fn backtrace(&self) -> &std::backtrace::Backtrace {
                    self.1.backtrace()
                }
            }
        },
    ];

    let streams = expected_impls.iter();
    let expected = quote! {
        #(#streams)*
    };

    let ast: syn::File = syn::parse2(expected).expect("not a valid tokenstream");
    let expected = prettyplease::unparse(&ast);
    assert_eq!(expected, result_string);
}
