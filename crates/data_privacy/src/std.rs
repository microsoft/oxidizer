// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{Debug, Display, Formatter};
use crate::{Classified, DataClass, RedactedDebug, RedactedDisplay, RedactionEngine};

impl Classified for String {
    fn data_class(&self) -> DataClass {
        DataClass::new("std", "string") // TODO: should use sensible "public" taxonomy name
    }
}

impl RedactedDebug for String {
    fn fmt(&self, _: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        <Self as Debug>::fmt(self, f)
    }
}

impl RedactedDisplay for String {
    fn fmt(&self, _: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        <Self as Display>::fmt(self, f)
    }
}

