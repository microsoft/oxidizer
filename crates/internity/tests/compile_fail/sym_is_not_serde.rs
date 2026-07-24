// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use internity::Sym;

// A bare `Sym` is a lexicon-local handle, meaningless without its interner, so it
// deliberately implements neither `serde::Serialize` nor `serde::Deserialize`.

fn assert_serialize<T: serde::Serialize>() {}
fn assert_deserialize<'de, T: serde::Deserialize<'de>>() {}

fn main() {
    assert_serialize::<Sym>();
    assert_deserialize::<Sym>();
}
