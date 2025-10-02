// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[derive(Clone)]
struct Something;

#[fundle::deps]
struct MyDeps {
    something: Something,
}

