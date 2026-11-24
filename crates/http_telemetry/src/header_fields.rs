// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The header fields of both sides: the headers that a layer logs, their values in one request, and the fields of
//! the events that log them.

use std::fmt::{self, Write as _};
use std::ops::ControlFlow;
use std::sync::Arc;

use data_privacy::{DataClass, Redactor, Sensitive};
use http::{HeaderMap, HeaderName, HeaderValue};
use observed::metadata::FieldDescriptor;
use observed::processing::FieldVisitorFn;
use observed::{Value, VisitFields};
use smallvec::SmallVec;

use crate::interner::intern;

pub(crate) const REQUEST_HEADER_PREFIX: &str = "http.request.header.";
pub(crate) const RESPONSE_HEADER_PREFIX: &str = "http.response.header.";

#[derive(Debug)]
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

pub(crate) struct RequestAndResponseHeaders {
    pub(crate) request: Option<HeaderValues>,
    pub(crate) response: Option<HeaderValues>,
}

impl RequestAndResponseHeaders {
    pub(crate) fn new(
        request_fields: Option<&Arc<[HeaderField]>>,
        response_fields: Option<&Arc<[HeaderField]>>,
        request_headers: &HeaderMap,
    ) -> Option<Box<Self>> {
        (request_fields.is_some() || response_fields.is_some()).then(|| {
            Box::new(Self {
                request: request_fields.map(|fields| HeaderValues::new(Arc::clone(fields), request_headers)),
                response: None,
            })
        })
    }

    pub(crate) fn set_response(&mut self, response_fields: Option<&Arc<[HeaderField]>>, response_headers: &HeaderMap) {
        self.response = response_fields.map(|fields| HeaderValues::new(Arc::clone(fields), response_headers));
    }
}

pub(crate) struct HeaderValues {
    fields: Arc<[HeaderField]>,
    values: SmallVec<[(usize, HeaderValue); 2]>,
}

impl HeaderValues {
    fn new(fields: Arc<[HeaderField]>, headers: &HeaderMap) -> Self {
        let values = fields
            .iter()
            .enumerate()
            .flat_map(|(index, field)| headers.get_all(&field.header_name).iter().map(move |value| (index, value.clone())))
            .collect();
        Self { fields, values }
    }
}

pub(crate) struct Headers<'a>(pub(crate) Option<&'a HeaderValues>);

impl VisitFields for Headers<'_> {
    const CONTAINS_METRICS: bool = false;

    fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        if let Some(headers) = self.0 {
            // The values of one header are next to each other, so each chunk holds the values of one header.
            for values in headers.values.chunk_by(|left, right| left.0 == right.0) {
                let header_field = &headers.fields[values[0].0];
                let value = |redactor: &dyn Redactor| {
                    Value::from_redacted(&Sensitive::new(JoinedValues(values), header_field.data_class.clone()), redactor)
                };
                visitor(&header_field.descriptor, &value)?;
            }
        }
        ControlFlow::Continue(())
    }
}

struct JoinedValues<'a>(&'a [(usize, HeaderValue)]);

impl fmt::Display for JoinedValues<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut separator = "";
        for (_, value) in self.0 {
            f.write_str(separator)?;
            for chunk in value.as_bytes().utf8_chunks() {
                f.write_str(chunk.valid())?;
                if !chunk.invalid().is_empty() {
                    f.write_char(char::REPLACEMENT_CHARACTER)?;
                }
            }
            separator = ", ";
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RejectNonEmptyWrites;

    impl fmt::Write for RejectNonEmptyWrites {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            if s.is_empty() { Ok(()) } else { Err(fmt::Error) }
        }
    }

    #[test]
    fn joined_values_return_writer_errors() {
        let written = |values: &[&'static [u8]]| {
            let values: Vec<_> = values.iter().map(|value| (0, HeaderValue::from_bytes(value).unwrap())).collect();
            write!(RejectNonEmptyWrites, "{}", JoinedValues(&values))
        };

        assert_eq!([written(&[b"a"]), written(&[b"", b"a"]), written(&[b"\xff"])], [Err(fmt::Error); 3]);
    }
}
