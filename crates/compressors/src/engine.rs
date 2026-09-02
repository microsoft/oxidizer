// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::mem::MaybeUninit;
use std::num::NonZeroUsize;

use bytesbuf::mem::{MemoryShared, OpaqueMemory};
use bytesbuf::{BytesBuf, BytesView};

use crate::error::{Error, Result};
use crate::output::Output;

/// How much output a single `pull` produces before handing control back.
///
/// This bounds the codec's working set: a caller streaming hundreds of gigabytes never holds more
/// than one pending input view plus one chunk of output.
pub(crate) const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;

/// Maximum input consumed by one public `pull` call.
const MAX_INPUT_PER_PULL: usize = 1024 * 1024;

/// Maximum engine calls made by one public `pull` call.
const MAX_STEPS_PER_PULL: usize = 64;

/// Enough room for the largest deflate sync-flush marker plus one spare byte.
const MIN_FLUSH_OUTPUT: usize = 7;

/// What the encoder should do with the input supplied to one engine step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    Process,
    Flush,
    Finish,
}

/// The outcome of a single engine step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// The engine can do more work, given more input or more output space.
    Continue,
    /// A requested resumable flush completed.
    FlushComplete,
    /// The engine reached the end of a compressed stream.
    StreamEnd,
}

/// What a decoder wants to do after one compressed stream ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamEnd {
    /// The logical input is complete; preserve any unconsumed bytes as a remainder.
    Complete,
    /// The stream is complete only when the caller confirms EOF.
    AwaitEof,
    /// Reset the codec and accept another compressed stream.
    NextStream,
}

/// One direction of a compression algorithm, as the [`Pump`] drives it.
pub(crate) trait Codec {
    /// Runs a single engine step.
    ///
    /// Returns the step outcome, the number of input bytes consumed, and the number of output
    /// bytes written to the front of `output`.
    ///
    /// `operation` is only `Flush` or `Finish` on the final slice of the currently pending input.
    /// A [`BytesView`] is a chain of segments, so signaling either operation on an earlier segment
    /// would flush or finalize at the wrong boundary.
    fn step(&mut self, input: &[u8], output: &mut [MaybeUninit<u8>], operation: Operation) -> Result<(Step, usize, usize)>;

    /// Called when [`Codec::step`] reported [`Step::StreamEnd`].
    ///
    /// Defaults to a complete, single-stream container; codecs whose containers can continue
    /// (trailing data, concatenated streams, strict EOF checks) override this.
    fn stream_ended(&mut self) -> Result<StreamEnd> {
        Ok(StreamEnd::Complete)
    }

    /// Validates the cumulative byte counts, for codecs that enforce limits.
    // Equivalent mutant: the body already reduces to `Ok(())` for every input; `mutants::skip`
    // documents that no test can distinguish this default from a mutant that also always
    // returns `Ok(())`, since the two are behaviorally identical (the only difference is
    // dropping the parameter-discarding `let` binding, which has no runtime effect).
    #[cfg_attr(test, mutants::skip)]
    fn check_limits(&self, total_in: u64, total_out: u64, streams: u64) -> Result<()> {
        let _ = (total_in, total_out, streams);
        Ok(())
    }

    /// Returns the remaining absolute output budget, if one is configured.
    fn remaining_output(&self, _total_out: u64) -> Option<u64> {
        None
    }

    /// Returns the maximum number of streams this codec may decode.
    fn max_streams(&self) -> Option<u64> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Accepting input.
    Open,
    /// Draining a resumable flush. `end_after` queues finalization behind it.
    Flushing { end_after: bool },
    /// The caller signaled end of input; drain the engine.
    Finishing,
    /// A compressed stream ended and the decoder is waiting for another one or EOF.
    BetweenStreams,
    /// A single stream ended and strict trailing-data validation is waiting for EOF.
    AwaitingEof,
    /// The configured stream-count limit was reached; only EOF is now valid.
    AtStreamLimit { maximum: u64 },
    /// The engine reported end of stream.
    Done,
    /// A fatal codec error occurred. Native state must never be entered again.
    Failed,
}

/// What a step loop iteration should do next, after a stream end updates [`State`].
///
/// Computed alongside the new `State` in the same match so the loop never needs a second,
/// redundant match over `State` (which would otherwise need an unreachable catch-all arm).
enum StreamContinuation {
    /// Keep pumping without returning to the caller.
    Loop,
    /// Report [`Output::NeedInput`] to the caller.
    NeedInput,
    /// Report [`Output::Done`] to the caller.
    Done,
}

