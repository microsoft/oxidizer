// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod define_fn_wrapper;
pub(crate) use define_fn_wrapper::define_fn_wrapper;

#[cfg(any(feature = "metrics", test))]
mod attributes;
#[cfg(any(feature = "metrics", test))]
pub(crate) use attributes::*;

mod telemetry_helper;
pub(crate) use telemetry_helper::TelemetryHelper;

define_fn_wrapper!(EnableIf<In>(Fn(&In) -> bool));

impl<In> EnableIf<In> {
    /// Creates a new `EnableIf` instance that always returns `true`.
    pub fn always() -> Self {
        Self::new(|_| true)
    }

    /// Creates a new `EnableIf` instance that always returns `false`.
    pub fn never() -> Self {
        Self::new(|_| false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_if_debug() {
        let enable_if: EnableIf<String> = EnableIf::always();
        assert_eq!(format!("{:?}", enable_if), "EnableIf");
    }
}
