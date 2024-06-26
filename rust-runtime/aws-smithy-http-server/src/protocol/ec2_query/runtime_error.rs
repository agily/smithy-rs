/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::body::BoxBody;
use crate::extension::RuntimeErrorExtension;
use crate::protocol::ec2_query::router::Error;
use crate::protocol::ec2_query::Ec2Query;
use crate::response::{IntoResponse, Response};
use crate::runtime_error::{InternalFailureException, INVALID_HTTP_RESPONSE_FOR_RUNTIME_ERROR_PANIC_MESSAGE};
use http::StatusCode;

use super::rejection::{RequestRejection, ResponseRejection};

#[derive(Debug)]
pub enum RuntimeError {
    Serialization(crate::Error),
    InternalFailure(crate::Error),
    NotAcceptable,
    UnsupportedMediaType,
    Validation(String),
}

impl RuntimeError {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Serialization(_) => "SerializationException",
            Self::InternalFailure(_) => "InternalFailureException",
            Self::NotAcceptable => "NotAcceptableException",
            Self::UnsupportedMediaType => "UnsupportedMediaTypeException",
            Self::Validation(_) => "ValidationException",
        }
    }

    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::Serialization(_) => StatusCode::BAD_REQUEST,
            Self::InternalFailure(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::NotAcceptable => StatusCode::NOT_ACCEPTABLE,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Validation(_) => StatusCode::BAD_REQUEST,
        }
    }
}

impl IntoResponse<Ec2Query> for Error {
    fn into_response(self) -> Response<BoxBody> {
        IntoResponse::<Ec2Query>::into_response(RuntimeError::InternalFailure(crate::Error::new(String::new())))
    }
}
//
impl IntoResponse<Ec2Query> for InternalFailureException {
    fn into_response(self) -> http::Response<crate::body::BoxBody> {
        IntoResponse::<Ec2Query>::into_response(RuntimeError::InternalFailure(crate::Error::new(String::new())))
    }
}
//
// impl IntoResponse<AwsJson1_1> for InternalFailureException {
//     fn into_response(self) -> http::Response<crate::body::BoxBody> {
//         IntoResponse::<AwsJson1_1>::into_response(RuntimeError::InternalFailure(crate::Error::new(String::new())))
//     }
// }

impl IntoResponse<Ec2Query> for RuntimeError {
    fn into_response(self) -> http::Response<crate::body::BoxBody> {
        let res = http::Response::builder()
            .status(self.status_code())
            .header("Content-Type", "application/x-amz-json-1.0")
            .extension(RuntimeErrorExtension::new(self.name().to_string()));

        let body = match self {
            RuntimeError::Validation(reason) => crate::body::to_boxed(reason),
            // See https://awslabs.github.io/smithy/2.0/aws/protocols/aws-json-1_0-protocol.html#empty-body-serialization
            _ => crate::body::to_boxed("{}"),
        };

        res.body(body)
            .expect(INVALID_HTTP_RESPONSE_FOR_RUNTIME_ERROR_PANIC_MESSAGE)
    }
}

impl From<ResponseRejection> for RuntimeError {
    fn from(err: ResponseRejection) -> Self {
        Self::Serialization(crate::Error::new(err))
    }
}

impl From<RequestRejection> for RuntimeError {
    fn from(err: RequestRejection) -> Self {
        match err {
            _ => Self::Serialization(crate::Error::new(err)),
        }
    }
}
