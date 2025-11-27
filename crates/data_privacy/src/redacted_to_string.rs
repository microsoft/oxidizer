// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{RedactedDisplay, RedactionEngine};

/// Trait for converting a value to a redacted string representation.
pub trait RedactedToString {
    /// Converts the value to a redacted string representation.
    fn to_string(&self, engine: &RedactionEngine) -> String;
}

impl<T: RedactedDisplay + ?Sized> RedactedToString for T {
    fn to_string(&self, engine: &RedactionEngine) -> String {
        struct Adapter<'a, T: ?Sized> {
            inner: &'a T,
            engine: &'a RedactionEngine,
        }

        impl<T: RedactedDisplay + ?Sized> std::fmt::Display for Adapter<'_, T> {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                <T as RedactedDisplay>::fmt(self.inner, self.engine, f)
            }
        }

        Adapter {
            inner: self,
            engine,
        }
        .to_string()
    }
}