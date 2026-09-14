// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Comma-separated lists of bare HTTP tokens.
//!
//! The grammar accepted here is the `#rule` expansion of [RFC 9110 section
//! 5.6.1.2] restricted to members that are bare tokens: a field line is a
//! sequence of comma-delimited members, each surrounded by optional
//! whitespace, and each member is either empty or one token. Quoted strings,
//! parameters, and any other byte leave the subset, so a rejected line means
//! "not a simple token list" and never "definitely malformed": callers that
//! need a diagnostic re-scan the line with their own parser.
//!
//! [RFC 9110 section 5.6.1.2]: https://www.rfc-editor.org/rfc/rfc9110#section-5.6.1.2

/// How a list scan treats zero-length members.
///
/// # Examples
///
/// ```
/// use http_headers_simd::{EmptyMembers, TokenListScan, scan_token_list};
///
/// assert_eq!(
///     scan_token_list(b"gzip,,br", EmptyMembers::Skip),
///     TokenListScan::Members,
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmptyMembers {
    /// Empty members are ignored, which is what `#rule` expansion prescribes.
    ///
    /// `a,,b` and `,a,` both hold the members `a` and `b`.
    Skip,
    /// An empty member rejects the whole field line.
    ///
    /// A line that holds nothing but optional whitespace is still reported as
    /// [`TokenListScan::Empty`] rather than rejected, because it carries no
    /// member at all and callers report that as a missing value.
    Reject,
}

/// What one field line turned out to be.
///
/// # Examples
///
/// ```
/// use http_headers_simd::{EmptyMembers, TokenListScan, scan_token_list};
///
/// assert_eq!(
///     scan_token_list(b"gzip;q=1", EmptyMembers::Skip),
///     TokenListScan::Rejected,
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenListScan {
    /// A valid list holding at least one token member.
    Members,
    /// A valid list holding no member at all.
    Empty,
    /// Not a simple token list under the requested empty-member policy.
    Rejected,
}

/// The byte classes a token list is built from.
/// Dense indices keep the classification table cache-small and directly indexable.
const CLASS_OTHER: usize = 0;
const CLASS_TOKEN: usize = 1;
const CLASS_OWS: usize = 2;
const CLASS_COMMA: usize = 3;

/// The current member is still empty: the line just started or a comma just
/// closed the member before it.
const STATE_MEMBER_DUE: u8 = 0;
/// A token run is in progress.
const STATE_IN_TOKEN: u8 = 1;
/// Whitespace opened right after a token, so only a comma may follow.
const STATE_AFTER_TOKEN: u8 = 2;
/// The line left the subset. The state absorbs, so the scan needs no branch to
/// leave the loop and the outcome is still exact.
const STATE_REJECTED: u8 = 3;

/// A non-empty member has been seen.
const FLAG_MEMBER: u8 = 4;
/// An empty member has been seen.
const FLAG_EMPTY: u8 = 8;

/// Drives the scan one byte at a time.
///
/// The entry for a state and a byte holds the next state in its low two bits
/// and the flags that byte raises above them, so a byte costs one indexed load,
/// one `or` into the sticky flags, and one mask to form the next row.
static LIST_TRANSITION: [u8; 4 * 256] = list_transition_table();

const fn list_transition_table() -> [u8; 4 * 256] {
    let mut table = [STATE_REJECTED; 4 * 256];
    let mut state = 0_usize;
    while state < 4 {
        let mut byte = 0_usize;
        while byte < 256 {
            #[expect(clippy::cast_possible_truncation, reason = "the loop bound keeps the index inside a byte")]
            let class = byte_class(byte as u8);
            table[(state << 8) | byte] = match (state, class) {
                (_, CLASS_OTHER) | (3, _) | (2, CLASS_TOKEN) => STATE_REJECTED,
                (0, CLASS_TOKEN) => STATE_IN_TOKEN | FLAG_MEMBER,
                (_, CLASS_TOKEN) => STATE_IN_TOKEN,
                (0, CLASS_OWS) => STATE_MEMBER_DUE,
                (_, CLASS_OWS) => STATE_AFTER_TOKEN,
                (0, _) => STATE_MEMBER_DUE | FLAG_EMPTY,
                _ => STATE_MEMBER_DUE,
            };
            byte += 1;
        }
        state += 1;
    }
    table
}

