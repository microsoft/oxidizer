// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Display, Formatter};

/// A health state reported by a publisher or computed for a component or process.
///
/// States are ordered from best to worst, so aggregation selects the maximum:
/// `Healthy < Degraded < Unhealthy`.
///
/// # Examples
///
/// ```
/// use health::Health;
///
/// let worst = [Health::Healthy, Health::Unhealthy, Health::Degraded]
///     .into_iter()
///     .max();
/// assert_eq!(worst, Some(Health::Unhealthy));
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Health {
    /// Fully operational.
    Healthy,
    /// Operational with reduced capability or quality.
    Degraded,
    /// Not operational.
    Unhealthy,
}

impl Health {
    pub(crate) const COUNT: usize = 3;
    pub(crate) const ALL: [Self; Self::COUNT] = [Self::Healthy, Self::Degraded, Self::Unhealthy];

    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    pub(crate) const fn from_index(index: u8) -> Self {
        match index {
            0 => Self::Healthy,
            1 => Self::Degraded,
            _ => Self::Unhealthy,
        }
    }

    /// Returns the worst state with a nonzero count, or `Unhealthy` when every count is zero.
    pub(crate) fn worst(counts: &[usize; Self::COUNT]) -> Self {
        Self::ALL
            .into_iter()
            .rev()
            .find(|state| counts[state.index()] > 0)
            .unwrap_or(Self::Unhealthy)
    }
}

impl Display for Health {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Healthy => "Healthy",
            Self::Degraded => "Degraded",
            Self::Unhealthy => "Unhealthy",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worst_selects_most_severe_present_state() {
        assert_eq!(Health::worst(&[0, 0, 0]), Health::Unhealthy);
        assert_eq!(Health::worst(&[2, 0, 0]), Health::Healthy);
        assert_eq!(Health::worst(&[2, 1, 0]), Health::Degraded);
        assert_eq!(Health::worst(&[2, 1, 1]), Health::Unhealthy);
    }

    #[test]
    fn index_round_trips() {
        for state in Health::ALL {
            assert_eq!(Health::from_index(u8::try_from(state.index()).unwrap()), state);
        }
    }
}
