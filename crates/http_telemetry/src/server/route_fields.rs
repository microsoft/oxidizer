// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The route fields of a layer with a resolver: the resolver and the configured route parameters.

use std::sync::Arc;

use data_privacy::DataClass;
use observed::metadata::FieldDescriptor;
use routerama::RouteTable;

use super::layer::Server;
use crate::interner::intern;

pub(crate) struct RouteFields {
    pub(crate) route_resolver: Arc<dyn RouteTable + Send + Sync>,
    pub(crate) parameters: Box<[RouteParameter]>,
}

impl RouteFields {
    pub(crate) fn new(server: &Server) -> Option<Self> {
        Some(Self {
            route_resolver: server.route_resolver.clone()?,
            parameters: server
                .route_parameter_data_classes
                .iter()
                .map(|(name, data_class)| RouteParameter {
                    name: name.clone(),
                    descriptor: FieldDescriptor::log_only(intern(&format!("http.route.parameter.{name}"))),
                    data_class: data_class.clone(),
                })
                .collect(),
        })
    }
}

pub(crate) struct RouteParameter {
    pub(crate) name: String,
    pub(crate) descriptor: FieldDescriptor,
    pub(crate) data_class: DataClass,
}
