// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashMap;

use http_headers::sink::{EncodedValues, FieldSink, InsertError};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{FieldName, FieldValue};

#[derive(Default)]
pub(crate) struct TestMap(pub(crate) HashMap<FieldName, Vec<FieldValue>>);

impl TestMap {
    pub(crate) fn get_all(&self, name: &FieldName) -> &[FieldValue] {
        self.0.get(name).map_or(&[], Vec::as_slice)
    }
}

impl FieldSource for TestMap {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        FieldLines::from_slice(name, self.get_all(name))
    }
}

impl FieldSink for TestMap {
    fn set_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        if values.is_empty() {
            self.0.remove(name);
        } else {
            self.0.insert(name.clone(), values.into_iter().collect());
        }
        Ok(())
    }

    fn append_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.0.entry(name.clone()).or_default().extend(values);
        Ok(())
    }

    fn remove_values(&mut self, name: &'static FieldName) {
        self.0.remove(name);
    }
}
