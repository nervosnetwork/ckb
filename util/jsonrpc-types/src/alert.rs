use crate::{bytes::JsonBytes, string, Timestamp};
use ckb_types::{packed, prelude::*};
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct AlertId(#[serde(with = "string")] pub u32);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct AlertPriority(#[serde(with = "string")] pub u32);

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Alert {
    pub id: AlertId,
    pub cancel: AlertId,
    pub min_version: Option<String>,
    pub max_version: Option<String>,
    pub priority: AlertPriority,
    pub notice_until: Timestamp,
    pub message: String,
    pub signatures: Vec<JsonBytes>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct AlertMessage {
    pub id: AlertId,
    pub priority: AlertPriority,
    pub notice_until: Timestamp,
    pub message: String,
}

impl From<Alert> for packed::Alert {
    fn from(json: Alert) -> Self {
        let Alert {
            id,
            cancel,
            min_version,
            max_version,
            priority,
            notice_until,
            message,
            signatures,
        } = json;
        let raw = packed::RawAlert::new_builder()
            .id(id.0.pack())
            .cancel(cancel.0.pack())
            .min_version(min_version.pack())
            .max_version(max_version.pack())
            .priority(priority.0.pack())
            .notice_until(notice_until.0.pack())
            .message(message.pack())
            .build();
        packed::Alert::new_builder()
            .raw(raw)
            .signatures(signatures.into_iter().map(Into::into).pack())
            .build()
    }
}

impl From<packed::Alert> for Alert {
    fn from(input: packed::Alert) -> Self {
        let raw = input.raw();
        Alert {
            id: AlertId(raw.id().unpack()),
            cancel: AlertId(raw.cancel().unpack()),
            min_version: raw
                .as_reader()
                .min_version()
                .to_opt()
                .map(|b| unsafe { b.as_utf8_unchecked() }.to_owned()),
            max_version: raw
                .as_reader()
                .max_version()
                .to_opt()
                .map(|b| unsafe { b.as_utf8_unchecked() }.to_owned()),
            priority: AlertPriority(raw.priority().unpack()),
            notice_until: Timestamp(raw.notice_until().unpack()),
            message: unsafe { raw.as_reader().message().as_utf8_unchecked() }.to_owned(),
            signatures: input.signatures().into_iter().map(Into::into).collect(),
        }
    }
}

impl From<packed::Alert> for AlertMessage {
    fn from(input: packed::Alert) -> Self {
        let raw = input.raw();
        AlertMessage {
            id: AlertId(raw.id().unpack()),
            priority: AlertPriority(raw.priority().unpack()),
            notice_until: Timestamp(raw.notice_until().unpack()),
            message: unsafe { raw.as_reader().message().as_utf8_unchecked() }.to_owned(),
        }
    }
}
