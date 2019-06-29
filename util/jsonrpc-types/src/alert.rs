use crate::{bytes::JsonBytes, string, Timestamp};
use ckb_core::alert::{Alert as CoreAlert, AlertBuilder};
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

impl From<Alert> for CoreAlert {
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

        AlertBuilder::default()
            .id(id.0)
            .cancel(cancel.0)
            .min_version(min_version)
            .max_version(max_version)
            .priority(priority.0)
            .notice_until(notice_until.0)
            .message(message)
            .signatures(
                signatures
                    .into_iter()
                    .map(JsonBytes::into_bytes)
                    .collect::<Vec<_>>(),
            )
            .build()
    }
}

impl From<CoreAlert> for Alert {
    fn from(core: CoreAlert) -> Self {
        let CoreAlert {
            id,
            cancel,
            min_version,
            max_version,
            priority,
            notice_until,
            message,
            signatures,
        } = core;
        Alert {
            id: AlertId(id),
            cancel: AlertId(cancel),
            min_version,
            max_version,
            priority: AlertPriority(priority),
            notice_until: Timestamp(notice_until),
            message,
            signatures: signatures.into_iter().map(JsonBytes::from_bytes).collect(),
        }
    }
}

impl From<&CoreAlert> for AlertMessage {
    fn from(core: &CoreAlert) -> Self {
        let CoreAlert {
            id,
            priority,
            notice_until,
            message,
            ..
        } = core;
        AlertMessage {
            id: AlertId(*id),
            priority: AlertPriority(*priority),
            notice_until: Timestamp(*notice_until),
            message: message.to_owned(),
        }
    }
}
