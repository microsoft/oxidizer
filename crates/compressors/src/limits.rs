// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::{NonZeroU32, NonZeroU64};

use crate::error::{Error, Result};

/// Cumulative output below this size is never rejected by the ratio guard.
///
/// A container carries a fixed header and trailer, and a short stream's compressed form can easily
/// be larger than its payload. Without a floor, a legitimate two-byte stream would look like an
/// infinitely bad expansion ratio and be rejected. 32 KiB is far below any size at which a
/// decompression bomb becomes a memory-exhaustion risk.
#[cfg_attr(
    all(
        not(test),
        not(any(
            test,
            feature = "brotli",
            feature = "deflate",
            feature = "gzip",
            feature = "zlib",
            feature = "zstd"
        ))
    ),
    expect(dead_code, reason = "only the decompressors resolve and enforce bounds, and no format is enabled")
)]
const RATIO_FLOOR_BYTES: u64 = 32 * 1024;

/// The cap the buffering conveniences put on total decompressed output.
///
/// A ratio bound cannot tell a bomb from legitimate highly-compressible data, so an absolute cap is
/// what actually bounds untrusted input. It applies where the crate accumulates a whole result --
/// each format's `decompress` and `decompress_with_limits`, and the same pair on
/// [`Format`][crate::format::Format] -- because those are the paths where a bomb exhausts the caller's
/// memory. A decompressor driven incrementally hands every chunk straight back, so a cumulative
/// bound there would cut off long streams that never buffer more than one chunk.
///
/// 64 MiB is a policy guardrail for the common case, not a universal safety guarantee: a server
/// decompressing many bodies at once still has to bound its own concurrency. A caller who buffers
/// more, or less, passes explicit [`DecompressorLimits`] to `decompress_with_limits`.
pub(crate) const DEFAULT_MAX_OUTPUT_LEN: u64 = 64 * 1024 * 1024;

/// The cap the buffering conveniences put on concatenated stream count.
///
/// Each stream costs engine setup that its own payload need not pay for, so a buffered input of
/// many tiny members amplifies work out of proportion to its size. Like the output cap this binds
/// only where output accumulates: formats that treat concatenated members as one logical stream are
/// used incrementally for exactly the block-oriented archive workloads that run to many thousands
/// of members, and those must keep passing through.
///
/// The rule the number encodes: comfortably above any plausible count for a single buffered HTTP
/// body, and far enough below an archive's member count that the two cases stay distinguishable. It
/// is a policy guardrail chosen for that separation rather than a measured threshold.
pub(crate) const DEFAULT_MAX_STREAMS: u64 = 1024;

/// One configurable bound, in one of three states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Limit<T> {
    /// The caller expressed no opinion, so the format's own default applies.
    #[default]
    Unset,
    /// The caller explicitly removed the bound.
    Unlimited,
    /// The caller explicitly chose a bound.
    Value(T),
}

impl<T> Limit<T> {
    #[cfg_attr(
        all(
            not(test),
            not(any(
                test,
                feature = "brotli",
                feature = "deflate",
                feature = "gzip",
                feature = "zlib",
                feature = "zstd"
            ))
        ),
        expect(dead_code, reason = "only the decompressors resolve and enforce bounds, and no format is enabled")
    )]
    fn resolve(self, default: Option<T>) -> Option<T> {
        match self {
            Self::Unset => default,
            Self::Unlimited => None,
            Self::Value(value) => Some(value),
        }
    }
}

