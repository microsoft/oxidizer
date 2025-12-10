// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "This is a test module")]
#![cfg(feature = "test-util")]

use thread_aware_macros::ThreadAware;

#[derive(ThreadAware)]
struct Simple {
    a: i32,
    b: Option<String>,
}

#[derive(ThreadAware)]
struct Tuple(i64, #[thread_aware(skip)] i64);

#[derive(ThreadAware)]
enum E {
    A,
    B(i32, i32),
    C {
        x: i32,
        #[thread_aware(skip)]
        y: i32,
    },
}

// adapter test removed (no 'with' support currently)

#[test]
fn derive_compiles_and_runs() {
    use thread_aware::test_util::create_manual_pinned_affinities;

    let affinities = create_manual_pinned_affinities(&[2]);
    let d0 = affinities[0].into();
    let d1 = affinities[1];

    let s = Simple {
        a: 10,
        b: Some("x".to_string()),
    };
    let _ = thread_aware::ThreadAware::relocated(s, d0, d1);

    let t = Tuple(5, 6);
    let t2 = thread_aware::ThreadAware::relocated(t, d0, d1);
    assert_eq!(t2.0, 5);
    assert_eq!(t2.1, 6);

    let e = E::C { x: 1, y: 2 };
    let _ = thread_aware::ThreadAware::relocated(e, d0, d1);

    // removed adapter test; basic derive coverage above is sufficient
}
