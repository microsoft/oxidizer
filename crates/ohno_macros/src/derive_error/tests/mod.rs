// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod debug;
mod display;
mod tuple;

use pretty_assertions::assert_eq;
use syn::{DeriveInput, parse_quote};

use super::*;

#[test]
fn test_basic_error_struct_expansion() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct TestError {
            message: String,
            #[error]
            inner: OhnoCore,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    let ast: syn::File = syn::parse2(result).expect("not a valid tokenstream");
    let result_string = prettyplease::unparse(&ast);

    let expected_impls: &[syn::ItemImpl] = &[
        parse_quote! {
            impl TestError {
                #[doc = r" Creates a new error with custom fields and default message."]
                pub(crate) fn new(message: impl Into<String>) -> Self {
                    Self {
                        message: message.into(),
                        inner: ohno::OhnoCore::default(),
                    }
                }

                #[doc = r" Creates a new error with custom fields and a specified error."]
                pub(crate) fn caused_by(message: impl Into<String>, error: impl Into<Box<dyn std::error::Error + Send + Sync > >) -> Self {
                    Self {
                        message: message.into(),
                        inner: ohno::OhnoCore::from(error),
                    }
                }
            }
        },
        parse_quote! {
            impl From<std::convert::Infallible> for TestError {
                fn from(_: std::convert::Infallible) -> Self {
                    unreachable!("Infallible should never be converted to Error")
                }
            }
        },
        parse_quote! {
            impl std::fmt::Display for TestError {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    self.inner.format_error(f, stringify!(TestError), None)
                }
            }
        },
        parse_quote! {
            impl std::error::Error for TestError {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    self.inner.source()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorSpan for TestError {
                fn add_error_span(&mut self, trace: ohno::TraceInfo) {
                    self.inner.add_error_span(trace);
                }
            }
        },
        parse_quote! {
            impl ::core::fmt::Debug for TestError {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    f.debug_struct(stringify!(TestError))
                        .field("message", &self.message)
                        .field("inner", &self.inner)
                        .finish()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorExt for TestError {
                fn message(&self) -> String {
                    self.inner.format_message(stringify!(TestError), None)
                }
                fn backtrace(&self) -> &std::backtrace::Backtrace {
                    self.inner.backtrace()
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
fn test_from_attribute_expansion() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        #[from(std::io::Error)]
        struct IoWrapperError {
            #[error]
            inner: OhnoCore,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    let ast: syn::File = syn::parse2(result).expect("not a valid tokenstream");
    let result_string = prettyplease::unparse(&ast);

    let expected_impls: &[syn::ItemImpl] = &[
        parse_quote! {
            impl IoWrapperError {
                #[doc = r" Creates a new error with default message."]
                pub(crate) fn new() -> Self {
                    Self {
                        inner: ohno::OhnoCore::default(),
                    }
                }

                #[doc = r" Creates a new error with a specified error."]
                pub(crate) fn caused_by(error: impl Into<Box<dyn std::error::Error + Send + Sync > >) -> Self {
                    Self {
                        inner: ohno::OhnoCore::from(error),
                    }
                }
            }
        },
        parse_quote! {
            impl From<std::io::Error> for IoWrapperError {
                fn from(error: std::io::Error) -> Self {
                    Self {
                        inner: ohno::OhnoCore::from(error),
                    }
                }
            }
        },
        parse_quote! {
            impl From<std::convert::Infallible> for IoWrapperError {
                fn from(_: std::convert::Infallible) -> Self {
                    unreachable!("Infallible should never be converted to Error")
                }
            }
        },
        parse_quote! {
            impl std::fmt::Display for IoWrapperError {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    self.inner.format_error(f, stringify!(IoWrapperError), None)
                }
            }
        },
        parse_quote! {
            impl std::error::Error for IoWrapperError {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    self.inner.source()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorSpan for IoWrapperError {
                fn add_error_span(&mut self, trace: ohno::TraceInfo) {
                    self.inner.add_error_span(trace);
                }
            }
        },
        parse_quote! {
            impl ::core::fmt::Debug for IoWrapperError {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    f.debug_struct(stringify!(IoWrapperError))
                        .field("inner", &self.inner)
                        .finish()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorExt for IoWrapperError {
                fn message(&self) -> String {
                    self.inner.format_message(stringify!(IoWrapperError), None)
                }
                fn backtrace(&self) -> &std::backtrace::Backtrace {
                    self.inner.backtrace()
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
fn test_no_debug_attribute_expansion() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        #[no_debug]
        struct NoDebugError {
            message: String,
            #[error]
            inner: OhnoCore,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    let ast: syn::File = syn::parse2(result).expect("not a valid tokenstream");
    let result_string = prettyplease::unparse(&ast);

    let expected_impls: &[syn::ItemImpl] = &[
        parse_quote! {
            impl NoDebugError {
                #[doc = r" Creates a new error with custom fields and default message."]
                pub(crate) fn new(message: impl Into<String>) -> Self {
                    Self {
                        message: message.into(),
                        inner: ohno::OhnoCore::default(),
                    }
                }

                #[doc = r" Creates a new error with custom fields and a specified error."]
                pub(crate) fn caused_by(message: impl Into<String>, error: impl Into<Box<dyn std::error::Error + Send + Sync > >) -> Self {
                    Self {
                        message: message.into(),
                        inner: ohno::OhnoCore::from(error),
                    }
                }
            }
        },
        parse_quote! {
            impl From<std::convert::Infallible> for NoDebugError {
                fn from(_: std::convert::Infallible) -> Self {
                    unreachable!("Infallible should never be converted to Error")
                }
            }
        },
        parse_quote! {
            impl std::fmt::Display for NoDebugError {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    self.inner.format_error(f, stringify!(NoDebugError), None)
                }
            }
        },
        parse_quote! {
            impl std::error::Error for NoDebugError {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    self.inner.source()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorSpan for NoDebugError {
                fn add_error_span(&mut self, trace: ohno::TraceInfo) {
                    self.inner.add_error_span(trace);
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorExt for NoDebugError {
                fn message(&self) -> String {
                    self.inner.format_message(stringify!(NoDebugError), None)
                }
                fn backtrace(&self) -> &std::backtrace::Backtrace {
                    self.inner.backtrace()
                }
            }
        },
        // Note: No Debug impl should be generated due to #[no_debug]
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
fn test_no_constructors_expansion() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        #[no_constructors]
        struct NoConstructorsError {
            message: String,
            #[error]
            inner: OhnoCore,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    let ast: syn::File = syn::parse2(result).expect("not a valid tokenstream");
    let result_string = prettyplease::unparse(&ast);

    let expected_impls: &[syn::ItemImpl] = &[
        // Note: No constructor methods should be generated due to #[no_constructors]
        parse_quote! {
            impl From<std::convert::Infallible> for NoConstructorsError {
                fn from(_: std::convert::Infallible) -> Self {
                    unreachable!("Infallible should never be converted to Error")
                }
            }
        },
        parse_quote! {
            impl std::fmt::Display for NoConstructorsError {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    self.inner.format_error(f, stringify!(NoConstructorsError), None)
                }
            }
        },
        parse_quote! {
            impl std::error::Error for NoConstructorsError {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    self.inner.source()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorSpan for NoConstructorsError {
                fn add_error_span(&mut self, trace: ohno::TraceInfo) {
                    self.inner.add_error_span(trace);
                }
            }
        },
        parse_quote! {
            impl ::core::fmt::Debug for NoConstructorsError {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    f.debug_struct(stringify!(NoConstructorsError))
                        .field("message", &self.message)
                        .field("inner", &self.inner)
                        .finish()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorExt for NoConstructorsError {
                fn message(&self) -> String {
                    self.inner.format_message(stringify!(NoConstructorsError), None)
                }
                fn backtrace(&self) -> &std::backtrace::Backtrace {
                    self.inner.backtrace()
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
fn test_generate_from_implementations_tuple() {
    // Test that generate_from_implementations_tuple doesn't return Ok(Default::default())
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        #[from(String)]
        struct TestError(String, #[error] OhnoCore);
    };

    let result = crate::derive_error::impl_error_derive(&input).unwrap();
    let ast: syn::File = syn::parse2(result).expect("not a valid tokenstream");
    let result_string = prettyplease::unparse(&ast);

    let expected_impls: &[syn::ItemImpl] = &[
        parse_quote! {
            impl TestError {
                #[doc = r" Creates a new error with custom fields and default message."]
                pub(crate) fn new (param_0: impl Into<String>) -> Self {
                    Self (param_0.into(), ohno::OhnoCore::default())
                }

                #[doc = r" Creates a new error with custom fields and a specified error."]
                pub(crate) fn caused_by(
                    param_0: impl Into<String>,
                    error: impl Into<Box<dyn std::error::Error + Send + Sync>>
                ) -> Self {
                    Self(param_0.into(), ohno::OhnoCore::from(error))
                }
            }
        },
        parse_quote! {
            impl From<String> for TestError {
                fn from(error: String) -> Self {
                    Self (Default::default(), ohno::OhnoCore::from(error))
                }
            }
        },
        parse_quote! {
            impl From<std::convert::Infallible> for TestError {
                fn from(_: std::convert::Infallible) -> Self {
                    unreachable!("Infallible should never be converted to Error")
                }
            }
        },
        parse_quote! {
            impl std::fmt::Display for TestError {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    self.1.format_error(f, stringify!(TestError), None)
                }
            }
        },
        parse_quote! {
            impl std::error::Error for TestError {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    self.1.source()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorSpan for TestError {
                fn add_error_span(&mut self, trace: ohno::TraceInfo) {
                    self.1.add_error_span(trace);
                }
            }
        },
        parse_quote! {
            impl ::core::fmt::Debug for TestError {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    f.debug_tuple(stringify!(TestError))
                         .field(&self.0)
                         .field(&self.1)
                         .finish()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorExt for TestError {
                fn message(&self) -> String {
                    self.1.format_message(stringify!(TestError), None)
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
    assert_eq!(result_string, expected);
}

#[test]
fn test_ohno_core_first_position_struct() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct FirstPositionError {
            #[error]
            inner: OhnoCore,
            message: String,
            code: u32,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    let ast: syn::File = syn::parse2(result).expect("not a valid tokenstream");
    let result_string = prettyplease::unparse(&ast);

    let expected_impls: &[syn::ItemImpl] = &[
        parse_quote! {
            impl FirstPositionError {
                #[doc = r" Creates a new error with custom fields and default message."]
                pub(crate) fn new(message: impl Into<String>, code: impl Into<u32>) -> Self {
                    Self {
                        message: message.into(),
                        code: code.into(),
                        inner: ohno::OhnoCore::default(),
                    }
                }

                #[doc = r" Creates a new error with custom fields and a specified error."]
                pub(crate) fn caused_by(message: impl Into<String>, code: impl Into<u32>, error: impl Into<Box<dyn std::error::Error + Send + Sync > >) -> Self {
                    Self {
                        message: message.into(),
                        code: code.into(),
                        inner: ohno::OhnoCore::from(error),
                    }
                }
            }
        },
        parse_quote! {
            impl From<std::convert::Infallible> for FirstPositionError {
                fn from(_: std::convert::Infallible) -> Self {
                    unreachable!("Infallible should never be converted to Error")
                }
            }
        },
        parse_quote! {
            impl std::fmt::Display for FirstPositionError {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    self.inner.format_error(f, stringify!(FirstPositionError), None)
                }
            }
        },
        parse_quote! {
            impl std::error::Error for FirstPositionError {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    self.inner.source()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorSpan for FirstPositionError {
                fn add_error_span(&mut self, trace: ohno::TraceInfo) {
                    self.inner.add_error_span(trace);
                }
            }
        },
        parse_quote! {
            impl ::core::fmt::Debug for FirstPositionError {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    f.debug_struct(stringify!(FirstPositionError))
                        .field("inner", &self.inner)
                        .field("message", &self.message)
                        .field("code", &self.code)
                        .finish()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorExt for FirstPositionError {
                fn message(&self) -> String {
                    self.inner.format_message(stringify!(FirstPositionError), None)
                }
                fn backtrace(&self) -> &std::backtrace::Backtrace {
                    self.inner.backtrace()
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
fn test_ohno_core_middle_position_struct() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct MiddlePositionError {
            message: String,
            #[error]
            inner: OhnoCore,
            code: u32,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    let ast: syn::File = syn::parse2(result).expect("not a valid tokenstream");
    let result_string = prettyplease::unparse(&ast);

    let expected_impls: &[syn::ItemImpl] = &[
        parse_quote! {
            impl MiddlePositionError {
                #[doc = r" Creates a new error with custom fields and default message."]
                pub(crate) fn new(message: impl Into<String>, code: impl Into<u32>) -> Self {
                    Self {
                        message: message.into(),
                        code: code.into(),
                        inner: ohno::OhnoCore::default(),
                    }
                }

                #[doc = r" Creates a new error with custom fields and a specified error."]
                pub(crate) fn caused_by(message: impl Into<String>, code: impl Into<u32>, error: impl Into<Box<dyn std::error::Error + Send + Sync > >) -> Self {
                    Self {
                        message: message.into(),
                        code: code.into(),
                        inner: ohno::OhnoCore::from(error),
                    }
                }
            }
        },
        parse_quote! {
            impl From<std::convert::Infallible> for MiddlePositionError {
                fn from(_: std::convert::Infallible) -> Self {
                    unreachable!("Infallible should never be converted to Error")
                }
            }
        },
        parse_quote! {
            impl std::fmt::Display for MiddlePositionError {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    self.inner.format_error(f, stringify!(MiddlePositionError), None)
                }
            }
        },
        parse_quote! {
            impl std::error::Error for MiddlePositionError {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    self.inner.source()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorSpan for MiddlePositionError {
                fn add_error_span(&mut self, trace: ohno::TraceInfo) {
                    self.inner.add_error_span(trace);
                }
            }
        },
        parse_quote! {
            impl ::core::fmt::Debug for MiddlePositionError {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    f.debug_struct(stringify!(MiddlePositionError))
                        .field("message", &self.message)
                        .field("inner", &self.inner)
                        .field("code", &self.code)
                        .finish()
                }
            }
        },
        parse_quote! {
            impl ohno::ErrorExt for MiddlePositionError {
                fn message(&self) -> String {
                    self.inner.format_message(stringify!(MiddlePositionError), None)
                }
                fn backtrace(&self) -> &std::backtrace::Backtrace {
                    self.inner.backtrace()
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
