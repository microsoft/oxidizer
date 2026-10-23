// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A bounded state-machine campaign over the decompression surface.
//!
//! The deterministic suite covers carefully chosen cuts, chunk sizes and corruption offsets. What
//! it cannot cover is their product: a member boundary landing on a `BytesView` span boundary, on
//! output exhaustion, on an exact limit, or on trailing bytes, all at once. That space is
//! multiplicative, which is what this campaign is for.
//!
//! One target for every backend, deliberately. CI charges per target, and what is interesting here
//! is the interaction between this crate's framing and *some* engine rather than any one of them.
//!
//! Everything goes through the public API. The push/pull state machine is sealed, so this reaches
//! it the way callers do. That is also the surface an attacker reaches: the operation ordering the
//! sealed API would expose is chosen by trusted calling code, never by input bytes.
//!
//! # What is asserted
//!
//! That decompression is *safe*, not that it succeeds. Malformed input is expected to fail; what
//! must never happen is a panic, a non-terminating run, or a bound that quietly stops applying.

#![expect(
    clippy::expect_used,
    reason = "a fuzz target reports a violated property by failing loudly, which is the point"
)]

use std::num::{NonZeroU64, NonZeroUsize};

use bolero::TypeGenerator;
use bytesbuf::BytesView;
use compressors::format::Format;
use compressors::{CompressorBuilder, DecompressorBuilder, DecompressorLimits, Level, Resources, TrailingData};

/// Bounds the generated payload, so malformed cases stay cheap enough to run many of them.
const MAX_PAYLOAD: usize = 4096;

/// Bounds how many copies a concatenating mutation makes.
const MAX_COPIES: usize = 4;

/// A hard ceiling applied to every run regardless of what the scenario asked for.
///
/// Without it a generated input that happens to be a small expansion bomb would spend the whole
/// campaign budget producing megabytes, and report nothing. The scenario's own bound is applied on
/// top of this, never instead of it.
const HARD_OUTPUT_CEILING: u64 = 1 << 20;

/// What to do to a valid stream before decompressing it.
#[derive(Debug, Clone, Copy, TypeGenerator)]
enum Mutation {
    /// Leave it alone. This is the round-trip case, and the only one with a required output.
    None,
    /// Cut it short, so the decoder meets end of input mid-stream.
    Truncate(u16),
    /// Flip one bit, so a checksum or a structural field disagrees with the data.
    BitFlip(u16),
    /// Append bytes, which is trailing data or another member depending on policy.
    Append(u8),
    /// Repeat it, which is a concatenated stream to the formats that decode them.
    Concatenate(u8),
}

/// Where to place a bound relative to the value it bounds.
///
/// The boundary is where an off-by-one lives, so two of these sit exactly on it.
#[derive(Debug, Clone, Copy, TypeGenerator)]
enum LimitChoice {
    /// Leave it to the format, and to the buffering fallback.
    Unset,
    /// Exactly the value being bounded, which must be accepted.
    Exact,
    /// One below, which must be refused.
    JustUnder,
    /// Comfortably above.
    Generous,
}

impl LimitChoice {
    /// Resolves to a concrete bound around `actual`.
    fn resolve(self, actual: u64) -> Option<NonZeroU64> {
        let value = match self {
            Self::Unset => return None,
            Self::Exact => actual,
            Self::JustUnder => actual.saturating_sub(1),
            Self::Generous => actual.saturating_mul(4).max(1024),
        };

        NonZeroU64::new(value.max(1))
    }
}

/// One generated decompression.
#[derive(Debug, Clone, TypeGenerator)]
struct Scenario {
    /// Selects a compiled format, reduced modulo the number available.
    format: u8,
    /// Compressed by this crate first when `valid`, otherwise fed in raw.
    payload: Vec<u8>,
    /// Whether `payload` is compressed before being decompressed, or used as it is.
    valid: bool,
    /// The level used when compressing, reduced into range.
    level: u8,
    /// Applied to the compressed bytes before decompression.
    mutation: Mutation,
    /// Splits the input across this many spans, so span and member boundaries interact.
    fragments: u8,
    /// The decompressor's output chunk size, reduced into range.
    output_chunk: u16,
    /// Selects an output bound relative to the payload's real size.
    output_limit: LimitChoice,
    /// Whether concatenated members are decoded.
    multi_stream: bool,
    /// Whether bytes after a complete stream are an error.
    reject_trailing: bool,
}

#[test]
fn decompression_survives_arbitrary_input() {
    bolero::check!().with_type::<Scenario>().for_each(run);
}

