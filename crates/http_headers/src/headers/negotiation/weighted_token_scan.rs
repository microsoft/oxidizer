// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Whole-line recognizer for the weighted-token negotiation field lines.
//!
//! `Accept-Encoding` and `Accept-Language` share a member grammar: an item
//! followed by at most one quality parameter. Only the item differs, so one
//! machine serves both and the item shape selects which table is built.

/// The item a member starts with.
#[derive(Clone, Copy)]
pub(super) enum Item {
    /// Any RFC 9110 token, which is what a content coding is.
    Token,
    /// `*`, or an alphabetic primary tag and alphanumeric subtags of one to
    /// eight bytes each.
    LanguageRange,
}

const CLASS_OTHER: u8 = 0;
const CLASS_TCHAR: u8 = 1;
const CLASS_Q: u8 = 2;
const CLASS_ALPHA: u8 = 3;
const CLASS_ZERO: u8 = 4;
const CLASS_ONE: u8 = 5;
const CLASS_DIGIT: u8 = 6;
const CLASS_DOT: u8 = 7;
const CLASS_STAR: u8 = 8;
const CLASS_HYPHEN: u8 = 9;
const CLASS_SEMICOLON: u8 = 10;
const CLASS_COMMA: u8 = 11;
const CLASS_EQUALS: u8 = 12;
const CLASS_OWS: u8 = 13;

const CLASSES: usize = 16;
const STATES: usize = 64;

/// The line left the recognized subset. The state absorbs every later byte.
const REJECTED: u8 = 0;
/// A member is due: the line just started or a comma just closed one.
const MEMBER_DUE: u8 = 1;
/// Whitespace closed a complete item, so only `;`, `,`, or the end may follow.
const MEMBER_OWS: u8 = 2;
/// A semicolon opened the quality parameter.
const PARAM_DUE: u8 = 3;
/// The parameter name `q` has been read.
const NAME_Q: u8 = 4;
/// An equals sign closed the `q` name, so a quality value is due.
const QUALITY_DUE: u8 = 5;
/// The quality value read so far is `0`.
const QUALITY_ZERO: u8 = 6;
/// The quality value read so far is `1`.
const QUALITY_ONE: u8 = 7;
/// `0.` with no fraction digit yet.
const QUALITY_ZERO_DOT: u8 = 8;
/// `0.` followed by one fraction digit.
const QUALITY_ZERO_ONE_DIGIT: u8 = 9;
/// `0.` followed by two fraction digits.
const QUALITY_ZERO_TWO_DIGITS: u8 = 10;
/// `0.` followed by three fraction digits, the most the grammar allows.
const QUALITY_ZERO_THREE_DIGITS: u8 = 11;
/// `1.` with no fraction zero yet.
const QUALITY_ONE_DOT: u8 = 12;
/// `1.` followed by one fraction zero.
const QUALITY_ONE_ONE_DIGIT: u8 = 13;
/// `1.` followed by two fraction zeros.
const QUALITY_ONE_TWO_DIGITS: u8 = 14;
/// `1.` followed by three fraction zeros, the most the grammar allows.
const QUALITY_ONE_THREE_DIGITS: u8 = 15;
/// Whitespace closed a member that already carries its quality.
const MEMBER_OWS_WEIGHED: u8 = 16;

/// A token item is in progress.
const TOKEN: u8 = 17;
/// The language range read so far is exactly `*`.
const LANGUAGE_STAR: u8 = 17;
/// The primary tag is one byte long. Seven more states follow it.
const PRIMARY_ONE: u8 = 18;
/// The primary tag has reached the eight-byte limit.
const PRIMARY_LAST: u8 = 25;
/// A hyphen closed a tag, so a subtag is due.
const SUBTAG_DUE: u8 = 26;
/// A subtag is one byte long. Seven more states follow it.
const SUBTAG_ONE: u8 = 27;
/// A subtag has reached the eight-byte limit.
const SUBTAG_LAST: u8 = 34;

/// The states that end a line inside the recognized subset, other than the
/// item states, which each shape contributes itself.
const SHARED_ACCEPTING: u64 = (1 << MEMBER_DUE)
    | (1 << MEMBER_OWS)
    | (1 << MEMBER_OWS_WEIGHED)
    | (1 << QUALITY_ZERO)
    | (1 << QUALITY_ONE)
    | (1 << QUALITY_ZERO_DOT)
    | (1 << QUALITY_ZERO_ONE_DIGIT)
    | (1 << QUALITY_ZERO_TWO_DIGITS)
    | (1 << QUALITY_ZERO_THREE_DIGITS)
    | (1 << QUALITY_ONE_DOT)
    | (1 << QUALITY_ONE_ONE_DIGIT)
    | (1 << QUALITY_ONE_TWO_DIGITS)
    | (1 << QUALITY_ONE_THREE_DIGITS);

