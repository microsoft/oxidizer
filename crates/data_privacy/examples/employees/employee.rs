// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use serde::{Deserialize, Serialize};

use crate::example_taxonomy::{OrganizationallyIdentifiableInformation, PersonallyIdentifiableInformation};

/// Holds info about a single corporate employee.
#[derive(Serialize, Deserialize, Clone)]
pub struct Employee {
    pub name: PersonallyIdentifiableInformation<String>,
    pub address: PersonallyIdentifiableInformation<String>,
    pub id: OrganizationallyIdentifiableInformation<String>,
    pub age: u32,
}
