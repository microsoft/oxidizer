// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for layer stacking.

use layered::Stack;
use tower_layer::Identity;

type I = Identity;

#[test]
#[expect(clippy::too_many_lines, reason = "no need to have 16 different unit tests")]
fn stack_tuples() {
    let _: () = (I::new(), ()).build();
    let _: () = (I::new(), I::new(), ()).build();
    let _: () = (I::new(), I::new(), I::new(), ()).build();
    let _: () = (I::new(), I::new(), I::new(), I::new(), ()).build();
    let _: () = (I::new(), I::new(), I::new(), I::new(), I::new(), ()).build();
    let _: () = (I::new(), I::new(), I::new(), I::new(), I::new(), I::new(), ()).build();
    let _: () = (I::new(), I::new(), I::new(), I::new(), I::new(), I::new(), I::new(), ()).build();
    let _: () = (I::new(), I::new(), I::new(), I::new(), I::new(), I::new(), I::new(), I::new(), ()).build();
    let _: () = (
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        (),
    )
        .build();
    let _: () = (
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        (),
    )
        .build();
    let _: () = (
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        (),
    )
        .build();
    let _: () = (
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        (),
    )
        .build();
    let _: () = (
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        (),
    )
        .build();
    let _: () = (
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        (),
    )
        .build();
    let _: () = (
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        (),
    )
        .build();
    let _: () = (
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        I::new(),
        (),
    )
        .build();
}
