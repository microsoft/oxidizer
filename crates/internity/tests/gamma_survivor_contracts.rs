// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public symbol and reader contracts.

use internity::{Lexicon, LocalLexicon, Reader};

#[test]
fn gamma_round2_from_symbol_keeps_noninitial_raw_handle() {
    let mut lexicon = LocalLexicon::new();
    lexicon.intern("first");
    let second = lexicon.intern("second");
    assert_eq!(u32::from(second), 2);
}

#[cfg(feature = "std")]
#[test]
fn gamma_round2_bytes_and_text_share_threaded_dedup_table() {
    use internity::ThreadedLexicon;

    let lexicon = ThreadedLexicon::new();
    for index in 0..96 {
        let text = format!("entry-{index}-with-a-distinct-hash");
        let symbol = lexicon.intern(&text);
        assert_eq!(lexicon.intern_bytes(text.as_bytes()).unwrap(), symbol);
    }
    for index in 96..192 {
        let text = format!("entry-{index}-with-a-distinct-hash");
        let symbol = lexicon.intern_bytes(text.as_bytes()).unwrap();
        assert_eq!(lexicon.intern(&text), symbol);
    }
    assert_eq!(lexicon.len(), 192);
}

#[test]
fn gamma_survivor_dense_handles_debug_and_trait_length() {
    let mut lexicon = LocalLexicon::new();
    assert_eq!(format!("{lexicon:?}"), "LocalLexicon { len: 0, .. }");
    assert_eq!(Lexicon::len(&lexicon), 0);
    for (index, text) in ["", "first", "second", "third"].into_iter().enumerate() {
        let symbol = lexicon.intern(text);
        assert_eq!(symbol.as_u32(), u32::try_from(index + 1).unwrap());
        assert_eq!(lexicon.sym_at(index), Some(symbol));
        assert_eq!(lexicon.index_of(symbol), Some(index));
        assert_eq!(lexicon.resolve(symbol), text);
        assert_eq!(Lexicon::len(&lexicon), index + 1);
        assert_eq!(Reader::len(&lexicon), index + 1);
    }
    assert_eq!(format!("{lexicon:?}"), "LocalLexicon { len: 4, .. }");
    assert_eq!(
        lexicon.iter().map(|(_, text)| text).collect::<Vec<_>>(),
        ["", "first", "second", "third"]
    );
    let reader = lexicon.freeze();
    assert_eq!(Reader::len(&reader), 4);
}

#[cfg(feature = "std")]
#[test]
fn gamma_survivor_threaded_reader_debug_and_snapshot_invalidation() {
    use internity::ThreadedLexicon;

    let lexicon = ThreadedLexicon::new();
    let first = lexicon.intern("first");
    let before = lexicon.clone().freeze();
    assert_eq!(format!("{before:?}"), "ThreadedReader { len: 1, .. }");
    let second = lexicon.intern("second");
    let after = lexicon.freeze();
    assert_eq!(format!("{after:?}"), "ThreadedReader { len: 2, .. }");
    assert_eq!(Reader::len(&before), 1);
    assert_eq!(Reader::len(&after), 2);
    assert_eq!(before.resolve(first), "first");
    assert_eq!(after.resolve(first), "first");
    assert_eq!(after.resolve(second), "second");
}

#[cfg(feature = "serde")]
#[test]
fn gamma_survivor_collection_deserializer_errors_are_not_empty_successes() {
    use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

    use internity::Sym;

    let mut lexicon = LocalLexicon::new();
    lexicon
        .deserialize_in::<Vec<Sym>, _>(&mut serde_json::Deserializer::from_str("[\"good\", 4]"))
        .unwrap_err();
    lexicon
        .deserialize_in::<HashSet<Sym>, _>(&mut serde_json::Deserializer::from_str("[\"good\", 4]"))
        .unwrap_err();
    lexicon
        .deserialize_in::<BTreeSet<bool>, _>(&mut serde_json::Deserializer::from_str("[true, 4]"))
        .unwrap_err();
    lexicon
        .deserialize_in::<HashMap<Sym, Sym>, _>(&mut serde_json::Deserializer::from_str("{\"key\":4}"))
        .unwrap_err();
    lexicon
        .deserialize_in::<BTreeMap<String, Sym>, _>(&mut serde_json::Deserializer::from_str("{\"key\":4}"))
        .unwrap_err();
    lexicon
        .deserialize_in::<HashMap<u32, Sym>, _>(&mut serde_json::Deserializer::from_str("{\"bad\":\"value\"}"))
        .unwrap_err();
    lexicon
        .deserialize_in::<BTreeMap<u32, Sym>, _>(&mut serde_json::Deserializer::from_str("{\"bad\":\"value\"}"))
        .unwrap_err();
}
