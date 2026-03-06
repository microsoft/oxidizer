// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use super::args::{CloneArgs, HedgingDelayArgs, OnExecuteArgs, RecoveryArgs};
use crate::RecoveryInfo;

crate::utils::define_fn_wrapper!(CloneInput<In>(Fn(&mut In, CloneArgs) -> Option<In>));
crate::utils::define_fn_wrapper!(ShouldRecover<Out>(Fn(&Out, RecoveryArgs) -> RecoveryInfo));
crate::utils::define_fn_wrapper!(OnExecute<In>(Fn(&mut In, OnExecuteArgs)));
crate::utils::define_fn_wrapper!(DelayFn<In>(Fn(&In, HedgingDelayArgs) -> Duration));

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn delay_fn_debug() {
        let delay_fn = DelayFn::<String>::new(|_input, _args| Duration::from_secs(1));
        let debug_str = format!("{delay_fn:?}");
        assert_eq!(debug_str, "DelayFn");
    }
}
