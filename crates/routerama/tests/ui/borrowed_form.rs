// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Bytes;
use http_body_util::Empty;
use routerama::query::FromQuery;
use routerama::route::form::Form;
use routerama::route::FromRequestBody;

#[derive(FromQuery)]
struct Borrowed<'form> {
    value: &'form str,
}

fn require_form_extractor<T>()
where
    Form<T, 64>: FromRequestBody<(), Empty<Bytes>>,
{
}

fn main() {
    require_form_extractor::<Borrowed<'static>>();
}
