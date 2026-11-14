// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The route fields of a layer with a resolver: the template and the configured parameters of each route.

use std::sync::Arc;

use data_privacy::DataClass;
use observed::Text;
use observed::metadata::FieldDescriptor;
use routerama::RouteTable;

use super::interner::intern;
use crate::TelemetryLayer;

pub(crate) struct RouteFields {
    pub(crate) route_table: Arc<dyn RouteTable + Send + Sync>,
    pub(crate) routes: Box<[Option<Route>]>,
}

impl RouteFields {
    pub(crate) fn new(layer: &TelemetryLayer) -> Option<Self> {
        let route_table = layer.route_resolver.clone()?;
        let routes = (0..route_table.route_count())
            .map(|route| {
                let parameters = route_table.parameters(route)?.iter().map(|name| {
                    Some(RouteParameter {
                        descriptor: FieldDescriptor::log_only(intern(&format!("http.route.parameter.{name}"))),
                        data_class: layer.route_parameter_data_classes.get(name.as_ref())?.clone(),
                    })
                });
                Some(Route {
                    template: Text::from(Arc::<str>::from(route_table.template(route)?)),
                    parameters: parameters.collect(),
                })
            })
            .collect();
        Some(Self { route_table, routes })
    }
}

pub(crate) struct Route {
    pub(crate) template: Text,
    pub(crate) parameters: Box<[Option<RouteParameter>]>,
}

pub(crate) struct RouteParameter {
    pub(crate) descriptor: FieldDescriptor,
    pub(crate) data_class: DataClass,
}
