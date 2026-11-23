// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use smallvec::{IntoIter, SmallVec};

/// A batch of items returned by a sampler.
///
/// Items are yielded in the order they were supplied.
#[derive(Debug)]
pub struct SamplingBatch<T> {
    items: IntoIter<[T; 1]>,
}

impl<T> SamplingBatch<T> {
    /// Creates an empty batch.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            items: SmallVec::new().into_iter(),
        }
    }

    /// Creates a batch containing one item.
    #[must_use]
    pub fn one(item: T) -> Self {
        Self {
            items: SmallVec::from_buf([item]).into_iter(),
        }
    }

    /// Creates a batch from the supplied items.
    #[must_use]
    pub fn many(items: impl IntoIterator<Item = T>) -> Self {
        let items: SmallVec<[T; 1]> = items.into_iter().collect();
        Self { items: items.into_iter() }
    }
}

impl<T> Iterator for SamplingBatch<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.items.next()
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::sampling::SamplingBatch;

    #[test]
    fn sampling_batch_yields_supplied_items() {
        let cases = [
            (SamplingBatch::empty(), vec![]),
            (SamplingBatch::one(7), vec![7]),
            (SamplingBatch::many([]), vec![]),
            (SamplingBatch::many([7]), vec![7]),
            (SamplingBatch::many([7, 3]), vec![7, 3]),
            (SamplingBatch::many([7, 3, 5]), vec![7, 3, 5]),
            (SamplingBatch::many([7, 3, 7]), vec![7, 3, 7]),
        ];

        for (batch, expected) in cases {
            let actual: Vec<_> = batch.collect();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn sampling_batch_releases_remaining_items() {
        let item = Rc::new(7);
        for count in 1..=3 {
            let mut batch = SamplingBatch::many((0..count).map(|_| Rc::clone(&item)));
            let returned = batch.next().unwrap();
            assert_eq!(*returned, 7);

            drop(batch);
            assert_eq!(Rc::strong_count(&item), 2);

            drop(returned);
            assert_eq!(Rc::strong_count(&item), 1);
        }
    }
}
