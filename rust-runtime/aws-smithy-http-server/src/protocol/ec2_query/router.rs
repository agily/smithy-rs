#![allow(non_snake_case)]

/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

use bytes::Bytes;
use futures_util::{StreamExt, TryStream, TryStreamExt};
use heck::ToLowerCamelCase;
use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tower::Layer;
use tower::Service;

use crate::body::{empty, BoxBody, HttpBody};
use crate::routing::tiny_map::TinyMap;
use crate::routing::Router;
use crate::routing::{method_disallowed, Route, UNKNOWN_OPERATION_EXCEPTION};

use http::header::ToStrError;
use http::Request;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;
// use http_body::Body as _;
// use http_body::Body;
use crate::extension::RuntimeErrorExtension;
use crate::protocol::ec2_query::Ec2Query;
use crate::protocol::rest;
use crate::response::IntoResponse;
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;
use tokio::runtime::Handle;
use tracing::instrument::WithSubscriber;
use url::form_urlencoded;

/// An AWS JSON routing error.
#[derive(Debug, Error)]
pub enum Error {
    /// Relative URI was not "/".
    #[error("relative URI is not \"/\"")]
    NotRootUrl,
    /// Method was not `POST`.
    #[error("method not POST")]
    MethodNotAllowed,
    /// Missing the `x-amz-target` header.
    #[error("missing the \"x-amz-target\" header")]
    MissingHeader,
    /// Unable to parse header into UTF-8.
    #[error("failed to parse header: {0}")]
    InvalidHeader(ToStrError),
    /// Operation not found.
    #[error("operation not found")]
    NotFound,
}

// This constant determines when the `TinyMap` implementation switches from being a `Vec` to a
// `HashMap`. This is chosen to be 15 as a result of the discussion around
// https://github.com/smithy-lang/smithy-rs/pull/1429#issuecomment-1147516546
const ROUTE_CUTOFF: usize = 15;

/// A [`Router`] supporting [`AWS JSON 1.0`] and [`AWS JSON 1.1`] protocols.
///
/// [AWS JSON 1.0]: https://smithy.io/2.0/aws/protocols/aws-json-1_0-protocol.html
/// [AWS JSON 1.1]: https://smithy.io/2.0/aws/protocols/aws-json-1_1-protocol.html
#[derive(Debug, Clone)]
pub struct Ec2QueryRouter<S> {
    routes: TinyMap<String, S, ROUTE_CUTOFF>,
}

impl<S> Ec2QueryRouter<S> {
    /// Applies a [`Layer`] uniformly to all routes.
    pub fn layer<L>(self, layer: L) -> Ec2QueryRouter<L::Service>
    where
        L: Layer<S>,
    {
        Ec2QueryRouter {
            routes: self
                .routes
                .into_iter()
                .map(|(key, route)| (key, layer.layer(route)))
                .collect(),
        }
    }

    /// Applies type erasure to the inner route using [`Route::new`].
    pub fn boxed<B>(self) -> Ec2QueryRouter<Route<B>>
    where
        S: Service<http::Request<B>, Response = http::Response<BoxBody>, Error = Infallible>,
        S: Send + Clone + 'static,
        S::Future: Send + 'static,
    {
        Ec2QueryRouter {
            routes: self.routes.into_iter().map(|(key, s)| (key, Route::new(s))).collect(),
        }
    }
}

fn map_to_xml(map: &serde_json::Map<String, Value>) -> String {
    let mut writer = Writer::new(Vec::new());

    let action_name = map.get("Action").and_then(Value::as_str).unwrap_or("Response");
    let root_name = format!("{}Response", action_name);

    // Start root element
    writer
        .write_event(Event::Start(BytesStart::from_content(
            root_name.as_str(),
            root_name.len(),
        )))
        .unwrap();

    for (key, value) in map.iter() {
        if key != "Action" {
            let key_transformed = if key == "Filter" || key == "InstanceId" {
                key.to_string()
            } else {
                key.to_lower_camel_case()
            };
            append_xml_element(&mut writer, &key_transformed, value, "");
        }
    }

    // End root element
    writer.write_event(Event::End(BytesEnd::new(root_name))).unwrap();

    String::from_utf8(writer.into_inner()).unwrap()
}

