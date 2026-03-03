// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use super::args::{CloneArgs, HedgingDelayArgs, OnExecuteArgs, RecoveryArgs};
use crate::RecoveryInfo;

crate::utils::define_fn_wrapper!(CloneInput<In>(Fn(&mut In, CloneArgs) -> Option<In>));
crate::utils::define_fn_wrapper!(ShouldRecover<Out>(Fn(&Out, RecoveryArgs) -> RecoveryInfo));
crate::utils::define_fn_wrapper!(OnExecute<In>(Fn(&mut In, OnExecuteArgs)));
crate::utils::define_fn_wrapper!(DelayFn(Fn(HedgingDelayArgs) -> Duration));
