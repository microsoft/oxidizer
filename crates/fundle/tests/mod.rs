// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::thread::Thread;
use trybuild::TestCases;

#[test]
#[cfg_attr(miri, ignore)]
fn proc() {
    let t = TestCases::new();

    t.pass("tests/proc/bundle_empty.rs");
    t.compile_fail("tests/proc/bundle_enum.rs");
    t.pass("tests/proc/bundle_forward.rs");
    t.pass("tests/proc/bundle_simple.rs");
    t.compile_fail("tests/proc/bundle_tupled.rs");
    t.pass("tests/proc/deps_simple.rs");
    t.pass("tests/proc/newtype_simple.rs");
}

//
// #[test]
// fn ff() {
//     let result = std::thread::Builder::new().stack_size(32*1024*1024).spawn(|| {
//         let x = [0; 4 * 1024 * 1024];
//         dbg!("Other")
//     });
//     dbg!(result.unwrap().join().unwrap());
// }