// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Using a raw `String` in a restricted template position (`{param}`) must fail
//! to compile because `String` does not implement `UriParam`.

use templated_uri::templated;

#[templated(template = "/users/{user_id}", bypass_redaction)]
#[derive(Clone)]
struct UserPath {
    user_id: String,
}

fn main() {}
