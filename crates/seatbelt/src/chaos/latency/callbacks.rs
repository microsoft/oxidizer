// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use super::{LatencyDurationArgs, LatencyRateArgs};

crate::utils::define_fn_wrapper!(LatencyDuration<In>(Fn(&In, LatencyDurationArgs) -> Duration));
crate::utils::define_fn_wrapper!(LatencyRate<In>(Fn(&In, LatencyRateArgs) -> f64));