/// Bounds on how much data decompression may produce.
///
/// Compressed data can expand by orders of magnitude, so a decompressor pointed at untrusted input is a
/// memory-exhaustion vector.
///
/// This type carries *overrides*, not values. Each bound starts unset, meaning the format applies
/// its own default. The only bound a format sets is the ratio, because the formats differ by orders
/// of magnitude in what they can legitimately produce:
///
/// | Format | Default ratio bound | Why |
/// |---|---|---|
/// | `deflate`, `zlib`, `gzip` | `1100x` | deflate cannot expand further than about `1032x`; that is structural |
/// | `brotli` | none | brotli has no structural ceiling, so any ratio bound rejects sufficiently compressible legitimate data |
/// | `zstd` | `250 000x` | zstd has no structural ceiling either, so this is a very loose coarse backstop rather than a bound derived from the format |
///
/// Total output and stream count are not bounded by default, because a decompressor hands each
/// chunk straight back and a stream of any length passes through it in bounded memory. The
/// conveniences that buffer a whole result -- each format's `decompress` and
/// `decompress_with_limits`, and the same pair on [`Format`][crate::format::Format] -- add a 64 MiB output
/// cap and a 1024 stream cap to whichever of those bounds the caller left unset.
///
/// # Security
///
/// A ratio bound is a coarse backstop, not real protection: in a format with no structural
/// expansion ceiling it cannot separate a bomb from legitimate highly-compressible data. What
/// bounds untrusted input is an absolute cap on what you buffer. Set
/// [`max_output_len`][Self::max_output_len] to whatever the caller can afford whenever it
/// accumulates decompressed output itself.
///
/// # Examples
///
/// ```
/// use std::num::{NonZeroU32, NonZeroU64};
///
/// use compressors::DecompressorLimits;
///
/// // Tighten the shared 64 MiB cap to what this caller can actually buffer.
/// let untrusted =
///     DecompressorLimits::new().max_output_len(NonZeroU64::new(16 * 1024 * 1024).unwrap());
///
/// // Or override every bound.
/// let strict = DecompressorLimits::new()
///     .max_ratio(NonZeroU32::new(50).unwrap())
///     .max_output_len(NonZeroU64::new(1024 * 1024).unwrap())
///     .max_streams(NonZeroU64::new(16).unwrap());
/// # let _ = (untrusted, strict);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DecompressorLimits {
    ratio: Limit<u32>,
    output_len: Limit<u64>,
    streams: Limit<u64>,
}

impl DecompressorLimits {
    /// Overrides nothing: every bound is left to the format's own default.
    ///
    /// This is what [`Default`] returns.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            ratio: Limit::Unset,
            output_len: Limit::Unset,
            streams: Limit::Unset,
        }
    }

    /// Removes every bound, overriding whatever the format would have applied.
    ///
    /// # Security
    ///
    /// Only use this when the compressed data comes from a source you trust to the same degree you
    /// trust your own process. It removes the caps the buffering conveniences would otherwise apply,
    /// so a decompression bomb passed to one of those will consume memory until the allocator gives
    /// up. Driving a decompressor directly still hands back one bounded chunk at a time, and there
    /// the risk is only what the consumer chooses to keep.
    pub const UNLIMITED: Self = Self {
        ratio: Limit::Unlimited,
        output_len: Limit::Unlimited,
        streams: Limit::Unlimited,
    };

    /// Bounds the ratio of decompressed to compressed bytes.
    ///
    /// The ratio is only enforced once cumulative output exceeds 32 KiB, so small streams are never
    /// rejected for the fixed overhead of their container.
    #[must_use]
    pub const fn max_ratio(mut self, ratio: NonZeroU32) -> Self {
        self.ratio = Limit::Value(ratio.get());
        self
    }

    /// Removes the ratio bound, overriding the format's default.
    #[must_use]
    pub const fn unbounded_ratio(mut self) -> Self {
        self.ratio = Limit::Unlimited;
        self
    }

    /// Bounds the total decompressed size, in bytes.
    ///
    /// This is the bound that actually protects a caller which buffers the output. It takes a
    /// [`NonZeroU64`] for the same reason the ratio and stream bounds take non-zero types: a bound
    /// of zero rejects every stream, which is a way of not decompressing rather than a limit.
    #[must_use]
    pub const fn max_output_len(mut self, bytes: NonZeroU64) -> Self {
        self.output_len = Limit::Value(bytes.get());
        self
    }

    /// Removes the total size bound, overriding the format's default.
    #[must_use]
    pub const fn unbounded_output_len(mut self) -> Self {
        self.output_len = Limit::Unlimited;
        self
    }

    /// Bounds how many compressed streams may be decompressed from one input.
    ///
    /// One independently framed compressed stream costs one count. A gzip member is one such
    /// stream, so a file of concatenated members costs one per member even though multi-stream mode
    /// joins them into a single logical output; the same holds for concatenated zstd frames.
    ///
    /// This limits work that produces little or no output, such as a file containing millions of
    /// empty gzip members.
    #[must_use]
    pub const fn max_streams(mut self, streams: NonZeroU64) -> Self {
        self.streams = Limit::Value(streams.get());
        self
    }

    /// Removes the stream-count bound, overriding the format's default.
    #[must_use]
    pub const fn unbounded_streams(mut self) -> Self {
        self.streams = Limit::Unlimited;
        self
    }

    /// Adds the bounds an API that buffers a whole result needs, wherever the caller left them open.
    ///
    /// A decompressor that hands each chunk back keeps nothing, so it carries no cumulative bounds
    /// and a stream of any length passes through it. The conveniences that accumulate are a
    /// different proposition: what they produce is what the caller holds, so they apply the shared
    /// caps. Only bounds the caller left [`Limit::Unset`] are filled -- an explicit value, or an
    /// explicit removal, is the caller's decision and survives untouched.
    pub(crate) const fn for_buffered_output(mut self) -> Self {
        if matches!(self.output_len, Limit::Unset) {
            self.output_len = Limit::Value(DEFAULT_MAX_OUTPUT_LEN);
        }

        if matches!(self.streams, Limit::Unset) {
            self.streams = Limit::Value(DEFAULT_MAX_STREAMS);
        }

        self
    }

    /// Applies these overrides on top of a format's defaults.
    #[cfg_attr(
        all(
            not(test),
            not(any(
                test,
                feature = "brotli",
                feature = "deflate",
                feature = "gzip",
                feature = "zlib",
                feature = "zstd"
            ))
        ),
        expect(dead_code, reason = "only the decompressors resolve and enforce bounds, and no format is enabled")
    )]
    pub(crate) fn resolve(self, defaults: FormatLimits) -> FormatLimits {
        FormatLimits {
            ratio: self.ratio.resolve(defaults.ratio),
            output_len: self.output_len.resolve(defaults.output_len),
            streams: self.streams.resolve(defaults.streams),
        }
    }
}

