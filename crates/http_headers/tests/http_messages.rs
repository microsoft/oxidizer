// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Direct `http::Request` and `http::Response` field-adapter contracts.

#![cfg(feature = "headers-all")]
#![cfg(feature = "http")]
#![expect(clippy::unwrap_used, reason = "test failures provide sufficient context")]

use http_headers::headers::{UserAgent, UserAgentOwned};
use http_headers::sink::{EncodedValues, FieldEncodeOutput, FieldEncoder, FieldSink, InsertError};
use http_headers::{FieldName, FieldSensitivity, FieldValue, FieldValueRef};

struct RejectEncoder;

impl FieldEncoder for RejectEncoder {
    fn encode<O>(self, _output: &mut O) -> Result<(), InsertError>
    where
        O: FieldEncodeOutput,
    {
        Err(InsertError)
    }
}

fn exercise(message: &mut impl FieldSink) {
    message
        .set_values(&FieldName::UserAgent, EncodedValues::single(FieldValue::from_static("client/1")))
        .unwrap();
    assert_eq!(UserAgent::view(message).unwrap().unwrap().as_bytes(), b"client/1");

    UserAgent::insert(message, UserAgentOwned::try_from_static("replacement").unwrap()).unwrap();
    message
        .append_values(&FieldName::UserAgent, EncodedValues::single(FieldValue::from_static("additional")))
        .unwrap();
    assert_eq!(
        message
            .lines(&FieldName::UserAgent)
            .unwrap()
            .repeated()
            .map(FieldValueRef::as_bytes)
            .collect::<Vec<_>>(),
        [b"replacement".as_slice(), b"additional".as_slice()]
    );

    message
        .set_encoded(
            &FieldName::Authorization,
            FieldValueRef::new(b"Basic dXNlcjpwYXNz").with_sensitivity(FieldSensitivity::Sensitive),
        )
        .unwrap();
    assert!(
        message
            .lines(&FieldName::Authorization)
            .unwrap()
            .exactly_one()
            .unwrap()
            .is_sensitive()
    );

    assert_eq!(message.set_encoded(&FieldName::UserAgent, RejectEncoder), Err(InsertError));
    assert_eq!(message.append_encoded(&FieldName::UserAgent, RejectEncoder), Err(InsertError));
    assert_eq!(
        message
            .lines(&FieldName::UserAgent)
            .unwrap()
            .repeated()
            .map(FieldValueRef::as_bytes)
            .collect::<Vec<_>>(),
        [b"replacement".as_slice(), b"additional".as_slice()]
    );

    message.remove_values(&FieldName::UserAgent);
    message.remove_values(&FieldName::Authorization);
    assert!(!message.contains(&FieldName::UserAgent));
    assert!(!message.contains(&FieldName::Authorization));
}

#[test]
fn request_and_response_delegate_to_their_header_maps() {
    exercise(&mut http::Request::new(()));
    exercise(&mut http::Response::new(()));
}
