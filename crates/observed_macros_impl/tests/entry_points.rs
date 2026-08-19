// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![expect(missing_docs, reason = "Test code")]

use observed_macros_impl::{derive_enrichment, event};
use quote::quote;

// These tests drive the crate's own entry points rather than `internals::*`, so a mutation that
// replaces either body with `Ok(Default::default())` is caught. They carry no snapshots, so unlike
// the sibling test files they also run under miri.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_expands_a_struct() {
        let expanded = event(
            quote!("http.request"),
            quote! {
                #[info]
                struct HttpRequest {
                    method: ClassifiedString,
                    #[unredacted]
                    status: i64,
                }
            },
        )
        .expect("the event attribute expands")
        .to_string();

        assert!(expanded.contains("impl"), "{expanded}");
        assert!(expanded.contains("HttpRequest"), "{expanded}");
    }

    #[test]
    fn event_reports_an_unparsable_item() {
        _ = event(quote!("http.request"), quote!(1 + 1)).expect_err("a non-item is rejected");
    }

    #[test]
    fn derive_enrichment_expands_a_struct() {
        let expanded = derive_enrichment(quote! {
            struct RequestContext {
                method: ClassifiedString,
                #[unredacted]
                status: i64,
            }
        })
        .expect("the derive expands")
        .to_string();

        assert!(expanded.contains("impl"), "{expanded}");
        assert!(expanded.contains("RequestContext"), "{expanded}");
    }

    #[test]
    fn derive_enrichment_reports_an_unparsable_input() {
        _ = derive_enrichment(quote!(1 + 1)).expect_err("a non-derive-input is rejected");
    }
}
