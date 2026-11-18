// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Redaction output and formatter failure contracts.

use std::fmt::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use data_privacy::{DataClass, RedactionEngine, Redactor, Sensitive};

const CLASS: DataClass = DataClass::new("gamma", "secret");

#[test]
fn gamma_survivor_asterisk_boundary_preserves_fallible_writer_dispatch() {
    struct RejectCharacters {
        text: String,
        characters: usize,
    }

    impl Write for RejectCharacters {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            self.text.push_str(s);
            Ok(())
        }

        fn write_char(&mut self, _: char) -> fmt::Result {
            self.characters += 1;
            Err(fmt::Error)
        }
    }

    for tagged in [false, true] {
        for length in [119, 120] {
            let mut output = RejectCharacters {
                text: String::new(),
                characters: 0,
            };
            let mode = if tagged {
                SimpleRedactorMode::ReplaceAndTag('*')
            } else {
                SimpleRedactorMode::Replace('*')
            };
            let result = SimpleRedactor::with_mode(mode).redact(&CLASS, &"x".repeat(length), &mut output);
            if length == 119 {
                assert_eq!(result, Ok(()));
                assert_eq!(output.characters, 0);
                let stars = "*".repeat(length);
                assert_eq!(output.text, if tagged { format!("<gamma/secret:{stars}>") } else { stars });
            } else {
                assert_eq!(result, Err(fmt::Error));
                assert_eq!(output.characters, 1);
                assert_eq!(output.text, if tagged { "<gamma/secret:" } else { "" });
            }
        }
    }
}

struct FailAfter(usize);

impl Write for FailAfter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if s.len() > self.0 {
            return Err(fmt::Error);
        }
        self.0 -= s.len();
        Ok(())
    }
}

#[test]
fn gamma_survivor_replacement_modes_preserve_character_and_exact_boundary_output() {
    for length in [0, 1, 127, 128, 129, 255] {
        let input = "é".repeat(length);
        for character in ['*', '#', '界'] {
            for tagged in [false, true] {
                let mode = if tagged {
                    SimpleRedactorMode::ReplaceAndTag(character)
                } else {
                    SimpleRedactorMode::Replace(character)
                };
                let redactor = SimpleRedactor::with_mode(mode);
                let mut output = String::new();
                redactor.redact(&CLASS, &input, &mut output).unwrap();
                let replacement = character.to_string().repeat(input.len());
                let expected = if tagged {
                    format!("<gamma/secret:{replacement}>")
                } else {
                    replacement
                };
                assert_eq!(output, expected);
            }
        }
    }
}

#[test]
fn gamma_survivor_redactors_propagate_failure_at_prefix_and_each_replacement() {
    for (mode, budget) in [
        (SimpleRedactorMode::Replace('#'), 0),
        (SimpleRedactorMode::Replace('#'), 2),
        (SimpleRedactorMode::ReplaceAndTag('#'), 0),
        (SimpleRedactorMode::ReplaceAndTag('#'), "<gamma/secret:".len() + 2),
    ] {
        assert_eq!(
            SimpleRedactor::with_mode(mode).redact(&CLASS, "abcdef", &mut FailAfter(budget)),
            Err(fmt::Error)
        );
    }
}

struct Rendered {
    text: String,
    calls: AtomicUsize,
}

impl fmt::Display for Rendered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.calls.fetch_add(1, Ordering::Relaxed);
        f.write_str(&self.text)
    }
}

impl fmt::Debug for Rendered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[test]
fn gamma_survivor_sensitive_formats_once_at_stack_limit_and_twice_above_it() {
    let engine = RedactionEngine::builder()
        .add_class_redactor(CLASS, SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough))
        .build();
    for length in [0, 127, 128, 129, 256] {
        let value = Sensitive::new(
            Rendered {
                text: "a".repeat(length),
                calls: AtomicUsize::new(0),
            },
            CLASS,
        );
        let mut output = String::new();
        engine.redacted_display(&value, &mut output).unwrap();
        assert_eq!(output, "a".repeat(length));
        let expected_calls = if length <= 128 { 1 } else { 2 };
        assert_eq!(value.declassify_ref().calls.load(Ordering::Relaxed), expected_calls);
        output.clear();
        engine.redacted_debug(&value, &mut output).unwrap();
        assert_eq!(output, "a".repeat(length));
        assert_eq!(value.declassify_ref().calls.load(Ordering::Relaxed), expected_calls * 2);
    }
}
