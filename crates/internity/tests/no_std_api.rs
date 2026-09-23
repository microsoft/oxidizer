// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Package-isolated regression tests for internity's `no_std` public API.

#[cfg(not(feature = "std"))]
#[test]
fn no_std_local_freeze_and_checked_resolution() {
    use internity::{LocalLexicon, Reader, Sym};

    let mut lexicon = LocalLexicon::new();
    let first = lexicon.intern("café");
    let second = lexicon.intern_bytes(b"second").unwrap();
    assert_eq!(lexicon.intern("café"), first);
    assert!(lexicon.intern_bytes(&[0xff]).is_err());
    let live: &dyn Reader = &lexicon;
    assert_eq!(live.try_resolve(first), Some("café"));
    let reader = lexicon.freeze();
    assert_eq!(reader.len(), 2);
    assert_eq!(reader.resolve(first), "café");
    assert_eq!(reader.try_resolve(second), Some("second"));
    assert_eq!(reader.try_resolve(Sym::from_u32(u32::MAX).unwrap()), None);
    assert_eq!(reader.iter().collect::<Vec<_>>(), [(first, "café"), (second, "second")]);
}

#[cfg(all(not(feature = "std"), feature = "serde"))]
#[test]
fn no_std_serde_round_trips_strings_and_handles() {
    use internity::LocalLexicon;
    use internity::se::{SerializeInWith, SerializeReader};

    let mut lexicon = LocalLexicon::new();
    let sym = lexicon.intern("café");
    let reader = lexicon.freeze();
    let encoded = serde_json::to_string(&SerializeInWith::new(&sym, &reader)).unwrap();
    assert_eq!(encoded, r#""café""#);

    let corpus = serde_json::to_string(&SerializeReader(&reader)).unwrap();
    let restored: LocalLexicon = serde_json::from_str(&corpus).unwrap();
    assert_eq!(restored.resolve(sym), "café");
    let mut target = LocalLexicon::new();
    let decoded: internity::Sym = target.deserialize_in(&mut serde_json::Deserializer::from_str(&encoded)).unwrap();
    assert_eq!(target.resolve(decoded), "café");
}