/// Maps one byte to its role inside a comma-separated token list.
const fn byte_class(byte: u8) -> usize {
    if is_token_byte(byte) {
        CLASS_TOKEN
    } else if byte == b',' {
        CLASS_COMMA
    } else if byte == b' ' || byte == b'\t' {
        CLASS_OWS
    } else {
        CLASS_OTHER
    }
}

/// Returns whether one byte is an RFC 9110 `tchar`.
const fn is_token_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'!' | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
            | b'0'..=b'9'
            | b'A'..=b'Z'
            | b'a'..=b'z'
    )
}

/// Runs the whole scan without any architecture-specific acceleration.
pub(super) fn scan_token_list(bytes: &[u8], empty: EmptyMembers) -> TokenListScan {
    ListScan::new().finish(bytes, empty)
}

/// Joins the class masks of two overlapping vectors into one lane mask.
///
/// `low` covers the first vector of the line and `high` the vector that ends
/// it, so the two overlap by `shift` lanes and dropping that many lanes from
/// `high` leaves exactly the lanes the line still owes.
pub(super) fn join_lanes(low: u32, high: u32, shift: u32) -> u64 {
    u64::from(low) | (u64::from(high >> shift) << 16)
}

/// The state a token-list scan carries between blocks and bytes.
///
/// Accelerated scanners fold whole blocks with [`ListScan::push_block`] and
/// hand the trailing bytes to [`ListScan::finish`], so the grammar itself
/// lives here once rather than in every architecture's kernel.
#[derive(Clone, Copy, Debug)]
pub(super) struct ListScan {
    /// Where the scan stands between members, as one of the `STATE_` values.
    state: u8,
    /// The `FLAG_` bits the bytes fed one at a time have raised so far.
    flags: u8,
    /// Every token lane a block has reported, kept as raw bits so a block
    /// costs one `or` rather than a comparison and a shift.
    member_lanes: u64,
    /// Every lane at which a block closed an empty member, kept as raw bits
    /// for the same reason.
    empty_lanes: u64,
}

impl ListScan {
    /// Starts a scan at the beginning of a field line.
    pub(super) const fn new() -> Self {
        Self {
            state: STATE_MEMBER_DUE,
            flags: 0,
            member_lanes: 0,
            empty_lanes: 0,
        }
    }

    /// Folds one 16-byte block described by its per-lane class masks.
    ///
    /// `tokens`, `ows`, and `commas` hold one bit per lane, lane zero in bit
    /// zero, and every other bit must be clear. Returns `false` as soon as the
    /// block cannot belong to a token list, which happens either because a lane
    /// is outside the three classes or because whitespace alone separates two
    /// members.
    pub(super) fn push_block(&mut self, tokens: u32, ows: u32, commas: u32) -> bool {
        self.push_lanes(u64::from(tokens), u64::from(ows), u64::from(commas), 16)
    }

    /// Folds the `lanes` low lanes of a block that covers only part of a line.
    ///
    /// A scanner reaching a trailing run shorter than one vector reloads the
    /// last whole vector of the line and shifts its masks down, so the lanes
    /// it already folded fall away and lane zero is the first byte still
    /// owed. `lanes` must be between one and sixteen and every mask bit at or
    /// above it must be clear.
    pub(super) fn push_partial_block(&mut self, tokens: u32, ows: u32, commas: u32, lanes: u32) -> bool {
        self.push_lanes(u64::from(tokens), u64::from(ows), u64::from(commas), lanes)
    }

    /// Folds a whole line of at most two vectors as one run of `lanes` lanes.
    ///
    /// The grammar's cost is dominated by the fold rather than by the class
    /// masks, so a line that spans two vectors is cheaper to classify twice
    /// and fold once than to fold twice. `lanes` must be between one and
    /// thirty-two and every mask bit at or above it must be clear.
    pub(super) fn push_line(&mut self, tokens: u64, ows: u64, commas: u64, lanes: u32) -> bool {
        self.push_lanes(tokens, ows, commas, lanes)
    }

