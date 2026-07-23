// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Isolated 1,024-route-per-topology compile and binary-size control.

#![allow(missing_docs, reason = "compile-size fixture needs no API documentation")]

mod fixture {
    include!("generated/literal_controls_1024.rs");
}

fn main() {
    let routers = fixture::Routers::new();
    fixture::assert_equivalent(&routers);
    std::hint::black_box(routers.run(2, 2));
}
