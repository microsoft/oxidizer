// Copyright (c) Microsoft Corporation.

use std::time::Duration;

use super::{OnTimeoutArgs, TimeoutOutputArgs, TimeoutOverrideArgs};

crate::define_fn_wrapper!(TimeoutOutput<Out>(Fn(TimeoutOutputArgs) -> Out));
crate::define_fn_wrapper!(OnTimeout<Out>(Fn(&Out, OnTimeoutArgs)));
crate::define_fn_wrapper!(TimeoutOverride<In>(Fn(&In, TimeoutOverrideArgs) -> Option<Duration>));
