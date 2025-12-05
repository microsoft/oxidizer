// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use data_privacy::{Classified, DataClass};

#[derive(Debug, Clone)]
struct ClassifiedExample {
    _data: u32,
}

impl Classified for ClassifiedExample {
    fn data_class(&self) -> DataClass {
        DataClass::new("example", "classified_example")
    }
}

#[test]
fn test_default_trait_methods() {
    let classified = ClassifiedExample { _data: 42 };
    assert_eq!(classified.data_class().name(), "classified_example");
}
