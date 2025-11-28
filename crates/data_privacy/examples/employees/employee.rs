// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::example_taxonomy::ExampleTaxonomy;
use data_privacy_macros::{classified, RedactedDebug, RedactedDisplay};
use derive_more::{Constructor, From};
use serde::{Deserialize, Serialize};

#[classified(ExampleTaxonomy::PersonallyIdentifiableInformation)]
#[derive(Clone, Hash, Serialize, Deserialize, Constructor, From)]
pub struct UserName(String);

#[classified(ExampleTaxonomy::PersonallyIdentifiableInformation)]
#[derive(Clone, Serialize, Deserialize, Constructor, From)]
pub struct UserAddress(String);

#[classified(ExampleTaxonomy::OrganizationallyIdentifiableInformation)]
#[derive(Clone, Serialize, Deserialize, Constructor, From)]
pub struct EmployeeID(String);

/// Holds info about a single corporate employee.
#[derive(Serialize, Deserialize, Clone, RedactedDebug, RedactedDisplay)]
pub struct Employee {
    pub name: UserName,
    pub address: UserAddress,
    pub id: EmployeeID,
    pub age: u32,
}
