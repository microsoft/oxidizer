// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The route fields of a layer with a resolver: the resolver and the configured route parameters.

use std::sync::Arc;

use data_privacy::DataClass;
use observed::__private::{field_descriptor, log_field};
use observed::metadata::FieldDescriptor;
use routerama::RouteTable;

use super::interner::intern;
use crate::TelemetryLayer;

pub(crate) struct RouteFields {
    pub(crate) route_resolver: Arc<dyn RouteTable + Send + Sync>,
    pub(crate) parameters: Box<[RouteParameter]>,
}

impl RouteFields {
    pub(crate) fn new(layer: &TelemetryLayer) -> Option<Self> {
        Some(Self {
            route_resolver: layer.route_resolver.clone()?,
            parameters: layer
                .route_parameter_data_classes
                .iter()
                .map(|(name, data_class)| {
                    let key = intern(&format!("http.route.parameter.{name}"));
                    RouteParameter {
                        name: name.clone(),
                        descriptor: field_descriptor(key, Some(log_field(key)), None),
                        data_class: data_class.clone(),
                    }
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
