// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::sink::{EncodedValues, FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{FieldName, FieldValue};

pub(super) struct TestMap {
    pub(super) name: &'static FieldName,
    pub(super) values: Vec<FieldValue>,
}

impl TestMap {
    pub(super) fn new(name: &'static FieldName, values: Vec<FieldValue>) -> Self {
        Self { name, values }
    }
}

impl FieldSource for TestMap {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == self.name).then(|| FieldLines::from_slice(name, &self.values)).flatten()
    }
}

impl FieldSink for TestMap {
    fn set_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.name = name;
        self.values = values.into_iter().collect();
        Ok(())
    }

    fn append_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        if name == self.name {
            self.values.extend(values);
        } else {
            self.name = name;
            self.values = values.into_iter().collect();
        }
        Ok(())
    }

    fn remove_values(&mut self, name: &'static FieldName) {
        if name == self.name {
            self.values.clear();
        }
    }
}
