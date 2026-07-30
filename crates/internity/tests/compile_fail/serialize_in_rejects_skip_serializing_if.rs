// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use internity::Sym;
use internity::se::SerializeIn;

// `skip_serializing_if` names a runtime predicate that `SerializeIn` cannot honor
// without diverging from the type's ordinary Serde wire schema, so the derive must
// reject it on both named and tuple fields.

#[derive(SerializeIn)]
struct Named {
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<Sym>,
}

#[derive(SerializeIn)]
struct Tuple(#[serde(skip_serializing_if = "Option::is_none")] Option<Sym>);

fn main() {}
