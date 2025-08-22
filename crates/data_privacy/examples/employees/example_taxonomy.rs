// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use data_privacy::taxonomy;

#[taxonomy(example)]
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum ExampleTaxonomy {
    PersonallyIdentifiableInformation,
    OrganizationallyIdentifiableInformation,
}
