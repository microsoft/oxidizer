// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Whole-line recognizer for the common shape of an `Accept` field line.

use crate::validate::token_byte;

/// The byte classes an `Accept` field line is built from.
///
/// Dense indices keep the table small, and padding the row to sixteen lets the
/// scan form an index with a shift instead of a multiply.
const CLASS_OTHER: u8 = 0;
const CLASS_TCHAR: u8 = 1;
const CLASS_Q: u8 = 2;
const CLASS_ZERO: u8 = 3;
const CLASS_ONE: u8 = 4;
const CLASS_DIGIT: u8 = 5;
const CLASS_DOT: u8 = 6;
const CLASS_STAR: u8 = 7;
const CLASS_SLASH: u8 = 8;
const CLASS_SEMICOLON: u8 = 9;
const CLASS_COMMA: u8 = 10;
const CLASS_EQUALS: u8 = 11;
const CLASS_OWS: u8 = 12;

const CLASSES: usize = 16;
const STATES: usize = 32;

/// The line left the recognized subset. The state absorbs every byte, so the
/// scan needs no branch to leave the loop.
const REJECTED: u8 = 0;
/// A member is due: the line just started or a comma just closed one.
const MEMBER_DUE: u8 = 1;
/// The type read so far is exactly `*`.
const TYPE_STAR: u8 = 2;
/// A type is in progress and is not the bare wildcard.
const TYPE: u8 = 3;
/// A slash closed a `*` type, so only a `*` subtype may follow.
const SUBTYPE_DUE_STAR: u8 = 4;
/// A slash closed a named type.
const SUBTYPE_DUE: u8 = 5;
/// The subtype read so far is exactly `*` and the type was `*`.
const SUBTYPE_STAR: u8 = 6;
/// A subtype is in progress.
const SUBTYPE: u8 = 7;
/// Whitespace closed a complete member, so only `;`, `,`, or the end may follow.
const MEMBER_OWS: u8 = 8;
/// A semicolon opened a parameter.
const PARAM_DUE: u8 = 9;
/// The parameter name read so far is exactly `q`.
const NAME_Q: u8 = 10;
/// A parameter name is in progress and is not the bare `q`.
const NAME: u8 = 11;
/// An equals sign closed an ordinary parameter name.
const VALUE_DUE: u8 = 12;
/// An ordinary parameter value is in progress.
const VALUE: u8 = 13;
/// An equals sign closed the `q` name, so a quality value is due.
const QUALITY_DUE: u8 = 14;
/// The quality value read so far is `0`.
const QUALITY_ZERO: u8 = 15;
/// The quality value read so far is `1`.
const QUALITY_ONE: u8 = 16;
/// The quality value read so far is `0.` with no fraction digit yet.
const QUALITY_ZERO_DOT: u8 = 17;
/// `0.` followed by one fraction digit.
const QUALITY_ZERO_ONE_DIGIT: u8 = 18;
/// `0.` followed by two fraction digits.
const QUALITY_ZERO_TWO_DIGITS: u8 = 19;
/// `0.` followed by three fraction digits, the most the grammar allows.
const QUALITY_ZERO_THREE_DIGITS: u8 = 20;
/// The quality value read so far is `1.` with no fraction digit yet.
const QUALITY_ONE_DOT: u8 = 21;
/// `1.` followed by one fraction zero.
const QUALITY_ONE_ONE_DIGIT: u8 = 22;
/// `1.` followed by two fraction zeros.
const QUALITY_ONE_TWO_DIGITS: u8 = 23;
/// `1.` followed by three fraction zeros, the most the grammar allows.
const QUALITY_ONE_THREE_DIGITS: u8 = 24;
/// Whitespace closed a member that already carries a quality parameter.
const MEMBER_OWS_WEIGHED: u8 = 25;
/// A semicolon opened a parameter in a member that already carries a quality.
const PARAM_DUE_WEIGHED: u8 = 26;
/// A bare `q` name in a member that already carries a quality: a repeat.
const NAME_Q_WEIGHED: u8 = 27;
/// A parameter name in a member that already carries a quality.
const NAME_WEIGHED: u8 = 28;
/// An equals sign closed a parameter name after a quality.
const VALUE_DUE_WEIGHED: u8 = 29;
/// A parameter value is in progress after a quality.
const VALUE_WEIGHED: u8 = 30;