/// A format's bounds after the caller's overrides have been applied.
///
/// Private: formats declare their defaults as constants of this type, and the decompressors enforce it.
#[cfg_attr(
    all(
        not(test),
        not(any(
            test,
            feature = "brotli",
            feature = "deflate",
            feature = "gzip",
            feature = "zlib",
            feature = "zstd"
        ))
    ),
    expect(dead_code, reason = "only the decompressors resolve and enforce bounds, and no format is enabled")
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FormatLimits {
    ratio: Option<u32>,
    output_len: Option<u64>,
    streams: Option<u64>,
}

#[cfg_attr(
    all(
        not(test),
        not(any(
            test,
            feature = "brotli",
            feature = "deflate",
            feature = "gzip",
            feature = "zlib",
            feature = "zstd"
        ))
    ),
    expect(dead_code, reason = "only the decompressors resolve and enforce bounds, and no format is enabled")
)]
impl FormatLimits {
    /// Declares a format's default bounds.
    pub(crate) const fn new(max_ratio: Option<u32>, max_output_len: Option<u64>, max_streams: Option<u64>) -> Self {
        Self {
            ratio: max_ratio,
            output_len: max_output_len,
            streams: max_streams,
        }
    }

    /// Fails if the totals so far violate either bound.
    pub(crate) fn check(self, input_len: u64, output_len: u64, streams: u64) -> Result<()> {
        if let Some(max) = self.output_len
            && output_len > max
        {
            return Err(Error::output_limit_exceeded(output_len, max));
        }

        if let Some(ratio) = self.ratio
            && output_len > RATIO_FLOOR_BYTES
            && output_len > input_len.saturating_mul(u64::from(ratio))
        {
            return Err(Error::ratio_limit_exceeded(input_len, output_len, ratio));
        }

        if let Some(max) = self.streams
            && streams > max
        {
            return Err(Error::stream_limit_exceeded(streams, max));
        }

        Ok(())
    }

    pub(crate) fn remaining_output(self, output_len: u64) -> Option<u64> {
        self.output_len.map(|maximum| maximum.saturating_sub(output_len))
    }

