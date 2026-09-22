// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashMap;

use crate::sink::{EncodedValues, FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{FieldName, FieldValue};

#[derive(Debug, Default)]
pub(crate) struct TestSink {
    values: HashMap<FieldName, Vec<FieldValue>>,
}

impl TestSink {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

impl FieldSource for TestSink {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        self.values.get(name).and_then(|values| FieldLines::from_slice(name, values))
    }
}

impl FieldSink for TestSink {
    fn set_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        if values.is_empty() {
            self.values.remove(name);
        } else {
            self.values.insert(name.clone(), values.into_iter().collect());
        }
        Ok(())
    }

    fn append_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.values.entry(name.clone()).or_default().extend(values);
        Ok(())
    }

    fn remove_values(&mut self, name: &'static FieldName) {
        self.values.remove(name);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::TestSink;
    use crate::sink::{EncodedValues, FieldSink};
    use crate::source::FieldSource;
    use crate::{FieldName, FieldValue};

    #[test]
    fn setting_no_values_clears_the_entry() {
        let mut sink = TestSink::new();

        sink.set_values(&FieldName::Accept, EncodedValues::single(FieldValue::from_static("text/html")))
            .expect("the test sink always accepts a value");
        assert!(sink.lines(&FieldName::Accept).is_some());

        sink.set_values(&FieldName::Accept, EncodedValues::new())
            .expect("the test sink always accepts an empty set");
        assert!(sink.lines(&FieldName::Accept).is_none());
    }
}
