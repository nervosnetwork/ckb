use crate::block_status::BlockStatus;
use crate::synchronizer::Synchronizer;
use crate::types::SyncSnapshot;
use crate::MAX_HEADERS_LEN;
use ckb_error::{Error, ErrorKind};
use ckb_logger::{debug, log_enabled, warn, Level};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_traits::BlockMedianTimeContext;
use ckb_types::{
    bytes::Bytes,
    core::{self, BlockNumber, HeaderContext},
    packed::{self, Byte32},
    prelude::*,
};
use ckb_verification::{HeaderError, HeaderErrorKind, HeaderResolver, HeaderVerifier, Verifier};
use failure::Error as FailureError;

pub enum SendHeadersMessage<'a> {
    POW(packed::SendHeadersReader<'a>),
    POA(packed::SendPOAHeadersReader<'a>),
}

impl<'a> From<packed::SendHeadersReader<'a>> for SendHeadersMessage<'a> {
    fn from(reader: packed::SendHeadersReader<'a>) -> SendHeadersMessage<'a> {
        SendHeadersMessage::POW(reader)
    }
}

impl<'a> SendHeadersMessage<'a> {
    pub fn headers(&self) -> Vec<HeaderContext> {
        match self {
            Self::POW(message) => message
                .headers()
                .to_entity()
                .into_iter()
                .map(|header| HeaderContext::new(header.into_view()))
                .collect(),
            Self::POA(message) => message
                .headers()
                .to_entity()
                .into_iter()
                .map(|poa_header| {
                    HeaderContext::with_cellbase(
                        poa_header.header().into_view(),
                        poa_header.cellbase(),
                    )
                })
                .collect(),
        }
    }
}