/// Moves bytes between a [`BytesView`] source and a [`BytesBuf`] sink through a [`Codec`].
///
/// This is where the impedance match happens: `BytesView` is a chain of segments with no
/// contiguous representation, and `BytesBuf` exposes its spare capacity one uninitialized segment
/// at a time. Both are fed to the engine a segment at a time, so no intermediate copy is needed
/// and no `std::io` trait is involved.
#[derive(Debug)]
pub(crate) struct Pump {
    memory: OpaqueMemory,
    chunk_size: usize,
    input: BytesView,
    output: BytesBuf,
    total_in: u64,
    total_out: u64,
    streams: u64,
    state: State,
    done_reported: bool,
}

impl Pump {
    pub(crate) fn new(memory: impl MemoryShared, chunk_size: NonZeroUsize) -> Self {
        let memory = OpaqueMemory::new(memory);
        let output = memory.reserve(chunk_size.get());

        Self {
            memory,
            chunk_size: chunk_size.get(),
            input: BytesView::new(),
            output,
            total_in: 0,
            total_out: 0,
            streams: 0,
            state: State::Open,
            done_reported: false,
        }
    }

    pub(crate) fn push(&mut self, input: BytesView) -> Result<()> {
        if !self.input.is_empty() {
            return Err(Error::invalid_state(
                "cannot push more input while previously pushed input is still pending",
            ));
        }

        match self.state {
            State::Open => {}
            State::BetweenStreams | State::AwaitingEof | State::AtStreamLimit { .. } if input.is_empty() => return Ok(()),
            State::BetweenStreams => self.state = State::Open,
            State::AwaitingEof => {
                let error = Error::corrupt_data("trailing data followed the compressed stream");
                return Err(self.fail(error));
            }
            State::AtStreamLimit { maximum } => {
                let error = Error::stream_limit_exceeded(self.streams.saturating_add(1), maximum);
                return Err(self.fail(error));
            }
            State::Flushing { .. } => {
                return Err(Error::invalid_state("cannot push more input while a flush is still pending"));
            }
            State::Finishing | State::Done => {
                return Err(Error::invalid_state("cannot push more input after end of input was signaled"));
            }
            State::Failed => {
                return Err(Error::invalid_state("cannot push more input after the codec failed"));
            }
        }

        self.input = input;
        Ok(())
    }

    pub(crate) fn flush(&mut self) -> Result<()> {
        match self.state {
            State::Open => self.state = State::Flushing { end_after: false },
            State::Flushing { end_after: false } => {}
            State::Flushing { end_after: true }
            | State::Finishing
            | State::BetweenStreams
            | State::AwaitingEof
            | State::AtStreamLimit { .. }
            | State::Done => {
                return Err(Error::invalid_state("cannot flush after end of input was signaled"));
            }
            State::Failed => {
                return Err(Error::invalid_state("cannot flush after the codec failed"));
            }
        }

        Ok(())
    }

    pub(crate) fn end_input(&mut self) {
        self.state = match self.state {
            State::Open => State::Finishing,
            State::Flushing { .. } => State::Flushing { end_after: true },
            State::BetweenStreams | State::AwaitingEof | State::AtStreamLimit { .. } => State::Done,
            State::Finishing | State::Done => self.state,
            State::Failed => State::Failed,
        };
    }

    pub(crate) fn total_in(&self) -> u64 {
        self.total_in
    }

    pub(crate) fn total_out(&self) -> u64 {
        self.total_out
    }

    pub(crate) fn take_remainder(&mut self) -> Result<BytesView> {
        if self.state != State::Done || !self.done_reported {
            return Err(Error::invalid_state("the input remainder is available only after decoding is done"));
        }

        Ok(std::mem::replace(&mut self.input, BytesView::new()))
    }

    fn fail(&mut self, error: Error) -> Error {
        self.state = State::Failed;
        error
    }

    /// Hands over whatever output has accumulated, if any.
    fn take_output(&mut self) -> Option<BytesView> {
        if self.output.is_empty() {
            return None;
        }

        Some(self.output.consume(self.output.len().min(self.chunk_size)))
    }