    /// Folds `lanes` lanes, which is the whole grammar for a block of any width.
    #[expect(clippy::inline_always, reason = "the whole-block caller folds the lane count into its constants")]
    #[inline(always)]
    fn push_lanes(&mut self, tokens: u64, ows: u64, commas: u64, lanes: u32) -> bool {
        // The two carry-sensitive facts about the byte before the block are
        // whether the current member is still empty and whether whitespace may
        // no longer be followed by a token, and both are one bit, so the whole
        // block folds without a branch until the single rejection test.
        let due = u64::from(self.state == STATE_MEMBER_DUE);
        // A block is only ever folded into a state the previous block or byte
        // left behind, and both leave one of the first three states, so the
        // high bit of the state alone says whether whitespace closed a token.
        let after_token = u64::from(self.state >> 1);
        let leading_ows = ows & 1;

        // Whitespace runs opening right after a token get a start bit, and
        // adding those bits to the run carries through it, so the carry lands
        // on the byte that ends the run. A token there is a member that no
        // comma separated from the one before it. Landing bits are the only
        // bits the sum can place outside the whitespace mask, so intersecting
        // with the token mask needs no further masking.
        let after_tokens = tokens << 1;
        let token_runs = (after_tokens & ows) | (leading_ows & (due ^ 1));
        let carried = ows + token_runs;

        // The same carry finds empty members: a run that opens right after a
        // comma and lands on another comma spans a member holding nothing, and
        // adjacent commas are the same thing with no whitespace between.
        let after_commas = commas << 1;
        let empty_runs = (after_commas & ows) | (leading_ows & due);
        let closed_empty = ((ows + empty_runs) & commas) | (after_commas & commas) | (commas & due);

        // A lane outside the three classes leaves a hole in the union, a token
        // in lane zero cannot follow whitespace that closed a token, and a
        // landing bit on a token is a member with no comma before it.
        let all = (1_u64 << lanes) - 1;
        let rejected = ((tokens | ows | commas) ^ all) | (after_token & tokens) | (carried & tokens);
        if rejected != 0 {
            self.state = STATE_REJECTED;
            return false;
        }

        // A block ends in a token, in a whitespace run that a token opened, or
        // with the next member due; the first two are mutually exclusive
        // because one lane cannot be both.
        self.state = u8::from(tokens & (1_u64 << (lanes - 1)) != 0) | (u8::from(carried & (1_u64 << lanes) != 0) << 1);
        self.member_lanes |= tokens;
        self.empty_lanes |= closed_empty;
        true
    }

    /// Folds the trailing bytes and reports the outcome for the whole line.
    pub(super) fn finish(self, tail: &[u8], empty: EmptyMembers) -> TokenListScan {
        let mut row = usize::from(self.state) << 8;
        let mut flags = self.flags;
        for &byte in tail {
            let entry = LIST_TRANSITION[row | usize::from(byte)];
            flags |= entry;
            row = usize::from(entry & 3) << 8;
        }
        #[expect(clippy::cast_possible_truncation, reason = "the row is one of the four states shifted into place")]
        let state = (row >> 8) as u8;
        if state == STATE_REJECTED {
            return TokenListScan::Rejected;
        }

        let members = flags & FLAG_MEMBER != 0 || self.member_lanes != 0;
        // A line that ends with a member still due ends on an empty member,
        // unless nothing at all preceded it and the line simply has no members.
        let empties = flags & FLAG_EMPTY != 0 || self.empty_lanes != 0 || state == STATE_MEMBER_DUE && members;
        match empty {
            EmptyMembers::Skip => {
                if members {
                    TokenListScan::Members
                } else {
                    TokenListScan::Empty
                }
            }
            EmptyMembers::Reject => {
                if !members && !empties {
                    TokenListScan::Empty
                } else if empties {
                    TokenListScan::Rejected
                } else {
                    TokenListScan::Members
                }
            }
        }
    }
}

