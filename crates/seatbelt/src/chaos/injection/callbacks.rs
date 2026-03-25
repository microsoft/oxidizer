// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::{InjectionOutputArgs, InjectionRateArgs};

crate::utils::define_fn_wrapper!(InjectionOutput<In, Out>(Fn(In, InjectionOutputArgs) -> Out));
crate::utils::define_fn_wrapper!(InjectionRate<In>(Fn(&mut In, InjectionRateArgs) -> f64));