/// Maps each byte to its role inside a weighted-token field line.
static CLASS: [u8; 256] = class_table();

/// Maps a row and a byte class to the next row.
///
/// A row is a state already multiplied by [`CLASSES`], so a step indexes the
/// table with `row | class` and stores the entry back unchanged. Keeping the
/// multiply in the table removes a shift from every byte of the scan.
static TOKEN_TRANSITION: [u16; STATES * CLASSES] = transition_table(Item::Token);
static LANGUAGE_TRANSITION: [u16; STATES * CLASSES] = transition_table(Item::LanguageRange);

/// The row a state occupies in a transition table.
const fn row(state: u8) -> u16 {
    (state as u16) << 4
}

const TOKEN_ACCEPTING: u64 = SHARED_ACCEPTING | (1 << TOKEN);
const LANGUAGE_ACCEPTING: u64 =
    SHARED_ACCEPTING | (1 << LANGUAGE_STAR) | range_mask(PRIMARY_ONE, PRIMARY_LAST) | range_mask(SUBTAG_ONE, SUBTAG_LAST);

/// Builds a mask holding every state from `first` to `last` inclusive.
const fn range_mask(first: u8, last: u8) -> u64 {
    let mut mask = 0_u64;
    let mut state = first;
    while state <= last {
        mask |= 1 << state;
        state += 1;
    }
    mask
}

const fn class_table() -> [u8; 256] {
    let mut table = [CLASS_OTHER; 256];
    let mut byte = 0_usize;
    while byte < 256 {
        #[expect(clippy::cast_possible_truncation, reason = "the loop bound keeps the index inside a byte")]
        let value = byte as u8;
        table[byte] = match value {
            b'q' | b'Q' => CLASS_Q,
            b'0' => CLASS_ZERO,
            b'1' => CLASS_ONE,
            b'2'..=b'9' => CLASS_DIGIT,
            b'A'..=b'Z' | b'a'..=b'z' => CLASS_ALPHA,
            b'.' => CLASS_DOT,
            b'*' => CLASS_STAR,
            b'-' => CLASS_HYPHEN,
            b';' => CLASS_SEMICOLON,
            b',' => CLASS_COMMA,
            b'=' => CLASS_EQUALS,
            b' ' | b'\t' => CLASS_OWS,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'+' | b'^' | b'_' | b'`' | b'|' | b'~' => CLASS_TCHAR,
            _ => CLASS_OTHER,
        };
        byte += 1;
    }
    table
}

/// Returns whether a class is one of the `tchar` classes.
const fn is_token_class(class: u8) -> bool {
    matches!(
        class,
        CLASS_TCHAR | CLASS_Q | CLASS_ALPHA | CLASS_ZERO | CLASS_ONE | CLASS_DIGIT | CLASS_DOT | CLASS_STAR | CLASS_HYPHEN
    )
}

/// Returns whether a class is an ASCII letter.
const fn is_alpha_class(class: u8) -> bool {
    matches!(class, CLASS_Q | CLASS_ALPHA)
}

/// Returns whether a class is an ASCII letter or digit.
const fn is_alphanumeric_class(class: u8) -> bool {
    matches!(class, CLASS_Q | CLASS_ALPHA | CLASS_ZERO | CLASS_ONE | CLASS_DIGIT)
}

/// Closes a complete item, which may carry a quality parameter next.
const fn item_close(class: u8) -> u8 {
    match class {
        CLASS_OWS => MEMBER_OWS,
        CLASS_SEMICOLON => PARAM_DUE,
        CLASS_COMMA => MEMBER_DUE,
        _ => REJECTED,
    }
}

/// Closes a member whose quality parameter is complete.
///
/// A second parameter is always an error in this grammar, so a semicolon
/// leaves the subset rather than opening one.
const fn weighed_close(class: u8) -> u8 {
    match class {
        CLASS_OWS => MEMBER_OWS_WEIGHED,
        CLASS_COMMA => MEMBER_DUE,
        _ => REJECTED,
    }
}

