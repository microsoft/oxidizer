// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::parse_quote;

use super::*;
use crate::utils::assert_formatted_snapshot;

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
    assert_formatted_snapshot!(result);
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
    assert_formatted_snapshot!(result);
}
