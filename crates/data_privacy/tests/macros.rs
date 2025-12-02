// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use data_privacy_macros::{classified, taxonomy, RedactedDebug, RedactedDisplay};

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
fn can_create_instance() {
    let _ = Contact {
        name: "Alice".to_string(),
        email: EMailAddress("a@b.c".to_string()),
    };
}