/// Steps the item half of the machine for one shape.
const fn item_step(item: Item, state: u8, class: u8) -> u8 {
    match item {
        Item::Token => match state {
            MEMBER_DUE | TOKEN if is_token_class(class) => TOKEN,
            TOKEN => item_close(class),
            _ => REJECTED,
        },
        Item::LanguageRange => match state {
            MEMBER_DUE if class == CLASS_STAR => LANGUAGE_STAR,
            MEMBER_DUE if is_alpha_class(class) => PRIMARY_ONE,
            LANGUAGE_STAR => item_close(class),
            _ if state >= PRIMARY_ONE && state < PRIMARY_LAST && is_alpha_class(class) => state + 1,
            _ if state >= PRIMARY_ONE && state <= PRIMARY_LAST && class == CLASS_HYPHEN => SUBTAG_DUE,
            _ if state >= PRIMARY_ONE && state <= PRIMARY_LAST => item_close(class),
            SUBTAG_DUE if is_alphanumeric_class(class) => SUBTAG_ONE,
            _ if state >= SUBTAG_ONE && state < SUBTAG_LAST && is_alphanumeric_class(class) => state + 1,
            _ if state >= SUBTAG_ONE && state <= SUBTAG_LAST && class == CLASS_HYPHEN => SUBTAG_DUE,
            _ if state >= SUBTAG_ONE && state <= SUBTAG_LAST => item_close(class),
            _ => REJECTED,
        },
    }
}

const fn transition_table(item: Item) -> [u16; STATES * CLASSES] {
    let mut table = [row(REJECTED); STATES * CLASSES];
    let mut state = 0_usize;
    while state < STATES {
        let mut class = 0_usize;
        while class < CLASSES {
            #[expect(clippy::cast_possible_truncation, reason = "the loop bounds keep both indices inside a byte")]
            let (state_value, class_value) = (state as u8, class as u8);
            table[(state << 4) | class] = row(match state_value {
                MEMBER_DUE => match class_value {
                    CLASS_OWS | CLASS_COMMA => MEMBER_DUE,
                    _ => item_step(item, MEMBER_DUE, class_value),
                },
                MEMBER_OWS => item_close(class_value),
                PARAM_DUE => match class_value {
                    CLASS_OWS => PARAM_DUE,
                    CLASS_Q => NAME_Q,
                    _ => REJECTED,
                },
                NAME_Q => match class_value {
                    CLASS_EQUALS => QUALITY_DUE,
                    _ => REJECTED,
                },
                QUALITY_DUE => match class_value {
                    CLASS_ZERO => QUALITY_ZERO,
                    CLASS_ONE => QUALITY_ONE,
                    _ => REJECTED,
                },
                QUALITY_ZERO => match class_value {
                    CLASS_DOT => QUALITY_ZERO_DOT,
                    _ => weighed_close(class_value),
                },
                QUALITY_ONE => match class_value {
                    CLASS_DOT => QUALITY_ONE_DOT,
                    _ => weighed_close(class_value),
                },
                QUALITY_ZERO_DOT => match class_value {
                    CLASS_ZERO | CLASS_ONE | CLASS_DIGIT => QUALITY_ZERO_ONE_DIGIT,
                    _ => weighed_close(class_value),
                },
                QUALITY_ZERO_ONE_DIGIT => match class_value {
                    CLASS_ZERO | CLASS_ONE | CLASS_DIGIT => QUALITY_ZERO_TWO_DIGITS,
                    _ => weighed_close(class_value),
                },
                QUALITY_ZERO_TWO_DIGITS => match class_value {
                    CLASS_ZERO | CLASS_ONE | CLASS_DIGIT => QUALITY_ZERO_THREE_DIGITS,
                    _ => weighed_close(class_value),
                },
                QUALITY_ONE_DOT => match class_value {
                    CLASS_ZERO => QUALITY_ONE_ONE_DIGIT,
                    _ => weighed_close(class_value),
                },
                QUALITY_ONE_ONE_DIGIT => match class_value {
                    CLASS_ZERO => QUALITY_ONE_TWO_DIGITS,
                    _ => weighed_close(class_value),
                },
                QUALITY_ONE_TWO_DIGITS => match class_value {
                    CLASS_ZERO => QUALITY_ONE_THREE_DIGITS,
                    _ => weighed_close(class_value),
                },
                QUALITY_ZERO_THREE_DIGITS | QUALITY_ONE_THREE_DIGITS | MEMBER_OWS_WEIGHED => weighed_close(class_value),
                _ => item_step(item, state_value, class_value),
            });
            class += 1;
        }
        state += 1;
    }
    table
}

/// Advances the table one byte.
///
/// Every row is already a multiple of `CLASSES` and every class is smaller
/// than `CLASSES`, so the mask changes no index it is given and exists only to
/// prove the bound, which is what lets the step compile to two loads and no
/// branch.
#[expect(clippy::inline_always, reason = "the step is the unrolled loop body and must not become a call")]
#[inline(always)]
fn step(row: u16, byte: u8, transition: &[u16; STATES * CLASSES]) -> u16 {
    let class = CLASS[usize::from(byte)];
    let index = (usize::from(row) | usize::from(class)) & (STATES * CLASSES - 1);
    transition[index]
}

