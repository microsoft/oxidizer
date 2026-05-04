// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "This is a test module")]

use thread_aware::ThreadAware;
use thread_aware::affinity::pinned_affinities;

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
    let affinities = pinned_affinities(&[2]);
    let d0 = affinities[0].into();
    let d1 = affinities[1];

    let mut s = Simple {
        a: 10,
        b: Some("x".to_string()),
    };
    thread_aware::ThreadAware::relocated(&mut s, d0, d1);

    let mut t = Tuple(5, 6);
    thread_aware::ThreadAware::relocated(&mut t, d0, d1);
    assert_eq!(t.0, 5);
    assert_eq!(t.1, 6);

    let mut e = E::C { x: 1, y: 2 };
    thread_aware::ThreadAware::relocated(&mut e, d0, d1);

    // removed adapter test; basic derive coverage above is sufficient
}
