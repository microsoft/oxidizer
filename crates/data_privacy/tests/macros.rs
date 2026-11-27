// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use data_privacy_macros::{RedactedDebug, RedactedDisplay, classified, taxonomy};

#[taxonomy(example)]
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum Taxonomy {
    A,
    B,
}

#[classified(Taxonomy::A)]
#[derive(Clone, Eq, PartialEq, Hash)]
struct EMailAddress(String);

#[derive(Clone, Eq, PartialEq, Hash, RedactedDisplay, RedactedDebug)]
struct Contact {
    #[unredacted]
    name: String,
    email: EMailAddress,
}

#[test]
fn test_pascal_to_snake_case() {
    #[taxonomy(test)]
    enum TestTaxonomy {
        PascalCase,
        AnotherExample,
        Simple,
        WithNumbers123,
    }

    assert_eq!(TestTaxonomy::PascalCase.data_class().name(), "pascal_case");
    assert_eq!(TestTaxonomy::AnotherExample.data_class().name(), "another_example");
    assert_eq!(TestTaxonomy::Simple.data_class().name(), "simple");
    assert_eq!(TestTaxonomy::WithNumbers123.data_class().name(), "with_numbers123");
}

#[test]
fn can_create_instance() {
    let _ = Contact {
        name: "Alice".to_string(),
        email: EMailAddress("a@b.c".to_string()),
    };
}
