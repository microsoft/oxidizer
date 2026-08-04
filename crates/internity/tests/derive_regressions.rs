// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for `DeserializeIn` derive expansion.

#![cfg(feature = "serde")]

use internity::de::DeserializeIn;
use internity::{LocalLexicon, Sym};

#[derive(DeserializeIn)]
struct __I {
    value: Sym,
}

#[derive(DeserializeIn)]
struct InternerField {
    __interner: Sym,
    other: Sym,
}

#[derive(DeserializeIn)]
struct RawIdentifier {
    r#type: Sym,
}

#[test]
fn struct_name_does_not_collide_with_generated_interner_parameter() {
    let mut lexicon: LocalLexicon = LocalLexicon::new();
    let value: __I = lexicon
        .deserialize_in(&mut serde_json::Deserializer::from_str(r#"{"value":"name"}"#))
        .unwrap();

    assert_eq!(lexicon.resolve(value.value), "name");
}

#[test]
fn interner_named_field_works_in_sequence_and_map_forms() {
    let mut lexicon: LocalLexicon = LocalLexicon::new();
    let sequence: InternerField = lexicon
        .deserialize_in(&mut serde_json::Deserializer::from_str(r#"["first","second"]"#))
        .unwrap();
    let map: InternerField = lexicon
        .deserialize_in(&mut serde_json::Deserializer::from_str(
            r#"{"__interner":"third","other":"fourth"}"#,
        ))
        .unwrap();

    assert_eq!(lexicon.resolve(sequence.__interner), "first");
    assert_eq!(lexicon.resolve(sequence.other), "second");
    assert_eq!(lexicon.resolve(map.__interner), "third");
    assert_eq!(lexicon.resolve(map.other), "fourth");
}

#[test]
fn raw_identifier_uses_unraw_wire_name() {
    let mut lexicon: LocalLexicon = LocalLexicon::new();
    let value: RawIdentifier = lexicon
        .deserialize_in(&mut serde_json::Deserializer::from_str(r#"{"type":"keyword"}"#))
        .unwrap();

    assert_eq!(lexicon.resolve(value.r#type), "keyword");
}