    /// Ensures the output buffer has at least `additional_bytes` of spare capacity.
    // Equivalent mutant: `BytesBuf::reserve` already re-derives `additional_bytes <=
    // remaining_capacity()` internally and returns immediately once that holds, so calling it
    // one comparison earlier (right at the `remaining_capacity() == additional_bytes` boundary)
    // is indistinguishable from skipping the call: either way `reserve` does nothing. Takes the
    // output buffer and memory provider by reference, rather than `&mut self`, so it borrows
    // disjointly from `self.input` at the call site.
    #[cfg_attr(test, mutants::skip)]
    fn ensure_output_capacity(output: &mut BytesBuf, memory: &OpaqueMemory, additional_bytes: usize) {
        if output.remaining_capacity() < additional_bytes {
            output.reserve(additional_bytes, memory);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "keeping the state transitions in one loop makes their ordering and terminal paths explicit"
    )]
    pub(crate) fn pull(&mut self, codec: &mut impl Codec) -> Result<Output> {
        match self.state {
            State::Done => {
                if let Some(data) = self.take_output() {
                    return Ok(Output::Data(data));
                }

                self.done_reported = true;
                return Ok(Output::Done);
            }
            State::Failed => {
                return Err(Error::invalid_state("cannot continue after a previous codec failure"));
            }
            State::BetweenStreams | State::AwaitingEof | State::AtStreamLimit { .. } if self.input.is_empty() => {
                return Ok(self.take_output().map_or(Output::NeedInput, Output::Data));
            }
            _ => {}
        }

        let mut steps = 0;
        let mut input_work = 0;

        loop {
            // Hand over a full chunk rather than growing the buffer, so the working set stays
            // bounded no matter how long the stream is.
            if self.output.len() >= self.chunk_size
                && let Some(data) = self.take_output()
            {
                return Ok(Output::Data(data));
            }

            if steps >= MAX_STEPS_PER_PULL || input_work >= MAX_INPUT_PER_PULL {
                return Ok(self.take_output().map_or(Output::Progress, Output::Data));
            }

            // A memory provider may hand back more capacity than asked for, so the chunk bound has
            // to be applied to the slice itself rather than to the reservation. This also bounds
            // the cost of the engine's zero-fill of the uninitialized output slice.
            let budget = self.chunk_size - self.output.len();
            let pending = self.input.len();
            let input_budget = MAX_INPUT_PER_PULL - input_work;
            let (step, consumed, produced, supplied, provided_output) = {
                let first = self.input.first_slice();
                let input = &first[..first.len().min(input_budget)];
                let supplied = input.len();
                let last_slice = input.len() == pending;
                let operation = match self.state {
                    State::Flushing { .. } if last_slice => Operation::Flush,
                    State::Finishing if last_slice => Operation::Finish,
                    State::Open | State::Flushing { .. } | State::Finishing => Operation::Process,
                    State::BetweenStreams | State::AwaitingEof | State::AtStreamLimit { .. } | State::Done | State::Failed => {
                        non_driving_state_reached_the_step_loop()
                    }
                };
                let engine_budget = if operation == Operation::Flush {
                    budget.max(MIN_FLUSH_OUTPUT)
                } else {
                    budget
                };
                Self::ensure_output_capacity(&mut self.output, &self.memory, engine_budget);
                let spare = self.output.first_unfilled_slice();
                let remaining = codec.remaining_output(self.total_out);
                let limit_budget = remaining.map_or(usize::MAX, |remaining| usize::try_from(remaining).unwrap_or(usize::MAX));
                // One probe byte lets the engine prove that a stream ending exactly at the limit
                // needs no more output, while bounding any overshoot to a byte that is never
                // returned to the caller.
                let take = spare.len().min(engine_budget).min(limit_budget.max(1));
                match codec.step(input, &mut spare[..take], operation) {
                    Ok((step, consumed, produced)) => (step, consumed, produced, supplied, take),
                    Err(error) => return Err(self.fail(error)),
                }
            };

            if consumed > supplied || produced > provided_output {
                return Err(self.fail(Error::invalid_state("the compression engine reported invalid byte counts")));
            }

            self.input.advance(consumed);

            // SAFETY: the engine reported writing `produced` bytes to the front of the slice
            // returned by `first_unfilled_slice`, so exactly that many bytes are initialized.
            unsafe { self.output.advance(produced) };

            self.total_in = self.total_in.saturating_add(u64::try_from(consumed).unwrap_or(u64::MAX));
            self.total_out = self.total_out.saturating_add(u64::try_from(produced).unwrap_or(u64::MAX));
            input_work = input_work.saturating_add(consumed);
            steps += 1;

            if let Err(error) = codec.check_limits(self.total_in, self.total_out, self.streams) {
                return Err(self.fail(error));
            }

            if step == Step::FlushComplete {
                self.state = match self.state {
                    State::Flushing { end_after: true } => State::Finishing,
                    State::Flushing { end_after: false } => State::Open,
                    _ => {
                        return Err(self.fail(Error::invalid_state("the compression engine completed an unrequested flush")));
                    }
                };

                if let Some(data) = self.take_output() {
                    return Ok(Output::Data(data));
                }

                if self.state == State::Open {
                    return Ok(Output::NeedInput);
                }

                continue;
            }

            if step == Step::StreamEnd {
                self.streams = self.streams.saturating_add(1);
                if let Err(error) = codec.check_limits(self.total_in, self.total_out, self.streams) {
                    return Err(self.fail(error));
                }

                let end_of_input = self.state == State::Finishing;
                let stream_end = match codec.stream_ended() {
                    Ok(stream_end) => stream_end,
                    Err(error) => return Err(self.fail(error)),
                };

                // Paired with the state so the match below stays exhaustive over exactly the
                // states this match can actually produce, with no catch-all for a state this
                // engine step can never reach.
                let (new_state, continuation) = match stream_end {
                    StreamEnd::Complete => (State::Done, StreamContinuation::Done),
                    StreamEnd::AwaitEof if !self.input.is_empty() => {
                        return Err(self.fail(Error::corrupt_data("trailing data followed the compressed stream")));
                    }
                    StreamEnd::AwaitEof if end_of_input => (State::Done, StreamContinuation::Done),
                    StreamEnd::AwaitEof => (State::AwaitingEof, StreamContinuation::NeedInput),
                    StreamEnd::NextStream
                        if codec.max_streams().is_some_and(|maximum| self.streams >= maximum) && !self.input.is_empty() =>
                    {
                        let maximum = codec.max_streams().unwrap_or(u64::MAX);
                        return Err(self.fail(Error::stream_limit_exceeded(self.streams.saturating_add(1), maximum)));
                    }
                    StreamEnd::NextStream if codec.max_streams().is_some_and(|maximum| self.streams >= maximum) && end_of_input => {
                        (State::Done, StreamContinuation::Done)
                    }
                    StreamEnd::NextStream if let Some(maximum) = codec.max_streams().filter(|maximum| self.streams >= *maximum) => {
                        (State::AtStreamLimit { maximum }, StreamContinuation::NeedInput)
                    }
                    StreamEnd::NextStream if !self.input.is_empty() && end_of_input => (State::Finishing, StreamContinuation::Loop),
                    StreamEnd::NextStream if !self.input.is_empty() => (State::Open, StreamContinuation::Loop),
                    StreamEnd::NextStream if end_of_input => (State::Done, StreamContinuation::Done),
                    StreamEnd::NextStream => (State::BetweenStreams, StreamContinuation::NeedInput),
                };

                self.state = new_state;

                if let Some(data) = self.take_output() {
                    return Ok(Output::Data(data));
                }

                return Ok(match continuation {
                    StreamContinuation::Done => {
                        self.done_reported = true;
                        Output::Done
                    }
                    StreamContinuation::Loop => continue,
                    StreamContinuation::NeedInput => Output::NeedInput,
                });
            }

            if consumed == 0 && produced == 0 {
                if self.state == State::Finishing {
                    let error = if self.streams == 0 {
                        Error::unexpected_end_of_stream()
                    } else {
                        Error::corrupt_data("trailing data did not form a complete compressed stream")
                    };
                    return Err(self.fail(error));
                }

                if self.input.is_empty() && self.state == State::Open {
                    return Ok(self.take_output().map_or(Output::NeedInput, Output::Data));
                }

                return Err(self.fail(Error::invalid_state("the compression engine could not make progress")));
            }
        }
    }
}

