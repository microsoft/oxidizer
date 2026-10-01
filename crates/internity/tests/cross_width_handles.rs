// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cross-target default threaded-handle serialization fixture.

#![cfg(all(feature = "serde", not(any(target_arch = "sparc64", target_arch = "wasm64"))))]

use internity::se::SerializeReader;
use internity::{Reader, ThreadedLexicon};

// Produced by a 64-bit widening-multiply default-hasher threaded reader.
// The emulated 32-bit path must restore these raw handles, not just the strings.
const CORPUS: &str = r#"["These are some bytes for testing rustc_hash.","uwu",""]"#;
const HANDLES: [(&str, u32); 3] = [
    ("These are some bytes for testing rustc_hash.", 402_653_185),
    ("uwu", 1_744_830_465),
    ("", 2_617_245_697),
];

#[test]
fn default_threaded_corpus_preserves_64_bit_handle_fixture() {
    let restored: ThreadedLexicon = serde_json::from_str(CORPUS).unwrap();
    for &(word, raw) in &HANDLES {
        assert_eq!(restored.get(word).unwrap().as_u32(), raw, "{word:?}");
    }
    let reader = restored.freeze();
    assert_eq!(serde_json::to_string(&SerializeReader(&reader)).unwrap(), CORPUS);
    for &(word, raw) in &HANDLES {
        let sym = internity::Sym::from_u32(raw).unwrap();
        assert_eq!(reader.resolve(sym), word);
    }
}
