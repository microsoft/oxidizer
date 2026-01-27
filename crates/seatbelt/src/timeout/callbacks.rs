// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use super::{OnTimeoutArgs, TimeoutOutputArgs, TimeoutOverrideArgs};

crate::utils::define_fn_wrapper!(TimeoutOutput<Out>(Fn(TimeoutOutputArgs) -> Out));
crate::utils::define_fn_wrapper!(OnTimeout<Out>(Fn(&Out, OnTimeoutArgs)));
crate::utils::define_fn_wrapper!(TimeoutOverride<In>(Fn(&In, TimeoutOverrideArgs) -> Option<Duration>));