/// The states that end a line inside the recognized subset.
///
/// A member must have reached a complete subtype, a complete parameter value,
/// or a complete quality, and the line may also end between members.
const ACCEPTING: u32 = (1 << MEMBER_DUE)
    | (1 << SUBTYPE_STAR)
    | (1 << SUBTYPE)
    | (1 << MEMBER_OWS)
    | (1 << VALUE)
    | (1 << QUALITY_ZERO)
    | (1 << QUALITY_ONE)
    | (1 << QUALITY_ZERO_DOT)
    | (1 << QUALITY_ZERO_ONE_DIGIT)
    | (1 << QUALITY_ZERO_TWO_DIGITS)
    | (1 << QUALITY_ZERO_THREE_DIGITS)
    | (1 << QUALITY_ONE_DOT)
    | (1 << QUALITY_ONE_ONE_DIGIT)
    | (1 << QUALITY_ONE_TWO_DIGITS)
    | (1 << QUALITY_ONE_THREE_DIGITS)
    | (1 << MEMBER_OWS_WEIGHED)
    | (1 << VALUE_WEIGHED);

/// Maps each byte to its role inside an `Accept` field line.
static CLASS: [u8; 256] = class_table();

/// Maps a row and a byte class to the next row.
///
/// A row is a state already multiplied by [`CLASSES`], so a step indexes the
/// table with `row | class` and stores the entry back unchanged. Keeping the
/// multiply in the table removes a shift from every byte of the scan.
static TRANSITION: [u16; STATES * CLASSES] = transition_table();

