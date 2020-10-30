use crate::ProtocolId;
use p2p::{
    builder::MetaBuilder,
    service::{BlockingFlag, ProtocolHandle, ProtocolMeta},
    traits::ServiceProtocol,
};
use tokio_util::codec::length_delimited;

/// TODO(doc): @driftluo
#[derive(Clone, Debug)]
pub enum SupportProtocols {
    /// TODO(doc): @driftluo
    Ping,
    /// TODO(doc): @driftluo
    Discovery,
    /// TODO(doc): @driftluo
    Identify,
    /// TODO(doc): @driftluo
    Feeler,
    /// TODO(doc): @driftluo
    DisconnectMessage,
    /// TODO(doc): @driftluo
    Sync,
    /// TODO(doc): @driftluo
    Relay,
    /// TODO(doc): @driftluo
    Time,
    /// TODO(doc): @driftluo
    Alert,
}

impl SupportProtocols {
    /// TODO(doc): @driftluo
    pub fn protocol_id(&self) -> ProtocolId {
        match self {
            SupportProtocols::Ping => 0,
            SupportProtocols::Discovery => 1,
            SupportProtocols::Identify => 2,
            SupportProtocols::Feeler => 3,
            SupportProtocols::DisconnectMessage => 4,
            SupportProtocols::Sync => 100,
            SupportProtocols::Relay => 101,
            SupportProtocols::Time => 102,
            SupportProtocols::Alert => 110,
        }
        .into()
    }

    /// TODO(doc): @driftluo
    pub fn name(&self) -> String {
        match self {
            SupportProtocols::Ping => "/ckb/ping",
            SupportProtocols::Discovery => "/ckb/discovery",
            SupportProtocols::Identify => "/ckb/identify",
            SupportProtocols::Feeler => "/ckb/flr",
            SupportProtocols::DisconnectMessage => "/ckb/disconnectmsg",
            SupportProtocols::Sync => "/ckb/syn",
            SupportProtocols::Relay => "/ckb/rel",
            SupportProtocols::Time => "/ckb/tim",
            SupportProtocols::Alert => "/ckb/alt",
        }
        .to_owned()
    }

    /// TODO(doc): @driftluo
    pub fn support_versions(&self) -> Vec<String> {
        // we didn't invoke MetaBuilder#support_versions fn for these protocols (Ping/Discovery/Identify/Feeler/DisconnectMessage)
        // in previous code, so the default 0.0.1 value is used ( https://github.com/nervosnetwork/tentacle/blob/master/src/builder.rs#L312 )
        // have to keep 0.0.1 for compatibility...
        match self {
            SupportProtocols::Ping => vec!["0.0.1".to_owned()],
            SupportProtocols::Discovery => vec!["0.0.1".to_owned()],
            SupportProtocols::Identify => vec!["0.0.1".to_owned()],
            SupportProtocols::Feeler => vec!["0.0.1".to_owned()],
            SupportProtocols::DisconnectMessage => vec!["0.0.1".to_owned()],
            SupportProtocols::Sync => vec!["1".to_owned()],
            SupportProtocols::Relay => vec!["1".to_owned()],
            SupportProtocols::Time => vec!["1".to_owned()],
            SupportProtocols::Alert => vec!["1".to_owned()],
        }
    }

    /// TODO(doc): @driftluo
    pub fn max_frame_length(&self) -> usize {
        match self {
            SupportProtocols::Ping => 1024,              // 1   KB
            SupportProtocols::Discovery => 512 * 1024,   // 512 KB
            SupportProtocols::Identify => 2 * 1024,      // 2   KB
            SupportProtocols::Feeler => 1024,            // 1   KB
            SupportProtocols::DisconnectMessage => 1024, // 1   KB
            SupportProtocols::Sync => 2 * 1024 * 1024,   // 2   MB
            SupportProtocols::Relay => 4 * 1024 * 1024,  // 4   MB
            SupportProtocols::Time => 1024,              // 1   KB
            SupportProtocols::Alert => 128 * 1024,       // 128 KB
        }
    }

    /// TODO(doc): @driftluo
    pub fn flag(&self) -> BlockingFlag {
        match self {
            SupportProtocols::Ping
            | SupportProtocols::Discovery
            | SupportProtocols::Identify
            | SupportProtocols::Feeler
            | SupportProtocols::DisconnectMessage
            | SupportProtocols::Time
            | SupportProtocols::Alert => {
                let mut no_blocking_flag = BlockingFlag::default();
                no_blocking_flag.disable_all();
                no_blocking_flag
            }
            SupportProtocols::Sync | SupportProtocols::Relay => {
                let mut blocking_recv_flag = BlockingFlag::default();
                blocking_recv_flag.disable_connected();
                blocking_recv_flag.disable_disconnected();
                blocking_recv_flag.disable_notify();
                blocking_recv_flag
            }
        }
    }

    /// TODO(doc): @driftluo
    // a helper fn to build `ProtocolMeta`
    pub fn build_meta_with_service_handle<
        SH: FnOnce() -> ProtocolHandle<Box<dyn ServiceProtocol + Send + 'static + Unpin>>,
    >(
        self,
        service_handle: SH,
    ) -> ProtocolMeta {
        let meta_builder: MetaBuilder = self.into();
        meta_builder.service_handle(service_handle).build()
    }
}

impl Into<MetaBuilder> for SupportProtocols {
    fn into(self) -> MetaBuilder {
        let max_frame_length = self.max_frame_length();
        MetaBuilder::default()
            .id(self.protocol_id())
            .support_versions(self.support_versions())
            .flag(self.flag())
            .name(move |_| self.name())
            .codec(move || {
                Box::new(
                    length_delimited::Builder::new()
                        .max_frame_length(max_frame_length)
                        .new_codec(),
                )
            })
    }
}
