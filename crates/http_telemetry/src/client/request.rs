// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The values of one client request, and the events that end it.

use std::borrow::Cow;
use std::sync::Arc;

use http::uri::Authority;
use http::{Method, StatusCode, Version};
use http_extensions::HttpError;
use observed::{Sink, Text, emit};
use ohno::ErrorExt;
use templated_uri::PathAndQuery;

use super::events::{
    Dimensions, RequestAbandoned, RequestComplete, RequestError, RequestTotalAbandoned, RequestTotalComplete, RequestTotalError,
};
use super::layer::Client;
use crate::drop_guard::EmitEvents;
use crate::shared::{http_method_name, method_original, protocol_version};

pub(crate) enum RequestEnd<'a> {
    /// The handler returned a response with this status and version.
    Complete(StatusCode, Version),
    /// The handler returned this error.
    Error(&'a HttpError),
    /// The handler panicked.
    Panic,
    /// The caller dropped the request future before the handler returned.
    Abandoned,
}

/// The values of one client request that its events record.
///
/// The request ends when the handler returns, so the values can borrow the layer.
pub(crate) struct ClientRequest<'a> {
    pub(crate) sink: &'a Sink,
    pub(crate) client: &'a Client,
    pub(crate) method: Method,
    pub(crate) scheme: Text,
    pub(crate) authority: Option<Authority>,
    pub(crate) server_port: Option<u16>,
    pub(crate) url_template: Option<Cow<'static, str>>,
    pub(crate) path: Option<PathAndQuery>,
}

impl EmitEvents for ClientRequest<'_> {
    type RequestEnd<'a> = RequestEnd<'a>;

    const ABANDONED: RequestEnd<'static> = RequestEnd::Abandoned;

    #[expect(
        clippy::too_many_lines,
        reason = "a request can end in four ways, and the code for each way lists all fields of its attempt event and of its total event"
    )]
    fn emit_events(&self, end: RequestEnd<'_>, duration_secs: f64) {
        let method = http_method_name(&self.method);
        let server_address = || {
            self.authority
                .as_ref()
                .map(|authority| Text::from(Arc::<str>::from(authority.host())))
        };
        let url_template = || match &self.url_template {
            Some(Cow::Borrowed(template)) => Text::from(*template),
            Some(Cow::Owned(template)) => Text::from(Arc::<str>::from(template.as_str())),
            None => Text::from("unknown"),
        };
        let path = self.path.as_ref();
        let dimensions = || Dimensions(&self.client.dimensions);
        match end {
            RequestEnd::Complete(status_code, version) => {
                if self.client.report_total_duration {
                    emit!(
                        self.sink,
                        RequestTotalComplete {
                            method,
                            method_original: method_original(&self.method),
                            scheme: self.scheme.clone(),
                            protocol_name: "http",
                            protocol_version: protocol_version(version),
                            server_address: server_address(),
                            server_port: self.server_port,
                            url_template: url_template(),
                            path,
                            status_code: status_code.as_u16(),
                            dimensions: dimensions(),
                            duration_secs,
                        }
                    );
                } else {
                    emit!(
                        self.sink,
                        RequestComplete {
                            method,
                            method_original: method_original(&self.method),
                            scheme: self.scheme.clone(),
                            protocol_name: "http",
                            protocol_version: protocol_version(version),
                            server_address: server_address(),
                            server_port: self.server_port,
                            url_template: url_template(),
                            path,
                            status_code: status_code.as_u16(),
                            dimensions: dimensions(),
                            duration_secs,
                        }
                    );
                }
            }
            RequestEnd::Error(error) => {
                if self.client.report_total_duration {
                    emit!(
                        self.sink,
                        RequestTotalError {
                            method,
                            method_original: method_original(&self.method),
                            scheme: self.scheme.clone(),
                            server_address: server_address(),
                            server_port: self.server_port,
                            url_template: url_template(),
                            path,
                            error_type: Text::from((self.client.error_labels)(error).into_cow()),
                            exception_message: Some(Text::from(error.message())),
                            dimensions: dimensions(),
                            duration_secs,
                        }
                    );
                } else {
                    emit!(
                        self.sink,
                        RequestError {
                            method,
                            method_original: method_original(&self.method),
                            scheme: self.scheme.clone(),
                            server_address: server_address(),
                            server_port: self.server_port,
                            url_template: url_template(),
                            path,
                            error_type: Text::from((self.client.error_labels)(error).into_cow()),
                            exception_message: Some(Text::from(error.message())),
                            dimensions: dimensions(),
                            duration_secs,
                        }
                    );
                }
            }
            RequestEnd::Panic => {
                if self.client.report_total_duration {
                    emit!(
                        self.sink,
                        RequestTotalError {
                            method,
                            method_original: method_original(&self.method),
                            scheme: self.scheme.clone(),
                            server_address: server_address(),
                            server_port: self.server_port,
                            url_template: url_template(),
                            path,
                            error_type: Text::from("panic"),
                            exception_message: None,
                            dimensions: dimensions(),
                            duration_secs,
                        }
                    );
                } else {
                    emit!(
                        self.sink,
                        RequestError {
                            method,
                            method_original: method_original(&self.method),
                            scheme: self.scheme.clone(),
                            server_address: server_address(),
                            server_port: self.server_port,
                            url_template: url_template(),
                            path,
                            error_type: Text::from("panic"),
                            exception_message: None,
                            dimensions: dimensions(),
                            duration_secs,
                        }
                    );
                }
            }
            RequestEnd::Abandoned => {
                if self.client.report_total_duration {
                    emit!(
                        self.sink,
                        RequestTotalAbandoned {
                            method,
                            method_original: method_original(&self.method),
                            scheme: self.scheme.clone(),
                            server_address: server_address(),
                            server_port: self.server_port,
                            url_template: url_template(),
                            path,
                            error_type: "abandoned",
                            dimensions: dimensions(),
                            duration_secs,
                        }
                    );
                } else {
                    emit!(
                        self.sink,
                        RequestAbandoned {
                            method,
                            method_original: method_original(&self.method),
                            scheme: self.scheme.clone(),
                            server_address: server_address(),
                            server_port: self.server_port,
                            url_template: url_template(),
                            path,
                            error_type: "abandoned",
                            dimensions: dimensions(),
                            duration_secs,
                        }
                    );
                }
            }
        }
    }
}
