// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builds the single root `vicinal::Pool` the [`Runtime`](crate::runtime::Runtime) owns.

use crate::runtime::config::BlockingPoolPolicy;
use crate::runtime::error::Error;

pub(crate) fn build_pool(policy: &BlockingPoolPolicy) -> Result<vicinal::Pool, Error> {
    let worker_policy = policy.worker_policy()?;
    let builder = vicinal::Pool::builder().name("arty-blocking");
    let builder = match worker_policy {
        Some(worker_policy) => builder.worker_policy(worker_policy),
        None => builder,
    };
    Ok(builder.build())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn policies_build_a_pool() {
        for policy in [BlockingPoolPolicy::shared().max(1), BlockingPoolPolicy::per_worker().max(1)] {
            drop(build_pool(&policy).unwrap());
        }
    }
}
