//! Public-controller refusal, victim-retention and same-peer retry diagnostic.
//! This deliberately exceeds ingress capacity and emits no throughput result.
use super::*;

pub(super) fn run(
    runtime: &tokio::runtime::Runtime,
    controller: &TxPoolController,
    diagnostics: &TerminalDiagnosticsGuard,
    warm: &[TransactionView],
    target: &Arc<Vec<TransactionView>>,
    cycles: &Arc<Vec<u64>>,
    peers: usize,
) -> BenchResult<()> {
    let completion = &diagnostics.completion;
    let relay = &diagnostics.relay;
    let expected_ok = &diagnostics.expected_relay;
    let live = || -> BenchResult<HashSet<Byte32>> {
        let ids = controller.get_all_ids().map_err(bench_error)?;
        Ok(ids.pending.into_iter().chain(ids.proposed).collect())
    };
    let victims: HashSet<_> = warm.iter().map(TransactionView::hash).collect();
    let targets: HashSet<_> = target.iter().map(TransactionView::hash).collect();
    require(live()? == victims, "warm accepted membership differs")?;
    completion.begin_target(Instant::now());
    // Warm work is complete and no target exists yet; suspension precedes
    // every target submission. The callback check below verifies that boundary.
    controller.suspend_chunk_process().map_err(bench_error)?;
    let ingress = runtime.block_on(submit_only(
        controller,
        Arc::clone(target),
        Arc::clone(cycles),
        &peer_ranges(target.len(), peers),
    ));
    let before_resume_accepted = completion.accepted_count();
    let ingress_refused = lock(&relay.rejects).clone();
    // Always release suspension even if the ingress transport failed.
    controller.continue_chunk_process().map_err(bench_error)?;
    ingress?;
    require(
        before_resume_accepted == warm.len(),
        "target computed while paused",
    )?;
    require(
        !ingress_refused.is_empty() && ingress_refused.is_subset(&targets),
        "pressure diagnostic requires target-only capacity refusal",
    )?;
    require(
        runtime.block_on(wait_for_stress_settlement(
            completion,
            relay,
            tokio::time::Instant::now() + Duration::from_secs(30),
        )),
        "pressure work did not settle after resume",
    )?;
    // Resolution can refuse additional candidates while the raw queue is full.
    // Derive the victim set only after the complete first-offer terminal union.
    let refused = lock(&relay.rejects)
        .intersection(&targets)
        .cloned()
        .collect::<HashSet<_>>();
    let expected_live: HashSet<_> = target
        .iter()
        .zip(warm)
        .map(|(replacement, victim)| {
            if refused.contains(&replacement.hash()) {
                victim.hash()
            } else {
                replacement.hash()
            }
        })
        .collect();
    require(
        live()? == expected_live,
        "a refused replacement displaced its victim or lost accepted membership",
    )?;
    // Check collection before retry can turn a refused candidate into an
    // accepted one in the generic failure snapshot.
    require(
        rejection_diagnostics::observation()["records"].as_u64() == Some(refused.len() as u64),
        "refusal diagnostic count differs from refused targets",
    )?;
    let before_retry = terminal_diagnostics(completion, relay, expected_ok);
    let total = warm.len() + target.len();
    let mut expected_accepted = total - refused.len();
    require(
        completion.accepted_count() == expected_accepted,
        "initial callback population differs",
    )?;
    let mut refused_per_peer = Vec::new();
    for (peer, (start, end)) in peer_ranges(target.len(), peers).into_iter().enumerate() {
        let retry: Vec<_> = (start..end)
            .filter(|&index| refused.contains(&target[index].hash()))
            .collect();
        refused_per_peer.push(retry.len());
        for chunk in retry.chunks(128) {
            let response = submit_remote_batch(
                controller,
                chunk
                    .iter()
                    .map(|&index| (target[index].clone(), cycles[index]))
                    .collect(),
                (peer + 1).into(),
            )?;
            runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(90), response).await
            })??;
            expected_accepted += chunk.len();
            runtime.block_on(completion.wait_for(expected_accepted))?;
        }
    }
    let mut expected_rejects = victims;
    expected_rejects.extend(refused.iter().cloned());
    runtime.block_on(relay.wait_for_terminals(expected_ok.len(), expected_rejects.len()))?;
    relay.validate(expected_ok, &expected_rejects, None)?;
    completion.validate(total, false)?;
    require(
        live()? == targets,
        "retry did not leave exactly the complete replacement set",
    )?;
    println!(
        "BENCH_RBF_PRESSURE {}",
        serde_json::json!({
            "schema": 1, "warm": warm.len(), "target": target.len(), "peers": peers,
            "target_callbacks_while_paused": before_resume_accepted - warm.len(),
            "refused": refused.len(), "ingress_refused": ingress_refused.len(),
            "after_resume_refused": refused.len() - ingress_refused.len(),
            "refused_per_peer": refused_per_peer,
            "all_refused_victims_retained": true, "all_retries_accepted": true,
            "retry_peer_identity_preserved": true, "final_live_replacements": targets.len(),
            "before_retry_terminals": before_retry,
            "final_terminals": terminal_diagnostics(completion, relay, expected_ok),
        })
    );
    Ok(())
}
