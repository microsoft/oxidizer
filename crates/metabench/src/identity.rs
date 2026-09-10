// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

/// Identifies one logical benchmark across all supported engines: Criterion,
/// Gungraun, allocation tracking, and perf-based instruction counting.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BenchmarkIdentity {
    group: &'static str,
    benchmark: &'static str,
    gungraun_group: &'static str,
    gungraun_benchmark: &'static str,
    allocation_tracking: bool,
}

impl fmt::Display for BenchmarkIdentity {
    #[expect(clippy::renamed_function_params, reason = "the descriptive name improves readability")]
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.group, self.benchmark)
    }
}

impl BenchmarkIdentity {
    /// Creates an identity for generated macro code.
    #[doc(hidden)]
    #[must_use]
    pub const fn __new(
        group: &'static str,
        benchmark: &'static str,
        gungraun_group: &'static str,
        gungraun_benchmark: &'static str,
        allocation_tracking: bool,
    ) -> Self {
        Self {
            group,
            benchmark,
            gungraun_group,
            gungraun_benchmark,
            allocation_tracking,
        }
    }

    /// Replaces the generated Gungraun group for generated macro code.
    #[doc(hidden)]
    #[must_use]
    pub const fn __in_gungraun_group(mut self, group: &'static str) -> Self {
        self.gungraun_group = group;
        self
    }

    /// Returns the logical group name.
    #[must_use]
    pub const fn group_name(self) -> &'static str {
        self.group
    }

    /// Returns the logical benchmark name.
    #[must_use]
    pub const fn benchmark_name(self) -> &'static str {
        self.benchmark
    }

    pub(crate) fn canonical_identity(self) -> String {
        join_identity(self.group, self.benchmark)
    }

    pub(crate) fn criterion_identity(self) -> String {
        self.canonical_identity()
    }

    pub(crate) fn gungraun_identity(self) -> String {
        join_identity(self.gungraun_group, self.gungraun_benchmark)
    }

    pub(crate) const fn allocation_tracking(self) -> bool {
        self.allocation_tracking
    }
}

fn join_identity(group: &str, benchmark: &str) -> String {
    format!("{group}/{benchmark}")
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::BenchmarkIdentity;

    fn assert_common_traits<T: Copy + Clone + Eq + Ord + std::hash::Hash + std::fmt::Debug + std::fmt::Display>() {}

    #[test]
    fn exposes_canonical_and_native_names() {
        assert_common_traits::<BenchmarkIdentity>();
        let identity = BenchmarkIdentity::__new(
            "basic/checksum",
            "rolling",
            "__metabench_group_ROLLING",
            "__metabench_benchmark_ROLLING",
            true,
        );

        assert_eq!(identity.to_string(), "basic/checksum/rolling");
        assert_eq!(identity.criterion_identity(), "basic/checksum/rolling");
        assert_eq!(
            identity.gungraun_identity(),
            "__metabench_group_ROLLING/__metabench_benchmark_ROLLING"
        );
    }
}
