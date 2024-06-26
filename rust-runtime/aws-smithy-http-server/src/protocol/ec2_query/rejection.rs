/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::rejection::MissingContentTypeReason;
use aws_smithy_runtime_api::http::HttpError;
use aws_smithy_xml::decode::XmlDecodeError;
use thiserror::Error;
#[derive(Debug, Error)]
pub enum ResponseRejection {
    #[error("error building HTTP response: {0}")]
    HttpBuild(#[from] http::Error),
}

#[derive(Debug, Error)]
pub enum RequestRejection {
    #[error("error converting non-streaming body to bytes: {0}")]
    BufferHttpBodyBytes(crate::Error),
    #[error("request contains invalid value for `Accept` header")]
    NotAcceptable,
}

impl From<std::convert::Infallible> for RequestRejection {
    fn from(_err: std::convert::Infallible) -> Self {
        match _err {}
    }
}

impl From<MissingContentTypeReason> for RequestRejection {
    fn from(_err: MissingContentTypeReason) -> Self {
        Self::NotAcceptable
    }
}

impl From<HttpError> for RequestRejection {
    fn from(_value: HttpError) -> Self {
        Self::NotAcceptable
    }
}
impl From<XmlDecodeError> for RequestRejection {
    fn from(_value: XmlDecodeError) -> Self {
        Self::NotAcceptable
    }
}

impl From<()> for RequestRejection {
    fn from(_value: ()) -> Self {
        Self::NotAcceptable
    }
}

convert_to_request_rejection!(hyper::Error, BufferHttpBodyBytes);
convert_to_request_rejection!(Box<dyn std::error::Error + Send + Sync + 'static>, BufferHttpBodyBytes);
