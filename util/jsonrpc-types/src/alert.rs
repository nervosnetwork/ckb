use crate::{bytes::JsonBytes, Timestamp, Uint32};
use ckb_types::{packed, prelude::*};
use serde::{Deserialize, Serialize};

/// The alert identifier that is used to filter duplicated alerts.
///
/// This is a 32-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint32](type.Uint32.html#examples).
pub type AlertId = Uint32;
/// Alerts are sorted by priority. Greater integers mean higher priorities.
///
/// This is a 32-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint32](type.Uint32.html#examples).
pub type AlertPriority = Uint32;

/// An alert is a message about critical problems to be broadcast to all nodes via the p2p network.
///
/// ## Examples
///
/// An example in JSON
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::Alert>(r#"
/// {
///   "id": "0x1",
///   "cancel": "0x0",
///   "min_version": "0.1.0",
///   "max_version": "1.0.0",
///   "priority": "0x1",
///   "message": "An example alert message!",
///   "notice_until": "0x24bcca57c00",
///   "signatures": [
///     "0xbd07059aa9a3d057da294c2c4d96fa1e67eeb089837c87b523f124239e18e9fc7d11bb95b720478f7f937d073517d0e4eb9a91d12da5c88a05f750362f4c214dd0",
///     "0x0242ef40bb64fe3189284de91f981b17f4d740c5e24a3fc9b70059db6aa1d198a2e76da4f84ab37549880d116860976e0cf81cd039563c452412076ebffa2e4453"
///   ]
/// }
/// # "#).unwrap();
/// ```
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Alert {
    /// The identifier of the alert. Clients use id to filter duplicated alerts.
    pub id: AlertId,
    /// Cancel a previous sent alert.
    pub cancel: AlertId,
    /// Optionally set the minimal version of the target clients.
    ///
    /// See [Semantic Version](https://semver.org/) about how to specify a version.
    pub min_version: Option<String>,
    /// Optionally set the maximal version of the target clients.
    ///
    /// See [Semantic Version](https://semver.org/) about how to specify a version.
    pub max_version: Option<String>,
    /// Alerts are sorted by priority, highest first.
    pub priority: AlertPriority,
    /// The alert is expired after this timestamp.
    pub notice_until: Timestamp,
    /// Alert message.
    pub message: String,
    /// The list of required signatures.
    pub signatures: Vec<JsonBytes>,
}

/// An alert sent by RPC `send_alert`.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct AlertMessage {
    /// The unique alert ID.
    pub id: AlertId,
    /// Alerts are sorted by priority, highest first.
    pub priority: AlertPriority,
    /// The alert is expired after this timestamp.
    pub notice_until: Timestamp,
    /// Alert message.
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
            .id(id.pack())
            .cancel(cancel.pack())
            .min_version(min_version.pack())
            .max_version(max_version.pack())
            .priority(priority.pack())
            .notice_until(notice_until.pack())
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
            id: raw.id().unpack(),
            cancel: raw.cancel().unpack(),
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
            priority: raw.priority().unpack(),
            notice_until: raw.notice_until().unpack(),
            message: unsafe { raw.as_reader().message().as_utf8_unchecked() }.to_owned(),
            signatures: input.signatures().into_iter().map(Into::into).collect(),
        }
    }
}

impl From<packed::Alert> for AlertMessage {
    fn from(input: packed::Alert) -> Self {
        let raw = input.raw();
        AlertMessage {
            id: raw.id().unpack(),
            priority: raw.priority().unpack(),
            notice_until: raw.notice_until().unpack(),
            message: unsafe { raw.as_reader().message().as_utf8_unchecked() }.to_owned(),
        }
    }
}