/// Splits on commas and checks every trimmed member, which is the grammar
/// spelled the obvious way rather than the way the scanners walk it.
///
/// Every differential test in this crate compares an implementation against
/// this, so it is deliberately the slow and literal reading of the rule.
#[cfg(test)]
pub(super) fn oracle(bytes: &[u8], empty: EmptyMembers) -> TokenListScan {
    let mut members = 0_usize;
    let mut empties = 0_usize;
    for member in bytes.split(|byte| *byte == b',') {
        let member = trim_ows(member);
        if member.is_empty() {
            empties += 1;
        } else if member.iter().copied().all(is_token_byte) {
            members += 1;
        } else {
            return TokenListScan::Rejected;
        }
    }
    match empty {
        EmptyMembers::Skip => {
            if members == 0 {
                TokenListScan::Empty
            } else {
                TokenListScan::Members
            }
        }
        EmptyMembers::Reject => {
            if members == 0 && empties == 1 {
                TokenListScan::Empty
            } else if empties == 0 {
                TokenListScan::Members
            } else {
                TokenListScan::Rejected
            }
        }
    }
}

#[cfg(test)]
fn trim_ows(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|byte| *byte != b' ' && *byte != b'\t').unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| *byte != b' ' && *byte != b'\t')
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::hint;
    #[cfg(not(feature = "std"))]
    use std::vec::Vec;

    use super::*;

    #[test]
    fn token_class_matches_the_shared_scalar_validator() {
        for byte in u8::MIN..=u8::MAX {
            assert_eq!(
                byte_class(byte) == CLASS_TOKEN,
                crate::scalar::all_token_bytes(&[byte]),
                "byte {byte:#04x}"
            );
        }
    }

    #[test]
    fn runtime_transition_table_matches_the_static_table() {
        let generated = hint::black_box(list_transition_table());
        assert_eq!(generated, LIST_TRANSITION);

        for state in 0..4 {
            for byte in u8::MIN..=u8::MAX {
                let entry = generated[(state << 8) | usize::from(byte)];
                assert_eq!(entry & !(3 | FLAG_MEMBER | FLAG_EMPTY), 0);
                if state == usize::from(STATE_REJECTED) {
                    assert_eq!(entry, STATE_REJECTED);
                }
            }
        }
    }

    #[test]
    fn overlapping_lane_masks_join_without_repeating_lanes() {
        assert_eq!(join_lanes(0x0000_ffff, 0xffff_0000, 16), 0x0000_0000_ffff_ffff);
        assert_eq!(join_lanes(0x0000_00ff, 0x0000_ff00, 8), 0x0000_0000_00ff_00ff);
    }

    #[test]
    fn scan_matches_the_oracle_for_every_short_alphabet_string() {
        let alphabet = b"a,\t %\0";
        let mut buffer = Vec::new();
        for length in 0..=4 {
            walk(alphabet, length, &mut buffer);
        }
    }

    fn walk(alphabet: &[u8], length: usize, buffer: &mut Vec<u8>) {
        if length == 0 {
            for empty in [EmptyMembers::Skip, EmptyMembers::Reject] {
                assert_eq!(scan_token_list(buffer, empty), oracle(buffer, empty), "{buffer:?} {empty:?}");
            }
            return;
        }
        for &byte in alphabet {
            buffer.push(byte);
            walk(alphabet, length - 1, buffer);
            buffer.pop();
        }
    }

    #[test]
    fn whitespace_separated_members_need_a_comma() {
        assert_eq!(scan_token_list(b"gzip deflate", EmptyMembers::Skip), TokenListScan::Rejected);
        assert_eq!(scan_token_list(b"gzip , deflate", EmptyMembers::Skip), TokenListScan::Members);
    }

    #[test]
    fn empty_members_follow_the_policy() {
        assert_eq!(scan_token_list(b"gzip,,deflate", EmptyMembers::Skip), TokenListScan::Members);
        assert_eq!(scan_token_list(b"gzip,,deflate", EmptyMembers::Reject), TokenListScan::Rejected);
        assert_eq!(scan_token_list(b"  \t ", EmptyMembers::Reject), TokenListScan::Empty);
        assert_eq!(scan_token_list(b"", EmptyMembers::Skip), TokenListScan::Empty);
    }

    #[test]
    fn quoted_and_control_bytes_leave_the_subset() {
        assert_eq!(scan_token_list(b"\"gzip\"", EmptyMembers::Skip), TokenListScan::Rejected);
        assert_eq!(scan_token_list(b"gzip;q=1", EmptyMembers::Skip), TokenListScan::Rejected);
    }
}
