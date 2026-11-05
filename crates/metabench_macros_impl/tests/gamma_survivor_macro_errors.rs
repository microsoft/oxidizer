// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Macro errors remain results without generated-code compilation.

use metabench_macros_impl::{benchmark, main};
use quote::quote;

#[test]
fn gamma_round2_missing_dependency_errors_name_metabench_without_panicking() {
    let benchmark_error = benchmark(
        quote!(ID, GROUP, "case"),
        quote!(
            fn measured() {}
        ),
    )
    .unwrap_err()
    .to_string();
    let main_error = main(quote!(criterion = benches, benchmarks = [ID])).unwrap_err().to_string();
    assert_eq!(benchmark_error, main_error);
    assert!(benchmark_error.contains("`metabench`"), "{benchmark_error}");
    assert!(benchmark_error.contains("dependencies"), "{benchmark_error}");
}

#[test]
fn gamma_survivor_public_macro_errors_are_results() {
    assert_eq!(
        benchmark(
            quote!(identity = ID, benchmark_name = "b"),
            quote!(
                fn f() {}
            )
        )
        .unwrap_err()
        .to_string(),
        "unexpected end of input, missing `group_name`"
    );
    main(quote!(criterion = c, benchmarks = [1])).unwrap_err();
    let oversized = std::iter::repeat_n(quote!(x), 129).collect();
    let error = benchmark(
        oversized,
        quote!(
            fn f() {}
        ),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(
        error,
        "macro arguments are too complex (nested too deeply, or a single field's value has too many tokens); simplify the invocation"
    );
    let oversized = std::iter::repeat_n(quote!(x), 129).collect();
    assert_eq!(main(oversized).unwrap_err().to_string(), error);
}
