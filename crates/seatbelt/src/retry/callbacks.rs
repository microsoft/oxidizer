// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::{CloneArgs, OnRetryArgs, RecoveryArgs, RestoreInputArgs};
use crate::RecoveryInfo;

crate::define_fn_wrapper!(CloneInput<In>(Fn(&mut In, CloneArgs) -> Option<In>));
crate::define_fn_wrapper!(ShouldRecover<Out>(Fn(&Out, RecoveryArgs) -> RecoveryInfo));
crate::define_fn_wrapper!(OnRetry<Out>(Fn(&Out, OnRetryArgs)));
crate::define_fn_wrapper!(RestoreInput<In, Out>(Fn(&mut Out, RestoreInputArgs) -> Option<In>));
