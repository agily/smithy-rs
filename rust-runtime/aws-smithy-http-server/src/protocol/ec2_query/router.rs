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
// const ROUTE_CUTOFF: usize = 15;

/// A [`Router`] supporting [`AWS JSON 1.0`] and [`AWS JSON 1.1`] protocols.
///
/// [AWS JSON 1.0]: https://smithy.io/2.0/aws/protocols/aws-json-1_0-protocol.html
/// [AWS JSON 1.1]: https://smithy.io/2.0/aws/protocols/aws-json-1_1-protocol.html
#[derive(Debug, Clone)]
pub struct Ec2QueryRouter<S> {
    pub routes: HashMap<String, S>,
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
        let header = request.headers();
        let target = String::from_utf8_lossy(&s)
            .split("&")
            .next()
            .unwrap()
            .replace("Action=", "");
        let q = String::from_utf8_lossy(&s);

        let new_data = Bytes::from(q.to_string());
        
        let mut t = Request::builder();

        for (name, value) in header {
            t = t.header(name, value);
        }
        
        let mut t = t.body(B::from(new_data)).unwrap();
        
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
