// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use data_privacy::DataClass;
use http::{HeaderMap, HeaderName};
use observed::metadata::FieldDescriptor;

use super::interner::intern;

pub(crate) struct HeaderField {
    pub(crate) header_name: HeaderName,
    pub(crate) descriptor: FieldDescriptor,
    pub(crate) data_class: DataClass,
}

pub(crate) fn header_fields(header_dataclass_map: &HeaderMap<DataClass>, prefix: &str) -> Option<Arc<[HeaderField]>> {
    // #[gamma::skip(cond.always_false, tag = "equivalent", reason = "An empty map logs no header either way; the check only saves the header box of each request.")]
    if header_dataclass_map.is_empty() {
        return None;
    }
    Some(
        header_dataclass_map
            .iter()
            .map(|(header_name, data_class)| HeaderField {
                header_name: header_name.clone(),
                descriptor: FieldDescriptor::log_only(intern(&format!("{prefix}{header_name}"))),
                data_class: data_class.clone(),
            })
            .collect(),
    )
}
