use super::*;

impl TxPoolService {
    pub(crate) async fn ban_malformed(&self, peer: PeerIndex, reason: String) {
        const DEFAULT_BAN_TIME: Duration = Duration::from_secs(MALFORMED_TX_BAN_SECONDS);

        let ban_permit = self
            .reserve_required_effects(
                crate::service::effects::EFFECT_ENVELOPE_BYTES.saturating_add(reason.len()),
                "peer-ban effect reservation failed",
            )
            .await;

        Self::report_malformed_peer_ban(peer, &reason);
        self.record_peer_ban(peer, DEFAULT_BAN_TIME);
        self.publish_required_reserved_effects(
            ban_permit,
            vec![TxPoolEffect::BanPeer {
                peer,
                duration: DEFAULT_BAN_TIME,
                reason,
            }],
            "reserved peer-ban journal failed",
        );
        self.remove_banned_peer_entries(peer).await;
    }
}