fn append_xml_element(writer: &mut Writer<Vec<u8>>, key: &str, value: &Value, parent: &str) {
    if key.parse::<i32>().is_ok() {
        // append_xml_element(writer, parent, value, parent);
        // return;
        match parent {
            "Filter" => append_xml_element(writer, "Filter", value, "Filter"),
            "Value" => append_xml_element(writer, "item", value, "Value"),
            _ => {}
        }
        return;
    }
    match value {
        Value::Object(map) => {
            writer
                .write_event(Event::Start(BytesStart::from_content(key, key.len())))
                .unwrap();
            for (k, v) in map.iter() {
                let transformed_key = if key == "InstanceId" {
                    "InstanceId".to_string()
                } else if k == "Name" || k == "Value" {
                    k.to_string()
                } else {
                    k.to_lower_camel_case()
                };
                append_xml_element(writer, &transformed_key, v, key);
            }
            writer.write_event(Event::End(BytesEnd::new(key))).unwrap();
        }
        Value::Array(arr) => {
            let array_key = if key.starts_with("Filter") { "Filter" } else { key };
            for v in arr {
                append_xml_element(writer, array_key, v, key);
            }
        }
        Value::String(s) => {
            writer
                .write_event(Event::Start(BytesStart::from_content(key, key.len())))
                .unwrap();
            writer.write_event(Event::Text(BytesText::new(s))).unwrap();
            writer.write_event(Event::End(BytesEnd::new(key))).unwrap();
        }
        _ => {
            // Handle other types if needed
        }
    }
}

struct QueryParser<'a> {
    query: &'a str,
}

impl<'a> QueryParser<'a> {
    pub fn new(query: &'a str) -> Self {
        QueryParser { query }
    }

    pub fn parse(&self) -> serde_json::Map<String, Value> {
        let mut map: serde_json::Map<String, Value> = serde_json::Map::new();

        let pairs = self.query.split('&');
        for pair in pairs {
            let mut split = pair.splitn(2, '=');
            if let (Some(key), Some(value)) = (split.next(), split.next()) {
                let decoded_key = urlencoding::decode(key).expect("Failed to decode key").to_string();
                let decoded_value = urlencoding::decode(value).expect("Failed to decode value").to_string();

                self.insert_into_map(&mut map, decoded_key, decoded_value);
            }
        }

        map
    }

    fn insert_into_map(&self, map: &mut serde_json::Map<String, Value>, key: String, value: String) {
        let parts: Vec<&str> = key.split('.').collect();
        let mut current_map = map;

        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                current_map.insert(part.to_string(), Value::String(value.clone()));
            } else {
                current_map = current_map
                    .entry(part.to_string())
                    .or_insert_with(|| Value::Object(serde_json::Map::new()))
                    .as_object_mut()
                    .unwrap();
            }
        }
    }
}

impl<B, S> Router<B> for Ec2QueryRouter<S>
where
    S: Clone,
    B: Default + Debug + HttpBody + std::marker::Unpin,
    hyper::Body: From<B>,
    B: From<Bytes>,
{
    type Service = S;
    type Error = Error;

    async fn match_route(&self, request: &mut http::Request<B>) -> Result<S, Self::Error> {
        // The URI must be root,
        if request.uri() != "/" {
            return Err(Error::NotRootUrl);
        }

        // Only `Method::POST` is allowed.
        if request.method() != http::Method::POST {
            return Err(Error::MethodNotAllowed);
        }

        let s = hyper::body::to_bytes(request.body_mut())
            .await
            .map_err(|_| Error::NotFound)?;

        let target = String::from_utf8_lossy(&s)
            .split("&")
            .next()
            .unwrap()
            .replace("Action=", "");
        let q = String::from_utf8_lossy(&s);
        let parser = QueryParser::new(q.as_ref());
        let parsed_query = parser.parse();

        let xml_string = map_to_xml(&parsed_query);

        let new_data = Bytes::from(xml_string);

        let mut t = Request::builder().body(B::from(new_data)).unwrap();

        std::mem::swap(request, &mut t);
        // Lookup in the `TinyMap` for a route for the target.
        let route = self.routes.get(&format!("Ec2.{target}")).ok_or(Error::NotFound)?;

        Ok(route.clone())
    }
}

impl<S> FromIterator<(String, S)> for Ec2QueryRouter<S> {
    #[inline]
    fn from_iter<T: IntoIterator<Item = (String, S)>>(iter: T) -> Self {
        Self {
            routes: iter.into_iter().collect(),
        }
    }
}

impl IntoResponse<Ec2Query> for rest::router::Error {
    fn into_response(self) -> http::Response<BoxBody> {
        match self {
            crate::protocol::rest::router::Error::MethodNotAllowed => method_disallowed(),
            _ => http::Response::builder()
                .status(http::StatusCode::NOT_FOUND)
                .header(http::header::CONTENT_TYPE, "application/x-amz-json-1.1")
                .extension(RuntimeErrorExtension::new(
                    UNKNOWN_OPERATION_EXCEPTION.to_string(),
                ))
                .body(empty())
                .expect("invalid HTTP response for AWS JSON 1.1 routing error; please file a bug report under https://github.com/smithy-lang/smithy-rs/issues"),
        }
    }
}
