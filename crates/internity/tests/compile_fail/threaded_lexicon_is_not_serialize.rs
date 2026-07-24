// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use internity::ThreadedLexicon;

// A live `ThreadedLexicon` must not be serialized directly: doing so would race a
// concurrent writer and persist the interner's internal handle layout. Callers
// freeze it and serialize the resulting `Reader` with `SerializeReader` instead.

fn assert_serialize<T: serde::Serialize>() {}

fn main() {
    assert_serialize::<ThreadedLexicon>();
}
