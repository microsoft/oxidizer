// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The buffer of a logged request body, which the client and the server middleware both use.

use std::sync::Arc;

use http_body::Body;
use http_body_util::BodyExt;
use http_extensions::HttpRequest;
use http_extensions::body::{Content, StreamingBody};
use observed::Text;
use parking_lot::Mutex;

use crate::body_buffer::{BodyBuffer, body_text};

/// The buffer of a request body.
pub(crate) enum RequestBodyBuffer {
    /// The layer does not log the body of this request.
    NotLogged,
    /// The text of a body that was in memory, made when the request started.
    InMemory(Text),
    /// The buffer of a body that streams, which the handler fills as it reads the body.
    Streaming(Arc<Mutex<BodyBuffer>>),
}

impl RequestBodyBuffer {
    /// Copies up to `size_limit` bytes of the body of `request` into a new buffer.
    pub(crate) fn new(request: &mut HttpRequest, size_limit: usize) -> Self {
        let mut buffer = Self::InMemory(Text::from(""));
        if request.body().is_end_stream() {
            return buffer;
        }
        let body = std::mem::take(request.body_mut());
        *request.body_mut() = body
            .map(|content| {
                Ok(match content {
                    Content::Bytes(bytes) => {
                        buffer = Self::InMemory(body_text(&bytes, size_limit));
                        Content::Bytes(bytes)
                    }
                    Content::Streaming(inner) => {
                        let body_buffer = Arc::new(Mutex::new(BodyBuffer::new(size_limit)));
                        buffer = Self::Streaming(Arc::clone(&body_buffer));
                        Content::Streaming(StreamingBody::new(inner.inspect_frame(move |frame| {
                            if let Some(data) = frame.data_ref() {
                                body_buffer.lock().push(data);
                            }
                        })))
                    }
                })
            })
            .expect("map fails only for a consumed body, which is at its end, so the check above rules it out");
        buffer
    }

    pub(crate) fn text(&self) -> Option<Text> {
        match self {
            Self::NotLogged => None,
            Self::InMemory(text) => Some(text.clone()),
            Self::Streaming(buffer) => Some(buffer.lock().text()),
        }
    }
}
