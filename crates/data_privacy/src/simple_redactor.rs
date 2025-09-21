// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{DataClass, Redactor};
use std::borrow::Cow;

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
    fn redact(&self, data_class: &DataClass, value: &str, output: &mut dyn FnMut(&str)) {
        static ASTERISKS: &str = "********************************";

        match &self.mode {
            SimpleRedactorMode::Erase => {
                // nothing
            }
            SimpleRedactorMode::EraseAndTag => {
                output(format!("<{data_class}:>").as_str());
            }
            SimpleRedactorMode::Passthrough => {
                output(value);
            }
            SimpleRedactorMode::PassthroughAndTag => {
                output(format!("<{data_class}:{value}>").as_str());
            }

            #[expect(clippy::string_slice, reason = "No problem with UTF-8 here")]
            SimpleRedactorMode::Replace(c) => {
                let len = value.len();
                if *c == '*' && len < ASTERISKS.len() {
                    output(&ASTERISKS[0..len]);
                } else {
                    output(c.to_string().repeat(len).as_str());
                }
            }

            #[expect(clippy::string_slice, reason = "No problem with UTF-8 here")]
            SimpleRedactorMode::ReplaceAndTag(c) => {
                let len = value.len();
                if *c == '*' && len < ASTERISKS.len() {
                    output(format!("<{data_class}:{}>", &ASTERISKS[0..len]).as_str());
                } else {
                    output(format!("<{data_class}:{}>", (*c.to_string()).repeat(len).as_str()).as_str());
                }
            }
            SimpleRedactorMode::Insert(s) => {
                output(s);
            }
            SimpleRedactorMode::InsertAndTag(s) => {
                output(format!("<{data_class}:{s}>").as_str());
            }
        }
    }

    fn exact_len(&self) -> Option<usize> {
        matches!(&self.mode, SimpleRedactorMode::Erase).then_some(0)
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
        redactor.redact(data_class, value, &mut |s| output.push_str(s));
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

    #[test]
    fn exact_len_should_return_expected_values_for_all_modes() {
        // Erase mode should return Some(0) as it produces no output
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);
        assert_eq!(redactor.exact_len(), Some(0));

        // EraseAndTag mode should return None as output length depends on data class
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::EraseAndTag);
        assert_eq!(redactor.exact_len(), None);

        // Passthrough mode should return None as output length depends on input
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough);
        assert_eq!(redactor.exact_len(), None);

        // PassthroughAndTag mode should return None as output length depends on input and data class
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag);
        assert_eq!(redactor.exact_len(), None);

        // Replace mode should return None as output length depends on input length
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*'));
        assert_eq!(redactor.exact_len(), None);

        // ReplaceAndTag mode should return None as output length depends on input length and data class
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::ReplaceAndTag('*'));
        assert_eq!(redactor.exact_len(), None);

        // Insert mode should return None as output length depends on the inserted string
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Insert("replacement".into()));
        assert_eq!(redactor.exact_len(), None);

        // InsertAndTag mode should return None as output length depends on inserted string and data class
        let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::InsertAndTag("replacement".into()));
        assert_eq!(redactor.exact_len(), None);
    }
}