/// Runs one table to completion over a whole field line.
///
/// Stepping eight bytes per iteration amortizes the loop counter and branch,
/// which otherwise cost about as much as the two table loads they carry.
fn scan(bytes: &[u8], transition: &[u16; STATES * CLASSES], accepting: u64) -> bool {
    let mut row = row(MEMBER_DUE);

    let mut blocks = bytes.chunks_exact(8);
    for block in &mut blocks {
        for byte in block {
            row = step(row, *byte, transition);
        }
    }
    for byte in blocks.remainder() {
        row = step(row, *byte, transition);
    }

    accepting & (1 << (row >> 4)) != 0
}

/// Reports whether a whole `Accept-Encoding` field line is valid.
///
/// A `false` answer means "not recognized" rather than "malformed": quoted
/// strings and whitespace around an equals sign are left to the general
/// parser, which also produces the diagnostic when the line really is
/// malformed.
pub(super) fn scan_accept_encoding_line(bytes: &[u8]) -> bool {
    scan(bytes, &TOKEN_TRANSITION, TOKEN_ACCEPTING)
}

/// Reports whether a whole `Accept-Language` field line is valid.
///
/// A `false` answer means "not recognized" rather than "malformed", exactly as
/// for [`scan_accept_encoding_line`].
pub(super) fn scan_accept_language_line(bytes: &[u8]) -> bool {
    scan(bytes, &LANGUAGE_TRANSITION, LANGUAGE_ACCEPTING)
}

#[cfg(test)]
pub(super) mod corpus {
    /// Builds every string up to `length` bytes over `alphabet`.
    pub(in super::super) fn exhaustive(alphabet: &[u8], length: usize) -> Vec<Vec<u8>> {
        let mut all = vec![Vec::new()];
        let mut frontier = vec![Vec::new()];
        for _ in 0..length {
            let mut next = Vec::new();
            for prefix in &frontier {
                for byte in alphabet {
                    let mut candidate = prefix.clone();
                    candidate.push(*byte);
                    next.push(candidate);
                }
            }
            all.extend_from_slice(&next);
            frontier = next;
        }
        all
    }

    /// Builds every concatenation of up to `count` of the given fragments.
    pub(in super::super) fn fragment_lines(fragments: &[&str], count: usize) -> Vec<Vec<u8>> {
        let mut lines = vec![Vec::new()];
        for _ in 0..count {
            let mut next = Vec::new();
            for prefix in &lines {
                for fragment in fragments {
                    let mut candidate = prefix.clone();
                    candidate.extend_from_slice(fragment.as_bytes());
                    next.push(candidate);
                }
            }
            lines.extend_from_slice(&next);
        }
        lines
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{
        CLASS, CLASSES, Item, LANGUAGE_ACCEPTING, LANGUAGE_STAR, LANGUAGE_TRANSITION, PRIMARY_LAST, PRIMARY_ONE, REJECTED,
        SHARED_ACCEPTING, STATES, SUBTAG_LAST, SUBTAG_ONE, TOKEN, TOKEN_ACCEPTING, TOKEN_TRANSITION, class_table, range_mask, row,
        transition_table,
    };

    #[test]
    fn runtime_tables_match_the_static_tables() {
        assert_eq!(std::hint::black_box(class_table()), CLASS);

        for (item, table) in [(Item::Token, &TOKEN_TRANSITION), (Item::LanguageRange, &LANGUAGE_TRANSITION)] {
            let generated = std::hint::black_box(transition_table(item));
            assert_eq!(&generated, table);

            for class in 0..CLASSES {
                assert_eq!(
                    generated[usize::from(row(REJECTED)) | class],
                    row(REJECTED),
                    "rejection must absorb every later byte"
                );
            }
            for entry in generated {
                assert!(usize::from(entry >> 4) < STATES, "every transition must land on a defined state");
                assert_eq!(entry & 0xf, 0, "every entry must be a state already multiplied by the class count");
            }
        }
    }

    #[test]
    fn runtime_accepting_masks_match_the_constants() {
        let token = SHARED_ACCEPTING | (1 << TOKEN);
        let language = SHARED_ACCEPTING
            | (1 << LANGUAGE_STAR)
            | std::hint::black_box(range_mask(PRIMARY_ONE, PRIMARY_LAST))
            | std::hint::black_box(range_mask(SUBTAG_ONE, SUBTAG_LAST));

        assert_eq!(token, TOKEN_ACCEPTING);
        assert_eq!(language, LANGUAGE_ACCEPTING);
        assert_eq!(TOKEN_ACCEPTING & (1 << REJECTED), 0);
        assert_eq!(LANGUAGE_ACCEPTING & (1 << REJECTED), 0);
    }
}
