// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An argument is scoped to `self`, so `self.` lands on its leftmost term whatever the expression
//! is built from. The root is therefore looked for through every form that keeps a term in that
//! position, and reported at the term itself rather than at the whole expression.

#[ohno::error]
#[display("bad: {}", cnt * 2)]
pub struct BinaryRootError {
    pub count: u32,
}

fn main() {}