/// `Pump::pull` returns before entering the step loop for every state that does not drive the
/// codec (`Done`, `Failed`, and, whenever no input is pending, `BetweenStreams`, `AwaitingEof`,
/// and `AtStreamLimit`); `push` never leaves input pending for the latter three otherwise. So the
/// step loop can only ever run in `Open`, `Flushing`, or `Finishing` when driven through the
/// public API. Exercised directly by a white-box test that violates the invariant through private
/// field access, so no coverage exclusion is needed here.
#[cold]
fn non_driving_state_reached_the_step_loop() -> ! {
    unreachable!("non-driving states return before stepping")
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use bytesbuf::mem::GlobalPool;
    use bytesbuf::mem::testing::FixedBlockMemory;

    use super::*;

    /// A codec that copies input to output verbatim, so pump behaviour can be tested on its own.
    #[derive(Debug, Default)]
    struct Passthrough {
        ended: bool,
    }

    impl Codec for Passthrough {
        fn step(&mut self, input: &[u8], output: &mut [MaybeUninit<u8>], operation: Operation) -> Result<(Step, usize, usize)> {
            let count = input.len().min(output.len());
            for (slot, byte) in output.iter_mut().zip(input.iter().take(count)) {
                slot.write(*byte);
            }

            if operation == Operation::Finish && count == input.len() {
                self.ended = true;
                return Ok((Step::StreamEnd, count, count));
            }

            if operation == Operation::Flush && count == input.len() {
                return Ok((Step::FlushComplete, count, count));
            }

            Ok((Step::Continue, count, count))
        }
    }

    fn chunk(size: usize) -> NonZeroUsize {
        NonZeroUsize::new(size).expect("test chunk sizes are never zero")
    }

    fn view(bytes: &[u8]) -> BytesView {
        BytesView::copied_from_slice(bytes, &GlobalPool::new())
    }

    #[test]
    fn reports_need_input_when_empty() {
        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        let output = pump.pull(&mut Passthrough::default()).expect("pull succeeds");

        assert!(output.is_need_input());
    }

    #[test]
    fn round_trips_data_through_the_codec() {
        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(b"hello world")).expect("push succeeds");

        let data = pump
            .pull(&mut Passthrough::default())
            .expect("pull succeeds")
            .into_data()
            .expect("data is available");

        assert_eq!(data.to_vec(), b"hello world".to_vec());
        assert_eq!(pump.total_in(), 11);
        assert_eq!(pump.total_out(), 11);
    }

    #[test]
    fn bounds_each_chunk_to_the_configured_size() {
        let mut pump = Pump::new(GlobalPool::new(), chunk(4));
        pump.push(view(b"abcdefghij")).expect("push succeeds");

        let data = pump
            .pull(&mut Passthrough::default())
            .expect("pull succeeds")
            .into_data()
            .expect("data is available");

        assert!(data.len() <= 8, "chunk was {} bytes, expected it near 4", data.len());
    }

    #[test]
    fn a_single_step_never_gets_more_room_than_the_chunk_has_left() {
        // Writes only 2 bytes on the first call, regardless of how much output room is offered,
        // then greedily fills the entire offered slice on every later call. A budget computed as
        // `chunk_size + output.len()` (instead of `chunk_size - output.len()`) would hand this
        // codec more than the chunk has left on the second call, overrunning `chunk_size`.
        #[derive(Debug, Default)]
        struct PartialThenGreedy {
            calls: u32,
        }

        impl Codec for PartialThenGreedy {
            fn step(&mut self, input: &[u8], output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                self.calls += 1;
                let produced = if self.calls == 1 { output.len().min(2) } else { output.len() };
                for byte in &mut output[..produced] {
                    byte.write(0);
                }
                Ok((Step::Continue, 1.min(input.len()), produced))
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(10));
        pump.push(view(&[0_u8; 10])).expect("push succeeds");

        let data = pump
            .pull(&mut PartialThenGreedy::default())
            .expect("pull succeeds")
            .into_data()
            .expect("data is available");

        // `take_output` itself caps a returned chunk at `chunk_size`, so a step that was handed
        // too much room would not show up in `data.len()`; it shows up as extra bytes recorded in
        // `total_out` that this pull was never entitled to produce.
        assert!(data.len() <= 10, "a returned chunk must never exceed the configured chunk size");
        assert_eq!(
            pump.total_out(),
            10,
            "a step must never be handed more room than the chunk still has left"
        );
    }

    #[test]
    fn a_single_pull_never_takes_more_engine_steps_than_its_cap_allows() {
        // Echoes 1 byte of input to 1 byte of output per call, so input and output grow at
        // identical, easily predictable rates. With a chunk size larger than
        // `MAX_STEPS_PER_PULL`, the step count is the only thing that can end this pull before
        // the output buffer fills, so a mutant that stops incrementing `steps` would let this
        // single pull keep stepping until the chunk fills instead of stopping at the step cap.
        #[derive(Debug, Default)]
        struct OneByteEcho;

        impl Codec for OneByteEcho {
            fn step(&mut self, input: &[u8], output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                let consumed = input.len().min(1);
                if consumed == 1 {
                    output[0].write(input[0]);
                }
                Ok((Step::Continue, consumed, consumed))
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(100));
        pump.push(view(&[0_u8; 150])).expect("push succeeds");

        let output = pump.pull(&mut OneByteEcho).expect("pull succeeds");
        assert!(output.is_data());
        assert_eq!(
            pump.total_in(),
            64,
            "a single pull must stop after MAX_STEPS_PER_PULL engine steps, not fill the whole chunk"
        );
    }

    #[test]
    fn bounds_input_work_and_reports_progress() {
        #[derive(Debug)]
        struct SilentConsumer;

        impl Codec for SilentConsumer {
            fn step(&mut self, input: &[u8], _output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                Ok((Step::Continue, input.len(), 0))
            }
        }

        // Hardcoded literals (rather than `MAX_INPUT_PER_PULL`) so this test actually pins the
        // budget's numeric value instead of trivially matching whatever the constant is set to.
        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(&vec![0_u8; 1_048_577])).expect("push succeeds");

        assert!(pump.pull(&mut SilentConsumer).expect("pull succeeds").is_progress());
        assert_eq!(
            pump.total_in(),
            1_048_576,
            "a single pull should stop at exactly one mebibyte of input"
        );
    }

    #[test]
    fn a_single_step_never_gets_more_input_than_the_pull_has_left() {
        // Consumes only 3 bytes on the first call, regardless of how much input is offered, then
        // greedily consumes everything offered on every later call. The pushed input is backed
        // by a single 2,000,000-byte memory block, so `first_slice()` always returns the entire
        // remaining input as one contiguous slice: the per-pull input budget, not a memory-block
        // boundary, is the only thing that can ever limit how much a later call is offered. A
        // budget computed as `MAX_INPUT_PER_PULL + input_work` (instead of
        // `MAX_INPUT_PER_PULL - input_work`) would let this codec consume more than one mebibyte
        // in a single pull.
        #[derive(Debug, Default)]
        struct SmallFirstThenGreedy {
            calls: u32,
        }

        impl Codec for SmallFirstThenGreedy {
            fn step(&mut self, input: &[u8], _output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                self.calls += 1;
                let consumed = if self.calls == 1 { input.len().min(3) } else { input.len() };
                Ok((Step::Continue, consumed, 0))
            }
        }

        let large_block = NonZeroU32::new(2_000_000).expect("test block size is never zero");
        let single_block_memory = FixedBlockMemory::new(large_block);

        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(BytesView::copied_from_slice(&vec![0_u8; 1_048_578], &single_block_memory))
            .expect("push succeeds");

        assert!(
            pump.pull(&mut SmallFirstThenGreedy::default())
                .expect("pull succeeds")
                .is_progress()
        );
        assert_eq!(
            pump.total_in(),
            1_048_576,
            "a single pull must not consume more than one mebibyte of input"
        );
    }

    #[test]
    fn flush_returns_to_the_open_state() {
        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(b"flush me")).expect("push succeeds");
        pump.flush().expect("flush request succeeds");

        let mut codec = Passthrough::default();
        assert_eq!(
            pump.pull(&mut codec)
                .expect("pull succeeds")
                .into_data()
                .expect("flushed data")
                .to_vec(),
            b"flush me".to_vec()
        );
        assert!(pump.pull(&mut codec).expect("pull succeeds").is_need_input());
        pump.push(view(b"more")).expect("input is accepted after the flush");
    }

    #[test]
    fn flush_is_idempotent_while_still_pending() {
        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.flush().expect("flush request succeeds");
        pump.flush()
            .expect("a repeated flush request while one is already pending is a no-op");

        assert!(pump.pull(&mut Passthrough::default()).expect("pull succeeds").is_need_input());
    }

    #[test]
    fn empty_flush_completes_without_output() {
        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.flush().expect("flush request succeeds");

        assert!(
            pump.pull(&mut Passthrough::default())
                .expect("empty flush succeeds")
                .is_need_input()
        );
        pump.flush().expect("a completed flush can be requested again");
    }

    #[test]
    fn flush_with_queued_end_input_continues_to_finishing() {
        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.flush().expect("flush request succeeds");
        pump.end_input();

        assert!(
            pump.pull(&mut Passthrough::default())
                .expect("the queued end of input drains straight through")
                .is_done()
        );
    }

    #[test]
    fn drains_leftover_output_across_multiple_pulls_after_done() {
        /// Reports `StreamEnd` on a `Flush` step and writes as much output as the flush's minimum
        /// output budget allows, so the produced output can outgrow a small chunk size.
        #[derive(Debug)]
        struct FlushSizedStreamEnd;

        impl Codec for FlushSizedStreamEnd {
            fn step(&mut self, input: &[u8], output: &mut [MaybeUninit<u8>], operation: Operation) -> Result<(Step, usize, usize)> {
                assert_eq!(
                    operation,
                    Operation::Flush,
                    "the pump only offers a flush-sized budget while flushing"
                );

                for slot in output.iter_mut() {
                    slot.write(0);
                }

                Ok((Step::StreamEnd, input.len(), output.len()))
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(4));
        pump.push(view(b"x")).expect("push succeeds");
        pump.flush().expect("flush request succeeds");

        let mut codec = FlushSizedStreamEnd;
        let first = pump.pull(&mut codec).expect("pull succeeds").into_data().expect("some data");
        assert_eq!(first.len(), 4, "the first pull hands over exactly one chunk");

        let second = pump
            .pull(&mut codec)
            .expect("pull succeeds")
            .into_data()
            .expect("leftover output beyond the chunk size");
        assert!(!second.is_empty(), "the remainder must still be delivered");

        assert!(pump.pull(&mut codec).expect("pull succeeds").is_done());
    }

    #[test]
    fn rejects_input_and_final_flush_while_flushing() {
        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.flush().expect("flush request succeeds");

        assert!(pump.push(view(b"late")).expect_err("push is rejected").is_invalid_state());
        pump.end_input();
        assert!(
            pump.flush()
                .expect_err("another flush after end_input is rejected")
                .is_invalid_state()
        );
    }

    #[test]
    fn failed_codecs_reject_every_later_operation() {
        #[derive(Debug)]
        struct Fails;

        impl Codec for Fails {
            fn step(&mut self, _input: &[u8], _output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                Err(Error::corrupt_data("failed"))
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        assert!(pump.pull(&mut Fails).expect_err("codec fails").is_corrupt_data());
        pump.end_input();

        assert!(pump.push(view(b"late")).expect_err("push is rejected").is_invalid_state());
        assert!(pump.flush().expect_err("flush is rejected").is_invalid_state());
        assert!(pump.pull(&mut Fails).expect_err("pull is rejected").is_invalid_state());
    }

    #[test]
    fn rejects_an_unrequested_flush_completion() {
        #[derive(Debug)]
        struct SpuriousFlush;

        impl Codec for SpuriousFlush {
            fn step(&mut self, _input: &[u8], _output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                Ok((Step::FlushComplete, 0, 0))
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        assert!(
            pump.pull(&mut SpuriousFlush)
                .expect_err("unrequested completion is rejected")
                .is_invalid_state()
        );
    }

    #[test]
    fn propagates_stream_end_hook_failures() {
        #[derive(Debug)]
        struct BadEnd;

        impl Codec for BadEnd {
            fn step(&mut self, input: &[u8], _output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                Ok((Step::StreamEnd, input.len(), 0))
            }

            fn stream_ended(&mut self) -> Result<StreamEnd> {
                Err(Error::invalid_state("cannot reset"))
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(b"input")).expect("push succeeds");
        assert!(
            pump.pull(&mut BadEnd)
                .expect_err("stream-end hook failure propagates")
                .is_invalid_state()
        );
    }

    #[test]
    fn await_eof_completes_when_end_was_already_signaled() {
        #[derive(Debug)]
        struct StrictEnd;

        impl Codec for StrictEnd {
            fn step(&mut self, input: &[u8], _output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                Ok((Step::StreamEnd, input.len(), 0))
            }

            fn stream_ended(&mut self) -> Result<StreamEnd> {
                Ok(StreamEnd::AwaitEof)
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(b"input")).expect("push succeeds");
        pump.end_input();

        assert!(pump.pull(&mut StrictEnd).expect("strict stream completes").is_done());
    }

    #[test]
    fn a_recyclable_codec_waits_between_streams_by_default() {
        /// Reports `NextStream` and relies on the `Codec` trait's default `max_streams` (no
        /// limit) and default `check_limits` (always ok), so the pump waits between streams.
        #[derive(Debug)]
        struct Recyclable;

        impl Codec for Recyclable {
            fn step(&mut self, input: &[u8], _output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                Ok((Step::StreamEnd, input.len(), 0))
            }

            fn stream_ended(&mut self) -> Result<StreamEnd> {
                Ok(StreamEnd::NextStream)
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(b"member")).expect("push succeeds");

        assert!(pump.pull(&mut Recyclable).expect("pull succeeds").is_need_input());

        // A real `Some(n)` limit would put the pump in `State::AtStreamLimit` right here, and the
        // next `push` would fail with `stream_limit_exceeded`. Succeeding proves the default is
        // genuinely unbounded (`None`), not merely a limit this test happens not to reach.
        pump.push(view(b"second member"))
            .expect("push succeeds after one stream with no limit");
        assert!(pump.pull(&mut Recyclable).expect("pull succeeds").is_need_input());
    }

    #[test]
    fn a_stream_end_with_more_input_already_pending_loops_without_waiting() {
        // Consumes at most 5 bytes per call and always reports `NextStream`. Pushing two
        // 5-byte members back to back in a single `push`, with no `end_input`, means the second
        // member's bytes are already pending the instant the first stream ends. The pump must
        // keep looping internally (`State::Open`) to drive the already-available next stream
        // rather than surfacing `NeedInput` for data that never needed a caller round trip. A
        // mutant that disables this guard would stop after the first member instead.
        #[derive(Debug)]
        struct FixedFrame;

        impl Codec for FixedFrame {
            fn step(&mut self, input: &[u8], _output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                Ok((Step::StreamEnd, input.len().min(5), 0))
            }

            fn stream_ended(&mut self) -> Result<StreamEnd> {
                Ok(StreamEnd::NextStream)
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(b"firstsecond")).expect("push succeeds");

        assert!(pump.pull(&mut FixedFrame).expect("pull succeeds").is_need_input());
        assert_eq!(
            pump.total_in(),
            11,
            "a single pull must drive every already-pending stream instead of stopping early"
        );
    }

    #[test]
    fn reaching_the_stream_limit_exactly_at_end_of_input_completes() {
        /// Reports `NextStream` but caps at a single stream, so reaching the limit right as the
        /// caller signals end of input completes the pump instead of returning an error or waiting.
        #[derive(Debug)]
        struct StreamLimited;

        impl Codec for StreamLimited {
            fn step(&mut self, input: &[u8], _output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                Ok((Step::StreamEnd, input.len(), 0))
            }

            fn stream_ended(&mut self) -> Result<StreamEnd> {
                Ok(StreamEnd::NextStream)
            }

            fn max_streams(&self) -> Option<u64> {
                Some(1)
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(b"member")).expect("push succeeds");
        pump.end_input();

        assert!(pump.pull(&mut StreamLimited).expect("pull succeeds").is_done());
    }

    #[test]
    fn check_limits_is_reevaluated_immediately_after_a_stream_ends() {
        /// Rejects every check once at least one stream has completed, so the limit check the
        /// pump repeats right after incrementing the stream count is what catches the failure.
        #[derive(Debug)]
        struct RejectsAnotherStream;

        impl Codec for RejectsAnotherStream {
            fn step(&mut self, input: &[u8], _output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                Ok((Step::StreamEnd, input.len(), 0))
            }

            fn check_limits(&self, _total_in: u64, _total_out: u64, streams: u64) -> Result<()> {
                if streams > 0 {
                    return Err(Error::stream_limit_exceeded(streams, 0));
                }

                Ok(())
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(b"member")).expect("push succeeds");

        let error = pump
            .pull(&mut RejectsAnotherStream)
            .expect_err("the stream-count limit is enforced right after the stream ends");
        assert!(error.is_limit_exceeded());
    }

    #[test]
    fn violating_the_input_invariant_panics_before_stepping() {
        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        // Bypass the public API (which never leaves input pending in a non-driving state) to
        // exercise the engine's internal defensive check directly.
        pump.input = view(b"stray input");
        pump.state = State::BetweenStreams;

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = pump.pull(&mut Passthrough::default());
        }));

        assert!(
            panicked.is_err(),
            "a non-driving state with pending input must never reach the step loop"
        );
    }

    #[test]
    fn no_progress_with_pending_input_is_terminal() {
        #[derive(Debug)]
        struct Stalled;

        impl Codec for Stalled {
            fn step(&mut self, _input: &[u8], _output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                Ok((Step::Continue, 0, 0))
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(b"input")).expect("push succeeds");

        assert!(pump.pull(&mut Stalled).expect_err("a stalled codec is rejected").is_invalid_state());
    }

    #[test]
    fn rejects_a_second_push_while_input_is_pending() {
        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(b"first")).expect("push succeeds");

        let error = pump.push(view(b"second")).expect_err("overlapping push is rejected");
        assert!(error.is_invalid_state());
    }

    #[test]
    fn rejects_push_after_end_input() {
        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.end_input();

        let error = pump.push(view(b"late")).expect_err("push after end_input is rejected");
        assert!(error.is_invalid_state());
    }

    #[test]
    fn end_input_is_idempotent() {
        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.end_input();
        pump.end_input();

        let output = pump.pull(&mut Passthrough::default()).expect("pull succeeds");
        assert!(output.is_done());
    }

    #[test]
    fn reports_done_after_the_stream_ends() {
        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(b"tail")).expect("push succeeds");
        pump.end_input();

        let mut codec = Passthrough::default();
        let data = pump.pull(&mut codec).expect("pull succeeds").into_data().expect("data");
        assert_eq!(data.to_vec(), b"tail".to_vec());

        assert!(pump.pull(&mut codec).expect("pull succeeds").is_done());
        assert!(pump.pull(&mut codec).expect("pull succeeds").is_done());
    }

    #[test]
    fn reports_truncation_when_the_codec_never_ends() {
        /// Consumes input but never reports `StreamEnd`, imitating a truncated container.
        #[derive(Debug)]
        struct NeverEnds;

        impl Codec for NeverEnds {
            fn step(&mut self, _input: &[u8], _output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                Ok((Step::Continue, 0, 0))
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.end_input();

        let error = pump.pull(&mut NeverEnds).expect_err("truncation is reported");
        assert!(error.is_unexpected_end_of_stream());
    }

    #[test]
    fn propagates_limit_failures() {
        /// Produces output without consuming input, and rejects it via `check_limits`.
        #[derive(Debug)]
        struct Expanding;

        impl Codec for Expanding {
            fn step(&mut self, _input: &[u8], output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                for slot in output.iter_mut() {
                    slot.write(0);
                }

                Ok((Step::Continue, 0, output.len()))
            }

            fn check_limits(&self, _total_in: u64, total_out: u64, _streams: u64) -> Result<()> {
                Err(Error::output_limit_exceeded(total_out, 0))
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(b"seed")).expect("push succeeds");

        let error = pump.pull(&mut Expanding).expect_err("limit is enforced");
        assert!(error.is_limit_exceeded());
    }

    #[test]
    fn rejects_output_counts_beyond_the_provided_slice() {
        #[derive(Debug)]
        struct Overreports;

        impl Codec for Overreports {
            fn step(&mut self, _input: &[u8], output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
                output[0].write(0);
                Ok((Step::Continue, 0, output.len() + 1))
            }

            fn remaining_output(&self, _total_out: u64) -> Option<u64> {
                Some(0)
            }
        }

        let mut pump = Pump::new(GlobalPool::new(), chunk(64));
        pump.push(view(b"input")).expect("push succeeds");

        let error = pump.pull(&mut Overreports).expect_err("invalid output count is rejected");
        assert!(error.is_invalid_state(), "got {error}");
        assert_eq!(pump.total_out(), 0, "uninitialized bytes must never be advanced");
    }
}
