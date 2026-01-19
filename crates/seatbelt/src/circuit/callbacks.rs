// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::{OnClosedArgs, OnOpenedArgs, OnProbingArgs, PartitionKey, RecoveryArgs, RejectedInputArgs};
use crate::RecoveryInfo;

crate::define_fn_wrapper!(PartionKeyProvider<In>(Fn(&In) -> PartitionKey));
crate::define_fn_wrapper!(ShouldRecover<Out>(Fn(&Out, RecoveryArgs) -> RecoveryInfo));
crate::define_fn_wrapper!(RejectedInput<In, Out>(Fn(In, RejectedInputArgs) -> Out));
crate::define_fn_wrapper!(OnProbing<In>(Fn(&mut In, OnProbingArgs)));
crate::define_fn_wrapper!(OnOpened<Out>(Fn(&Out, OnOpenedArgs)));
crate::define_fn_wrapper!(OnClosed<Out>(Fn(&Out, OnClosedArgs)));
