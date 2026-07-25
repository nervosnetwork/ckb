use super::*;

impl TxPoolService {
    pub(crate) async fn ban_malformed(&self, peer: PeerIndex, reason: String) {
        const DEFAULT_BAN_TIME: Duration = Duration::from_secs(MALFORMED_TX_BAN_SECONDS);

        Self::report_malformed_peer_ban(peer, &reason);
        self.record_peer_ban(peer, DEFAULT_BAN_TIME);
        self.publish_effects_class(
            vec![TxPoolEffect::BanPeer {
                peer,
                duration: DEFAULT_BAN_TIME,
                reason,
            }],
            crate::service::effects::EffectClass::Trusted,
        )
        .await;
        self.remove_banned_peer_entries(peer).await;
    }
}
