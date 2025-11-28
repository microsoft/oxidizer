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

    /// Returns the current mode of operation.
    #[must_use]
    pub const fn mode(&self) -> &SimpleRedactorMode {
        &self.mode
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