    #[cfg_attr(
        not(any(
            test,
            feature = "brotli",
            feature = "deflate",
            feature = "gzip",
            feature = "zlib",
            feature = "zstd"
        )),
        expect(dead_code, reason = "no decompression engine reads the stream limit when no format is enabled")
    )]
    pub(crate) fn max_streams(self) -> Option<u64> {
        self.streams
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_defaults_are_the_documented_values() {
        // Pinned as literals rather than by reference to the constants, so moving either one is a
        // deliberate edit here as well as there -- and so the doc table that quotes these numbers
        // cannot drift away from them unnoticed.
        assert_eq!(DEFAULT_MAX_OUTPUT_LEN, 64 * 1024 * 1024, "the shared output cap is 64 MiB");
        assert_eq!(DEFAULT_MAX_STREAMS, 1024, "the shared stream cap is 1024");
    }

    #[test]
    fn buffering_fills_only_the_bounds_the_caller_left_open() {
        // The trap this guards: a caller who overrides one bound must not silently lose the others.
        let ratio_only = DecompressorLimits::new().max_ratio(ratio(7)).for_buffered_output();

        assert_eq!(ratio_only.resolve(ALL_BOUNDS).output_len, Some(DEFAULT_MAX_OUTPUT_LEN));
        assert_eq!(ratio_only.resolve(ALL_BOUNDS).streams, Some(DEFAULT_MAX_STREAMS));
        assert_eq!(ratio_only.resolve(ALL_BOUNDS).ratio, Some(7), "the caller's own bound survives");
    }

    #[test]
    fn buffering_leaves_an_explicit_choice_alone() {
        let chosen = DecompressorLimits::new()
            .max_output_len(NonZeroU64::new(99).unwrap())
            .max_streams(NonZeroU64::new(3).unwrap())
            .for_buffered_output();

        assert_eq!(
            chosen.resolve(ALL_BOUNDS).output_len,
            Some(99),
            "an explicit cap is not overwritten"
        );
        assert_eq!(chosen.resolve(ALL_BOUNDS).streams, Some(3), "an explicit cap is not overwritten");
    }

    #[test]
    fn buffering_respects_an_explicit_removal() {
        let removed = DecompressorLimits::new()
            .unbounded_output_len()
            .unbounded_streams()
            .for_buffered_output();

        assert_eq!(removed.resolve(ALL_BOUNDS).output_len, None, "opting out is the caller's decision");
        assert_eq!(removed.resolve(ALL_BOUNDS).streams, None, "opting out is the caller's decision");
        assert_eq!(
            DecompressorLimits::UNLIMITED.for_buffered_output().resolve(ALL_BOUNDS),
            FormatLimits::new(None, None, None),
            "UNLIMITED removes every bound, buffering or not"
        );
    }

    /// Stands in for a format's declared defaults.
    const DEFAULTS: FormatLimits = FormatLimits::new(Some(1_000), None, None);

    /// Stands in for defaults that set every bound.
    ///
    /// Real formats declare only a ratio, so resolving against [`DEFAULTS`] cannot tell "the caller
    /// removed this bound" from "there was nothing to remove". Tests about removal use this instead.
    const ALL_BOUNDS: FormatLimits = FormatLimits::new(Some(1_000), Some(4_096), Some(8));

    fn ratio(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }

    fn resolved(limits: DecompressorLimits) -> FormatLimits {
        limits.resolve(DEFAULTS)
    }

    #[test]
    fn default_overrides_nothing() {
        assert_eq!(DecompressorLimits::default(), DecompressorLimits::new());
        assert_eq!(resolved(DecompressorLimits::default()), DEFAULTS);
    }

    #[test]
    fn an_unset_bound_defers_to_the_format() {
        // The whole point of the override model: a caller who cares about one bound must not
        // silently clobber the other with a value calibrated for a different format.
        let limits = DecompressorLimits::new().max_output_len(NonZeroU64::new(4096).unwrap());
        let resolved = resolved(limits);

        assert_eq!(resolved.ratio, DEFAULTS.ratio, "the format's ratio must survive");
        assert_eq!(resolved.output_len, Some(4096));
    }

    #[test]
    fn unlimited_removes_the_formats_defaults() {
        let resolved = DecompressorLimits::UNLIMITED.resolve(ALL_BOUNDS);

        assert_eq!(resolved.ratio, None);
        assert_eq!(resolved.output_len, None);
        assert_eq!(resolved.streams, None);
        resolved.check(1, u64::MAX, u64::MAX).unwrap();
    }

    #[test]
    fn each_bound_can_be_removed_independently() {
        let no_ratio = DecompressorLimits::new().unbounded_ratio().resolve(ALL_BOUNDS);
        assert_eq!(no_ratio.ratio, None);
        assert_eq!(no_ratio.output_len, ALL_BOUNDS.output_len, "the others are untouched");
        assert_eq!(no_ratio.streams, ALL_BOUNDS.streams, "the others are untouched");

        let no_len = DecompressorLimits::new().unbounded_output_len().resolve(ALL_BOUNDS);
        assert_eq!(no_len.ratio, ALL_BOUNDS.ratio, "the others are untouched");
        assert_eq!(no_len.output_len, None);
        assert_eq!(no_len.streams, ALL_BOUNDS.streams, "the others are untouched");

        let no_streams = DecompressorLimits::new().unbounded_streams().resolve(ALL_BOUNDS);
        assert_eq!(no_streams.ratio, ALL_BOUNDS.ratio, "the others are untouched");
        assert_eq!(no_streams.output_len, ALL_BOUNDS.output_len, "the others are untouched");
        assert_eq!(no_streams.streams, None);
    }

    #[test]
    fn an_explicit_bound_overrides_the_format() {
        let resolved = resolved(DecompressorLimits::new().max_ratio(ratio(7)));

        assert_eq!(resolved.ratio, Some(7));
    }

    #[test]
    fn ratio_guard_rejects_a_bomb() {
        let error = DEFAULTS.check(1_000, 100 * 1024 * 1024, 1).unwrap_err();

        assert!(error.is_limit_exceeded());
    }

    #[test]
    fn ratio_guard_allows_multi_gigabyte_streams() {
        // An absolute cap would reject this; a ratio guard must not.
        DEFAULTS.check(64 * 1024 * 1024 * 1024, 640 * 1024 * 1024 * 1024, 1).unwrap();
    }

    #[test]
    fn ratio_guard_ignores_output_below_the_floor() {
        DEFAULTS.check(0, RATIO_FLOOR_BYTES, 1).unwrap();
    }

    #[test]
    fn the_ratio_floor_is_exactly_32_kib() {
        // Pinned as a literal (not `32 * 1024`) so a mutated multiplication in the constant's
        // definition cannot hide behind a test that recomputes the same expression.
        assert_eq!(RATIO_FLOOR_BYTES, 32_768);
    }

    #[test]
    fn ratio_guard_engages_immediately_above_the_floor() {
        let error = DEFAULTS.check(0, RATIO_FLOOR_BYTES + 1, 1).unwrap_err();

        assert!(error.is_limit_exceeded());
    }

    #[test]
    fn absolute_bound_rejects_beyond_the_cap() {
        let limits = resolved(DecompressorLimits::new().max_output_len(NonZeroU64::new(100).unwrap()));
        let error = limits.check(1_000_000, 101, 1).unwrap_err();

        assert!(error.is_limit_exceeded());
    }

    #[test]
    fn absolute_bound_allows_exactly_the_cap() {
        let limits = resolved(DecompressorLimits::new().max_output_len(NonZeroU64::new(100).unwrap()));

        limits.check(1_000_000, 100, 1).unwrap();
    }

    #[test]
    fn ratio_multiplication_saturates_instead_of_overflowing() {
        let limits = resolved(DecompressorLimits::new().max_ratio(ratio(u32::MAX)));

        limits.check(u64::MAX, u64::MAX, 1).unwrap();
    }

    #[test]
    fn stream_count_is_bounded() {
        let limits = resolved(DecompressorLimits::new().max_streams(NonZeroU64::new(2).unwrap()));

        limits.check(100, 100, 2).unwrap();
        let error = limits.check(100, 100, 3).unwrap_err();

        assert!(error.is_limit_exceeded());
    }

    #[test]
    fn remaining_output_saturates_at_zero() {
        let limits = resolved(DecompressorLimits::new().max_output_len(NonZeroU64::new(100).unwrap()));

        assert_eq!(limits.remaining_output(40), Some(60));
        assert_eq!(limits.remaining_output(100), Some(0));
        assert_eq!(limits.remaining_output(101), Some(0));
        assert_eq!(DEFAULTS.remaining_output(100), None);
    }
}
