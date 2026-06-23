// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Backend: statically-allocated rows + a reading API (`DataAccess`).
//!
//! Mocked here with statically-allocated data, but the shape — iterate rows,
//! read each row's properties — matches a real storage or IPC source.

/// Tree shape: a forest of `ROOT_ROWS` roots, each `DEPTH` levels deep with
/// `FANOUT` children per node. Every node carries a name and a binary blob.
const ROOT_ROWS: usize = 12;
const FANOUT: usize = 2;
const DEPTH: usize = 2;
const BLOB_SIZE: usize = 256;

/// A single statically-allocated row of the backing data source.
struct Row {
    id: i64,
    name: &'static str,
    blob: &'static [u8],
    children: &'static [Self],
}

/// Reads the properties of a single [`Row`]. This is the only way the object
/// layer is allowed to touch backend data.
pub(crate) struct RowReader<'a> {
    row: &'a Row,
}

impl<'a> RowReader<'a> {
    #[must_use]
    pub(crate) fn id(&self) -> i64 {
        self.row.id
    }

    #[must_use]
    pub(crate) fn name(&self) -> &'a str {
        self.row.name
    }

    #[must_use]
    pub(crate) fn blob(&self) -> &'a [u8] {
        self.row.blob
    }

    /// A child [`DataAccess`] over the rows nested under this one.
    #[must_use]
    pub(crate) fn children(&self) -> DataAccess<'a> {
        DataAccess { rows: self.row.children }
    }
}

/// The backend handle over a set of rows.
#[derive(Clone, Copy)]
pub(crate) struct DataAccess<'a> {
    rows: &'a [Row],
}

impl<'a> DataAccess<'a> {
    /// Iterates the rows, yielding a [`RowReader`] for each.
    pub(crate) fn rows(&self) -> impl ExactSizeIterator<Item = RowReader<'a>> {
        self.rows.iter().map(|row| RowReader { row })
    }
}

/// Builds the mock dataset once and leaks it to obtain `'static` rows that
/// stand in for statically-allocated backend data.
#[must_use]
pub(crate) fn make_dataset() -> DataAccess<'static> {
    let mut next_id = 0;
    DataAccess {
        rows: make_rows(DEPTH, ROOT_ROWS, &mut next_id),
    }
}

fn make_rows(depth: usize, count: usize, next_id: &mut i64) -> &'static [Row] {
    let mut rows = Vec::with_capacity(count);
    for _ in 0..count {
        let id = *next_id;
        *next_id += 1;
        let children = if depth == 0 {
            &[][..]
        } else {
            make_rows(depth - 1, FANOUT, next_id)
        };
        rows.push(Row {
            id,
            name: "property-name",
            blob: &[0xABu8; BLOB_SIZE],
            children,
        });
    }
    Vec::leak(rows)
}
