// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use ohno_macros_impl::marker::*;
use syn::Attribute;

#[cfg(test)]
mod tests {
    use syn::parse::Parser as _;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn recognizes_the_marker_it_writes() {
        let attrs = Attribute::parse_outer
            .parse2(generated_marker())
            .expect("the generated marker parses");
        assert!(attrs.iter().any(is_generated_marker));
    }

    #[test]
    fn recognizes_a_hand_written_doc_comment_carrying_the_marker() {
        let attr: Attribute = parse_quote!(#[doc = " ohno::generated-core@7f3d9c2a"]);
        assert!(is_generated_marker(&attr));
    }

    #[test]
    fn an_ordinary_doc_comment_is_not_the_marker() {
        let attr: Attribute = parse_quote!(#[doc = " Where the failure happened."]);
        assert!(!is_generated_marker(&attr));
    }

    #[test]
    fn a_doc_comment_mentioning_the_marker_is_not_the_marker() {
        let attr: Attribute = parse_quote!(#[doc = " The ohno generated core field is not this one."]);
        assert!(!is_generated_marker(&attr));
    }

    #[test]
    fn a_non_doc_attribute_is_not_the_marker() {
        let attr: Attribute = parse_quote!(#[error]);
        assert!(!is_generated_marker(&attr));

        let attr: Attribute = parse_quote!(#[doc(hidden)]);
        assert!(!is_generated_marker(&attr));

        let attr: Attribute = parse_quote!(#[doc = 1]);
        assert!(!is_generated_marker(&attr));

        let attr: Attribute = parse_quote!(#[doc = concat!(" ohno::generated-core@7f3d9c2a")]);
        assert!(!is_generated_marker(&attr));

        let attr: Attribute = parse_quote!(#[other = " ohno::generated-core@7f3d9c2a"]);
        assert!(!is_generated_marker(&attr));
    }
}
