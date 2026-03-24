// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::InjectionOutputArgs;

crate::utils::define_fn_wrapper!(InjectionOutput<In, Out>(Fn(In, InjectionOutputArgs) -> Out));
