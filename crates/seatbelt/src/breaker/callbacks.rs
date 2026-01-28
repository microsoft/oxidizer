// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::{BreakerId, OnClosedArgs, OnOpenedArgs, OnProbingArgs, RecoveryArgs, RejectedInputArgs};
use crate::RecoveryInfo;

crate::utils::define_fn_wrapper!(BreakerIdProvider<In>(Fn(&In) -> BreakerId));
crate::utils::define_fn_wrapper!(ShouldRecover<Out>(Fn(&Out, RecoveryArgs) -> RecoveryInfo));
crate::utils::define_fn_wrapper!(RejectedInput<In, Out>(Fn(In, RejectedInputArgs) -> Out));
crate::utils::define_fn_wrapper!(OnProbing<In>(Fn(&mut In, OnProbingArgs)));
crate::utils::define_fn_wrapper!(OnOpened<Out>(Fn(&Out, OnOpenedArgs)));
crate::utils::define_fn_wrapper!(OnClosed<Out>(Fn(&Out, OnClosedArgs)));
