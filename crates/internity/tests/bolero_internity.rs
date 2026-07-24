// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(not(miri))]

//! Property tests for interning, freezing, iteration, and arbitrary handles.

use std::collections::HashMap;

use internity::{Lexicon, Reader, Sym, ThreadedLexicon};

fn words(input: &[u8]) -> Vec<String> {
    input
        .split(|&byte| byte == 0xff)
        .take(64)
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .collect()
}

#[test]
fn arbitrary_strings_preserve_dedup_and_resolution() {
    bolero::check!().for_each(|input: &[u8]| {
        let words = words(input);
        let mut lexicon = Lexicon::new();
        let threaded = ThreadedLexicon::new();
        let mut lexicon_handles = HashMap::new();
        let mut threaded_handles = HashMap::new();

        for word in &words {
            let sym = lexicon.intern(word);
            if let Some(previous) = lexicon_handles.insert(word.clone(), sym) {
                assert_eq!(sym, previous);
            }
            assert_eq!(lexicon.resolve(sym), word);

            let sym = threaded.intern(word);
            if let Some(previous) = threaded_handles.insert(word.clone(), sym) {
                assert_eq!(sym, previous);
            }
            assert_eq!(threaded.get(word), Some(sym));
        }

        assert_eq!(lexicon.len(), lexicon_handles.len());
        assert_eq!(threaded.len(), threaded_handles.len());

        for (word, sym) in &lexicon_handles {
            assert_eq!(lexicon.get(word), Some(*sym));
            assert_eq!(lexicon.resolve(*sym), word);
        }

        let reader = lexicon.freeze();
        for (word, sym) in &lexicon_handles {
            assert_eq!(reader.try_resolve(*sym), Some(word.as_str()));
        }
        let iterated: HashMap<_, _> = reader.iter().map(|(sym, word)| (word.to_owned(), sym)).collect();
        assert_eq!(iterated, lexicon_handles);

        let reader = threaded.freeze();
        for (word, sym) in &threaded_handles {
            assert_eq!(reader.try_resolve(*sym), Some(word.as_str()));
        }
        let iterated: HashMap<_, _> = reader.iter().map(|(sym, word)| (word.to_owned(), sym)).collect();
        assert_eq!(iterated, threaded_handles);
    });
}

#[test]
fn arbitrary_raw_handles_are_range_checked() {
    bolero::check!().for_each(|input: &[u8]| {
        let mut lexicon = Lexicon::new();
        let mut strings = Vec::new();
        for word in words(input) {
            let sym = lexicon.intern(&word);
            if sym.as_u32() as usize > strings.len() {
                strings.push(word);
            }
        }
        let reader = lexicon.freeze();

        let raw_handles = input
            .chunks(4)
            .map(|chunk| {
                let mut bytes = [0; 4];
                bytes[..chunk.len()].copy_from_slice(chunk);
                u32::from_le_bytes(bytes)
            })
            .chain([0, 1, u32::MAX]);

        for raw in raw_handles {
            let Some(sym) = Sym::from_u32(raw) else {
                assert_eq!(raw, 0);
                continue;
            };
            let expected = raw.checked_sub(1).and_then(|index| strings.get(index as usize)).map(String::as_str);
            assert_eq!(reader.try_resolve(sym), expected);
        }
    });
}
