// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process entry-point expansion contracts.

use arty_macros_impl::{main, test};
use quote::quote;

#[test]
fn gamma_survivor_controlled_clock_forces_builder() {
    let output = test(
        quote!(),
        quote!(
            async fn example(cx: arty::task::Context, clock: arty::runtime::ClockControl) {}
        ),
    );
    syn::parse2::<syn::File>(output.clone()).unwrap();
    let text = output.to_string();
    for expected in [
        quote!(::arty::runtime::Runtime::builder()),
        quote!(.clock(::core::clone::Clone::clone(&__arty_clock_control))),
        quote!(.monitor(::arty::runtime::MonitorPolicy::disabled())),
    ] {
        assert!(text.contains(&expected.to_string()), "{expected}");
    }
    assert!(!text.contains(&quote!(Runtime::new()).to_string()));
}

#[test]
fn gamma_survivor_extra_test_arguments_report_exact_diagnostics() {
    for (input, expected) in [
        (
            quote!(
                async fn example(cx: Context, (clock,): Clock) {}
            ),
            "argument must have an identifier",
        ),
        (
            quote!(
                async fn example(cx: Context, r#cx: Clock) {}
            ),
            "arguments must have distinct identifiers",
        ),
        (
            quote!(
                async fn example(cx: Context, clock: Clock, third: Clock) {}
            ),
            "unexpected arguments",
        ),
    ] {
        let output = test(quote!(), input).to_string();
        assert!(output.contains(&quote!(compile_error! { #expected }).to_string()), "{output}");
    }
    let output = main(
        quote!(),
        quote!(
            async fn example(cx: Context, clock: Clock) {}
        ),
    )
    .to_string();
    assert!(output.contains("\"unexpected arguments\""));
}
