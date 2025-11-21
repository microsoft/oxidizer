// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{DataClass, Redactor};
use std::borrow::Cow;
use std::fmt::Write;

/// Mode of operation for the `SimpleRedactor`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SimpleRedactorMode {
    /// Erases the original string.
    Erase,

    /// Erases the original string and tags it with the class id.
    EraseAndTag,

    /// Passes the original string through without modification.
    Passthrough,

    /// Passes the original string through and tags it with the class id.
    PassthroughAndTag,

    /// Replaces the original string with a repeated character.
    Replace(char),

    /// Replaces the original string with a repeated character and tags it with the class id.
    ReplaceAndTag(char),

    /// Inserts a custom string in place of the original string.
    Insert(Cow<'static, str>),

    /// Inserts a custom string in place of the original string and tags it with the class id.
    InsertAndTag(Cow<'static, str>),
}

/// A redactor that performs a variety of simple transformations on the input text.
#[derive(Clone, Debug)]
pub struct SimpleRedactor {
    mode: SimpleRedactorMode,
}

impl SimpleRedactor {
    /// Creates a new instance with the default mode of `SimpleRedactorMode::Replace('*')`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            mode: SimpleRedactorMode::Replace('*'),
        }
    }

    /// Creates a new instance with an explicit mode of operation.
    #[must_use]
    pub const fn with_mode(mode: SimpleRedactorMode) -> Self {
        Self { mode }
    }
}

impl Redactor for SimpleRedactor {
    #[cfg_attr(test, mutants::skip)]
    fn redact(&self, data_class: &DataClass, value: &str, output: &mut dyn Write) -> core::fmt::Result {
        static ASTERISKS: &str = "********************************";

        match &self.mode {
            SimpleRedactorMode::Erase => {
                // nothing
                Ok(())
            }

            SimpleRedactorMode::EraseAndTag => {
                write!(output, "<{data_class}:>")
            }

            SimpleRedactorMode::Passthrough => {
                write!(output, "{value}")
            }

            SimpleRedactorMode::PassthroughAndTag => {
                write!(output, "<{data_class}:{value}>")
            }

            #[expect(clippy::string_slice, reason = "No problem with UTF-8 here")]
            SimpleRedactorMode::Replace(c) => {
                let len = value.len();
                if *c == '*' && len < ASTERISKS.len() {
                    write!(output, "{}", &ASTERISKS[0..len])
                } else {
                    write!(output, "{}", c.to_string().repeat(len))
                }
            }

            #[expect(clippy::string_slice, reason = "No problem with UTF-8 here")]
            SimpleRedactorMode::ReplaceAndTag(c) => {
                let len = value.len();
                if *c == '*' && len < ASTERISKS.len() {
                    write!(output, "<{data_class}:{}>", &ASTERISKS[0..len])
                } else {
                    write!(output, "<{data_class}:{}>", (*c.to_string()).repeat(len))
                }
            }

            SimpleRedactorMode::Insert(s) => {
                write!(output, "{s}")
            }

            SimpleRedactorMode::InsertAndTag(s) => {
                write!(output, "<{data_class}:{s}>")
            }
        }
    }
}

impl Default for SimpleRedactor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CLASS_ID: DataClass = DataClass::new("test_taxonomy", "test_class");
    const TEST_VALUE: &str = "secret";

    fn redact_to_string(redactor: &SimpleRedactor, data_class: &DataClass, value: &str) -> String {
        let mut output = String::new();
        _ = redactor.redact(data_class, value, &mut output);
        output
    }

    #[test]
    fn new_should_create_default_redactor() {
        let redactor = SimpleRedactor::new();
        assert_eq!(redactor.mode, SimpleRedactorMode::Replace('*'));
    }

    #[test]
    fn default_should_be_same_as_new() {
        let r1 = SimpleRedactor::new();
        let r2 = SimpleRedactor::default();
        assert_eq!(r1.mode, r2.mode);
    }

    #[test]
    fn with_mode_should_set_mode() {
        let mode = SimpleRedactorMode::Erase;
        let redactor = SimpleRedactor::with_mode(mode.clone());
        assert_eq!(redactor.mode, mode);
    }

    #[test]
    fn redact_should_erase() {
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);
        let result = redact_to_string(&redactor, &TEST_CLASS_ID, TEST_VALUE);
        assert_eq!(result, "");
    }

    #[test]
    fn redact_should_erase_and_tag() {
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::EraseAndTag);
        let result = redact_to_string(&redactor, &TEST_CLASS_ID, TEST_VALUE);
        assert_eq!(result, "<test_taxonomy/test_class:>");
    }

    #[test]
    fn redact_should_passthrough() {
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough);
        let result = redact_to_string(&redactor, &TEST_CLASS_ID, TEST_VALUE);
        assert_eq!(result, TEST_VALUE);
    }

    #[test]
    fn redact_should_passthrough_and_tag() {
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag);
        let result = redact_to_string(&redactor, &TEST_CLASS_ID, TEST_VALUE);
        assert_eq!(result, format!("<{TEST_CLASS_ID}:{TEST_VALUE}>"));
    }

    #[test]
    fn redact_should_replace_with_asterisks() {
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*'));
        let result = redact_to_string(&redactor, &TEST_CLASS_ID, TEST_VALUE);
        assert_eq!(result, "******");
    }

    #[test]
    fn redact_should_replace_with_char() {
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Replace('#'));
        let result = redact_to_string(&redactor, &TEST_CLASS_ID, TEST_VALUE);
        assert_eq!(result, "######");
    }

    #[test]
    fn redact_should_replace_and_tag_with_asterisks() {
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::ReplaceAndTag('*'));
        let result = redact_to_string(&redactor, &TEST_CLASS_ID, TEST_VALUE);
        assert_eq!(result, format!("<{TEST_CLASS_ID}:******>"));
    }

    #[test]
    fn redact_should_replace_and_tag_with_char() {
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::ReplaceAndTag('#'));
        let result = redact_to_string(&redactor, &TEST_CLASS_ID, TEST_VALUE);
        assert_eq!(result, format!("<{TEST_CLASS_ID}:######>"));
    }

    #[test]
    fn redact_should_insert() {
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("replacement".into()));
        let result = redact_to_string(&redactor, &TEST_CLASS_ID, TEST_VALUE);
        assert_eq!(result, "replacement");
    }

    #[test]
    fn redact_should_insert_and_tag() {
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::InsertAndTag("replacement".into()));
        let result = redact_to_string(&redactor, &TEST_CLASS_ID, TEST_VALUE);
        assert_eq!(result, format!("<{TEST_CLASS_ID}:replacement>"));
    }
}
