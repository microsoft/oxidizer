// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use pretty_assertions::assert_eq;

use super::*;

#[test]
fn test_display_attribute_expansion() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        #[display("Failed to process file: {filename}")]
        struct FileError {
            filename: String,
            #[error]
            inner: OhnoCore,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    let ast: syn::File = syn::parse2(result).expect("not a valid tokenstream");
    let result_string = prettyplease::unparse(&ast);

    let expected = quote! {
        impl FileError {
            #[doc = r" Creates a new error with custom fields and default message."]
            pub(crate) fn new(filename: impl Into<String>) -> Self {
                Self {
                    filename: filename.into(),
                    inner: ohno::OhnoCore::default(),
                }
            }

            #[doc = r" Creates a new error with custom fields and a specified error."]
            pub(crate) fn caused_by(filename: impl Into<String>, error: impl Into<Box<dyn std::error::Error + Send + Sync >>) -> Self {
                Self {
                    filename: filename.into(),
                    inner: ohno::OhnoCore::from(error),
                }
            }
        }
        impl From<std::convert::Infallible> for FileError {
            fn from(_: std::convert::Infallible) -> Self {
                unreachable!("Infallible should never be converted to Error")
            }
        }
        impl std::fmt::Display for FileError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.inner.format_error(f, stringify!(FileError), Some(std::borrow::Cow::from(format!("Failed to process file: {}", &self.filename))))
            }
        }
        impl std::error::Error for FileError {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.inner.source()
            }
        }
        impl ohno::ErrorTrace for FileError {
            fn add_error_trace(&mut self, trace: ohno::TraceInfo) {
                self.inner.add_error_trace(trace);
            }
        }
        impl ::core::fmt::Debug for FileError {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(stringify!(FileError))
                    .field("filename", &self.filename)
                    .field("inner", &self.inner)
                    .finish()
            }
        }
        impl ohno::ErrorExt for FileError {
            fn message(&self) -> String {
                self.inner.format_message(stringify!(FileError), Some(std::borrow::Cow::from(format!("Failed to process file: {}", &self.filename))))
            }
            fn backtrace(&self) -> &std::backtrace::Backtrace {
                self.inner.backtrace()
            }
        }
    };

    let ast: syn::File = syn::parse2(expected).expect("not a valid tokenstream");
    let expected = prettyplease::unparse(&ast);
    assert_eq!(expected, result_string);
}

#[test]
fn test_display_with_format_specifiers_and_lifetime() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        #[display("Failed to process files: {files:?}")]
        struct FilesError<'a> {
            files: Vec<&'a str>,
            #[error]
            inner: OhnoCore,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    let ast: syn::File = syn::parse2(result).expect("not a valid tokenstream");
    let result_string = prettyplease::unparse(&ast);

    let expected = quote! {
        impl<'a> FilesError<'a> {
            #[doc = r" Creates a new error with custom fields and default message."]
            pub(crate) fn new(files: impl Into<Vec<&'a str>>) -> Self {
                Self {
                    files: files.into(),
                    inner: ohno::OhnoCore::default(),
                }
            }

            #[doc = r" Creates a new error with custom fields and a specified error."]
            pub(crate) fn caused_by(files: impl Into<Vec<&'a str>>, error: impl Into<Box<dyn std::error::Error + Send + Sync >>) -> Self {
                Self {
                    files: files.into(),
                    inner: ohno::OhnoCore::from(error),
                }
            }
        }
        impl<'a> From<std::convert::Infallible> for FilesError<'a> {
            fn from(_: std::convert::Infallible) -> Self {
                unreachable!("Infallible should never be converted to Error")
            }
        }
        impl<'a> std::fmt::Display for FilesError<'a> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.inner.format_error(f, stringify!(FilesError), Some(std::borrow::Cow::from(format!("Failed to process files: {:?}", &self.files))))
            }
        }
        impl<'a> std::error::Error for FilesError<'a> {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.inner.source()
            }
        }
        impl<'a> ohno::ErrorTrace for FilesError<'a> {
            fn add_error_trace(&mut self, trace: ohno::TraceInfo) {
                self.inner.add_error_trace(trace);
            }
        }
        impl<'a> ::core::fmt::Debug for FilesError<'a> {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(stringify!(FilesError))
                    .field("files", &self.files)
                    .field("inner", &self.inner)
                    .finish()
            }
        }
        impl<'a> ohno::ErrorExt for FilesError<'a> {
            fn message(&self) -> String {
                self.inner.format_message(stringify!(FilesError), Some(std::borrow::Cow::from(format!("Failed to process files: {:?}", &self.files))))
            }
            fn backtrace(&self) -> &std::backtrace::Backtrace {
                self.inner.backtrace()
            }
        }
    };

    let ast: syn::File = syn::parse2(expected).expect("not a valid tokenstream");
    let expected = prettyplease::unparse(&ast);
    assert_eq!(expected, result_string);
}