/// The row a state occupies in [`TRANSITION`].
const fn row(state: u8) -> u16 {
    (state as u16) << 4
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
            b'.' => CLASS_DOT,
            b'*' => CLASS_STAR,
            b'/' => CLASS_SLASH,
            b';' => CLASS_SEMICOLON,
            b',' => CLASS_COMMA,
            b'=' => CLASS_EQUALS,
            b' ' | b'\t' => CLASS_OWS,
            _ if token_byte(value) => CLASS_TCHAR,
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
        CLASS_TCHAR | CLASS_Q | CLASS_ZERO | CLASS_ONE | CLASS_DIGIT | CLASS_DOT | CLASS_STAR
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per state keeps the whole transition relation readable in one place"
)]
const fn transition_table() -> [u16; STATES * CLASSES] {
    let mut table = [row(REJECTED); STATES * CLASSES];
    let mut state = 0_usize;
    while state < STATES {
        let mut class = 0_usize;
        while class < CLASSES {
            #[expect(clippy::cast_possible_truncation, reason = "the loop bounds keep both indices inside a byte")]
            let (state_value, class_value) = (state as u8, class as u8);
            let token = is_token_class(class_value);
            table[(state << 4) | class] = row(match state_value {
                MEMBER_DUE => match class_value {
                    CLASS_OWS | CLASS_COMMA => MEMBER_DUE,
                    CLASS_STAR => TYPE_STAR,
                    _ if token => TYPE,
                    _ => REJECTED,
                },
                TYPE_STAR => match class_value {
                    CLASS_SLASH => SUBTYPE_DUE_STAR,
                    _ if token => TYPE,
                    _ => REJECTED,
                },
                TYPE => match class_value {
                    CLASS_SLASH => SUBTYPE_DUE,
                    _ if token => TYPE,
                    _ => REJECTED,
                },
                SUBTYPE_DUE_STAR => match class_value {
                    CLASS_STAR => SUBTYPE_STAR,
                    _ => REJECTED,
                },
                SUBTYPE_DUE => {
                    if token {
                        SUBTYPE
                    } else {
                        REJECTED
                    }
                }
                SUBTYPE => match class_value {
                    CLASS_OWS => MEMBER_OWS,
                    CLASS_SEMICOLON => PARAM_DUE,
                    CLASS_COMMA => MEMBER_DUE,
                    _ if token => SUBTYPE,
                    _ => REJECTED,
                },
                SUBTYPE_STAR | MEMBER_OWS => match class_value {
                    CLASS_OWS => MEMBER_OWS,
                    CLASS_SEMICOLON => PARAM_DUE,
                    CLASS_COMMA => MEMBER_DUE,
                    _ => REJECTED,
                },
                PARAM_DUE => match class_value {
                    CLASS_OWS => PARAM_DUE,
                    CLASS_Q => NAME_Q,
                    _ if token => NAME,
                    _ => REJECTED,
                },
                NAME_Q => match class_value {
                    CLASS_EQUALS => QUALITY_DUE,
                    _ if token => NAME,
                    _ => REJECTED,
                },
                NAME => match class_value {
                    CLASS_EQUALS => VALUE_DUE,
                    _ if token => NAME,
                    _ => REJECTED,
                },
                VALUE_DUE => {
                    if token {
                        VALUE
                    } else {
                        REJECTED
                    }
                }
                VALUE => match class_value {
                    CLASS_OWS => MEMBER_OWS,
                    CLASS_SEMICOLON => PARAM_DUE,
                    CLASS_COMMA => MEMBER_DUE,
                    _ if token => VALUE,
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
                PARAM_DUE_WEIGHED => match class_value {
                    CLASS_OWS => PARAM_DUE_WEIGHED,
                    CLASS_Q => NAME_Q_WEIGHED,
                    _ if token => NAME_WEIGHED,
                    _ => REJECTED,
                },
                NAME_Q_WEIGHED => {
                    if token {
                        NAME_WEIGHED
                    } else {
                        REJECTED
                    }
                }
                NAME_WEIGHED => match class_value {
                    CLASS_EQUALS => VALUE_DUE_WEIGHED,
                    _ if token => NAME_WEIGHED,
                    _ => REJECTED,
                },
                VALUE_DUE_WEIGHED => {
                    if token {
                        VALUE_WEIGHED
                    } else {
                        REJECTED
                    }
                }
                VALUE_WEIGHED => match class_value {
                    _ if token => VALUE_WEIGHED,
                    _ => weighed_close(class_value),
                },
                _ => REJECTED,
            });
            class += 1;
        }
        state += 1;
    }
    table
}

/// Closes a member that already carries a quality parameter.
///
/// Whitespace and a further parameter stay in the weighed half of the machine
/// so a repeated `q` is rejected, while a comma starts a fresh member.
const fn weighed_close(class: u8) -> u8 {
    match class {
        CLASS_OWS => MEMBER_OWS_WEIGHED,
        CLASS_SEMICOLON => PARAM_DUE_WEIGHED,
        CLASS_COMMA => MEMBER_DUE,
        _ => REJECTED,
    }
}

/// Advances the table one byte.
///
/// Every row is already a multiple of `CLASSES` and every class is smaller
/// than `CLASSES`, so the mask changes no index it is given and exists only to
/// prove the bound, which is what lets the step compile to two loads and no
/// branch.
#[expect(clippy::inline_always, reason = "the step is the unrolled loop body and must not become a call")]
#[inline(always)]
fn step(row: u16, byte: u8) -> u16 {
    let class = CLASS[usize::from(byte)];
    let index = (usize::from(row) | usize::from(class)) & (STATES * CLASSES - 1);
    TRANSITION[index]
}

/// Reports whether a whole `Accept` field line is valid under strict decoding.
///
/// The line is read once and every byte drives one table step, so the common
/// shape — comma separated media ranges carrying token parameters and a
/// quality — is settled without splitting the line into members and walking
/// each one. Stepping eight bytes per iteration amortizes the loop counter and
/// branch, which otherwise cost about as much as the two table loads they
/// carry.
///
/// A `false` answer means "not recognized" rather than "malformed": quoted
/// strings, whitespace around an equals sign, and parameters without values
/// are all legal yet left to the general parser, which also produces the
/// diagnostic when the line really is malformed.
pub(super) fn scan_accept_line(bytes: &[u8]) -> bool {
    let mut row = row(MEMBER_DUE);

    let mut blocks = bytes.chunks_exact(8);
    for block in &mut blocks {
        for byte in block {
            row = step(row, *byte);
        }
    }
    for byte in blocks.remainder() {
        row = step(row, *byte);
    }

    ACCEPTING & (1 << (row >> 4)) != 0
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::hint::black_box;

    use super::{ACCEPTING, CLASS, CLASSES, REJECTED, STATES, TRANSITION, class_table, row, transition_table};

    #[test]
    fn runtime_tables_match_the_static_tables() {
        assert_eq!(black_box(class_table()), CLASS);

        let generated = black_box(transition_table());
        assert_eq!(generated, TRANSITION);

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
        assert_eq!(
            ACCEPTING & (1 << REJECTED),
            0,
            "a rejected line must never end inside the recognized subset"
        );
    }
}
