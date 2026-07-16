// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test taxonomy and classified newtypes for redaction testing.
//!
//! These types go through `RedactedDisplay` / the `RedactionEngine` at emit time,
//! unlike primitive types which are passed through directly.

/// Test taxonomy with three data classes for testing redaction rules.
#[derive(Debug)]
#[data_privacy::taxonomy(test_taxonomy)]
pub enum TestTaxonomy {
    /// Public non-personal data.
    PublicData,
    /// Personally identifiable information.
    Pii,
    /// Secret/credential data.
    Secret,
}

/// A classified string type tagged as PII.
#[data_privacy::classified(TestTaxonomy::Pii)]
#[derive(Clone)]
pub struct PiiString(pub String);

/// A classified string type tagged as secret.
#[data_privacy::classified(TestTaxonomy::Secret)]
#[derive(Clone)]
pub struct SecretString(pub String);

/// A classified string type tagged as public.
#[data_privacy::classified(TestTaxonomy::PublicData)]
#[derive(Clone)]
pub struct PublicString(pub String);

/// A classified `i64` type tagged as public.
#[data_privacy::classified(TestTaxonomy::PublicData)]
#[derive(Clone, Copy)]
pub struct PublicI64(pub i64);

/// A classified `f64` type tagged as public.
#[data_privacy::classified(TestTaxonomy::PublicData)]
#[derive(Clone, Copy)]
pub struct PublicF64(pub f64);

/// A classified `bool` type tagged as public.
#[data_privacy::classified(TestTaxonomy::PublicData)]
#[derive(Clone, Copy)]
pub struct PublicBool(pub bool);