fn run(scenario: &Scenario) {
    let Some(&format) = Format::ALL.get(usize::from(scenario.format) % Format::ALL.len().max(1)) else {
        return;
    };

    let resources = Resources::default();
    let mut payload = scenario.payload.clone();
    payload.truncate(MAX_PAYLOAD);

    let Some(encoded) = encode(scenario, format, &payload, &resources) else {
        return;
    };

    let mutated = mutate(scenario.mutation, &encoded);

    // An unmutated valid stream is the only case with a knowable answer, and only when nothing
    // bounds it below its own size. Everything else is allowed to fail; what is checked there is
    // that it fails rather than panics or runs away.
    let pristine = scenario.valid && matches!(scenario.mutation, Mutation::None);
    let natural = payload.len() as u64;

    let input = fragmented(&mutated, scenario.fragments, &resources);
    let outcome = decompress(scenario, format, input, natural, &resources);

    match (pristine, scenario.output_limit) {
        (true, LimitChoice::Exact | LimitChoice::Generous | LimitChoice::Unset) => {
            let produced = outcome.expect("an unmutated stream within its bounds must decode");
            assert_eq!(produced, payload, "a round trip must return what went in");
        }
        // `JustUnder` can only be genuinely under when there are at least two bytes to be under:
        // a bound must be non-zero, so one less than a one-byte output is still one byte.
        (true, LimitChoice::JustUnder) if payload.len() >= 2 => {
            let error = outcome.expect_err("a bound below the real output must be refused");
            assert!(error.is_limit_exceeded(), "expected a limit error, got {error}");
        }
        _ => {
            // Malformed, mutated or degenerate: any outcome is legitimate, and reaching here at all
            // is the property -- no panic, and no run that never returned.
            drop(outcome);
        }
    }
}

/// Produces the bytes to decompress: either a real stream, or the payload used as one.
fn encode(scenario: &Scenario, format: Format, payload: &[u8], resources: &Resources) -> Option<Vec<u8>> {
    if !scenario.valid {
        return Some(payload.to_vec());
    }

    let level = Level::new(scenario.level % (Level::MAX.get() + 1))?;
    let compressor = CompressorBuilder::new().level(level).build_format(format, resources).ok()?;

    let view = BytesView::copied_from_slice(payload, resources.memory());

    compressors::compress(view, compressor).ok().map(|encoded| encoded.to_vec())
}

/// Applies the generated corruption.
fn mutate(mutation: Mutation, encoded: &[u8]) -> Vec<u8> {
    let mut bytes = encoded.to_vec();

    match mutation {
        Mutation::None => {}
        Mutation::Append(byte) => bytes.push(byte),
        Mutation::Concatenate(copies) => {
            let original = bytes.clone();
            for _ in 0..(usize::from(copies) % MAX_COPIES) {
                bytes.extend_from_slice(&original);
            }
        }
        // The remaining two index into the stream, so an empty one is left as it is.
        Mutation::Truncate(at) => {
            if let Some(len) = NonZeroUsize::new(bytes.len()) {
                bytes.truncate(usize::from(at) % len.get());
            }
        }
        Mutation::BitFlip(at) => {
            if let Some(len) = NonZeroUsize::new(bytes.len()) {
                let index = usize::from(at) % len.get();
                bytes[index] ^= 1 << (at % 8);
            }
        }
    }

    bytes
}

/// Splits `bytes` across spans, so span boundaries and member boundaries can coincide.
fn fragmented(bytes: &[u8], fragments: u8, resources: &Resources) -> BytesView {
    let segments = usize::from(fragments) % 16 + 1;
    let size = bytes.len().div_ceil(segments).max(1);

    BytesView::from_views(
        bytes
            .chunks(size)
            .map(|chunk| BytesView::copied_from_slice(chunk, resources.memory())),
    )
}

/// Drives one decompression through the collecting convenience.
fn decompress(scenario: &Scenario, format: Format, input: BytesView, natural: u64, resources: &Resources) -> compressors::Result<Vec<u8>> {
    // The scenario's bound narrows the hard ceiling; it never widens it.
    let ceiling = scenario
        .output_limit
        .resolve(natural)
        .map_or(HARD_OUTPUT_CEILING, |chosen| chosen.get().min(HARD_OUTPUT_CEILING));

    let limits = DecompressorLimits::new().max_output_len(NonZeroU64::new(ceiling.max(1)).expect("clamped to at least one"));

    let trailing = if scenario.reject_trailing {
        TrailingData::Reject
    } else {
        TrailingData::Ignore
    };

    let chunk = NonZeroUsize::new(usize::from(scenario.output_chunk) % 4096 + 1).expect("at least one");

    let decompressor = DecompressorBuilder::new()
        .limits(limits)
        .multi_stream(scenario.multi_stream)
        .trailing_data(trailing)
        .output_chunk_size(chunk)
        .build_format(format, resources)
        .map_err(|error| compressors::Error::other("the decompressor could not be built", error))?;

    compressors::decompress(input, decompressor).map(|view| view.to_vec())
}
