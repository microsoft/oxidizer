// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Display, Formatter};

use crate::{Attribute, Health};

/// One publisher's current [`Health`] state and diagnostic attributes.
///
/// Signals are created by [`Publisher::publish`](crate::Publisher::publish) and inspected through
/// publishers and reports. Attribute order is not significant for equality. `Signal` does not
/// implement `Eq` or `Hash` because [`AttributeValue::Double`](crate::AttributeValue::Double) can
/// contain `NaN`.
///
/// # Examples
///
/// ```
/// # testing_aids::init_tracing!();
/// use oxidizer_health::{Aggregator, Health};
///
/// let aggregator = Aggregator::new();
/// let database = aggregator.component("database");
/// let mut publisher = database.publisher();
/// publisher.degraded("high latency");
///
/// let signal = publisher.signal();
/// assert_eq!(signal.state(), Health::Degraded);
/// assert_eq!(signal.to_string(), "Degraded (reason=high latency)");
/// ```
#[derive(Clone, Debug)]
pub struct Signal {
    state: Health,
    attributes: Box<[Attribute]>,
}

impl Signal {
    pub(crate) fn new(state: Health, attributes: Box<[Attribute]>) -> Self {
        Self { state, attributes }
    }

    pub(crate) fn healthy() -> Self {
        Self::new(Health::Healthy, Box::default())
    }

    /// The individual publisher's state, before component aggregation.
    #[must_use]
    pub const fn state(&self) -> Health {
        self.state
    }

    /// Attribute order does not affect signal equality.
    #[must_use]
    pub fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }
}

impl PartialEq for Signal {
    // Attributes compare as a multiset; signals carry few attributes, so the quadratic scan is
    // cheaper than allocating a sorted copy.
    fn eq(&self, other: &Self) -> bool {
        let count = |attributes: &[Attribute], attribute: &Attribute| attributes.iter().filter(|candidate| *candidate == attribute).count();

        self.state == other.state
            && self.attributes.len() == other.attributes.len()
            && self.attributes.iter().all(|attribute| {
                // An attribute that is not equal to itself (a NaN value) makes the signals unequal.
                let own = count(&self.attributes, attribute);
                own != 0 && own == count(&other.attributes, attribute)
            })
    }
}

impl Display for Signal {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.state, f)?;
        let mut attributes = self.attributes.iter();
        if let Some(first) = attributes.next() {
            write!(f, " ({first}")?;
            for attribute in attributes {
                write!(f, ", {attribute}")?;
            }
            f.write_str(")")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(attributes: impl IntoIterator<Item = (&'static str, i64)>) -> Signal {
        Signal::new(Health::Degraded, attributes.into_iter().map(Attribute::from).collect())
    }

    #[test]
    fn equality_ignores_attribute_order() {
        assert_eq!(signal([("a", 1), ("b", 2)]), signal([("b", 2), ("a", 1)]));
    }

    #[test]
    fn equality_respects_duplicate_counts() {
        assert_ne!(signal([("a", 1), ("a", 1), ("b", 2)]), signal([("a", 1), ("b", 2), ("b", 2)]));
    }

    #[test]
    fn equality_respects_state() {
        assert_ne!(Signal::healthy(), Signal::new(Health::Unhealthy, Box::default()));
    }

    #[test]
    fn nan_attributes_are_never_equal() {
        let nan = Signal::new(Health::Healthy, Box::new([Attribute::from(("ratio", f64::NAN))]));
        assert_ne!(nan, nan.clone());
    }

    #[test]
    fn display_lists_attributes() {
        assert_eq!(Signal::healthy().to_string(), "Healthy");
        assert_eq!(signal([("a", 1), ("b", 2)]).to_string(), "Degraded (a=1, b=2)");
    }
}
