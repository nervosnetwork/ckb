use super::time_context::{
    ModelAcceptedValidity, ModelContextSensitivity, ModelPoolPhase, ModelProposalTimeObservation,
    ModelProposalTimeRelation, ModelTimePoint, ModelTimeRequirement, model_accepted_validity,
    model_context_sensitivity, model_phase_owned_environment_observation,
    model_proposal_time_observation,
};

fn requirements() -> Vec<ModelTimeRequirement> {
    (0..=2)
        .flat_map(|min_block| {
            (0..=2).flat_map(move |min_epoch_tick| {
                (0..=2).map(move |min_median_time| ModelTimeRequirement {
                    min_block,
                    min_epoch_tick,
                    min_median_time,
                })
            })
        })
        .collect()
}

#[test]
fn model_time_requirement_is_the_least_coordinatewise_join() {
    let requirements = requirements();
    for left in &requirements {
        for right in &requirements {
            let joined = left.join(*right);
            assert_eq!(joined, right.join(*left));
            assert_eq!(joined.join(*left), joined);
            for point in &requirements {
                let point = ModelTimePoint {
                    block: point.min_block,
                    epoch_tick: point.min_epoch_tick,
                    median_time: point.min_median_time,
                };
                assert_eq!(
                    point.satisfies(joined),
                    point.satisfies(*left) && point.satisfies(*right)
                );
            }
        }
    }
}

#[test]
fn model_time_eligibility_is_monotone_only_on_coordinatewise_extension() {
    let requirements = requirements();
    for requirement in requirements {
        for block in 0..=2 {
            for epoch_tick in 0..=2 {
                for median_time in 0..=2 {
                    let earlier = ModelTimePoint {
                        block,
                        epoch_tick,
                        median_time,
                    };
                    for later_block in block..=2 {
                        for later_epoch in epoch_tick..=2 {
                            for later_time in median_time..=2 {
                                let later = ModelTimePoint {
                                    block: later_block,
                                    epoch_tick: later_epoch,
                                    median_time: later_time,
                                };
                                assert!(later.extends(earlier));
                                if earlier.satisfies(requirement) {
                                    assert!(later.satisfies(requirement));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let pre_reorg = ModelTimePoint {
        block: 2,
        epoch_tick: 1,
        median_time: 1,
    };
    let post_reorg = ModelTimePoint {
        block: 1,
        epoch_tick: 2,
        median_time: 2,
    };
    let block_requirement = ModelTimeRequirement {
        min_block: 2,
        min_epoch_tick: 0,
        min_median_time: 0,
    };
    assert!(pre_reorg.satisfies(block_requirement));
    assert!(!post_reorg.extends(pre_reorg));
    assert!(!post_reorg.satisfies(block_requirement));
}

#[test]
fn model_current_proposal_status_quotient_uses_phase_owned_bounds() {
    for tip in 0u64..64 {
        for closest in 1u64..16 {
            let observations = [
                (ModelPoolPhase::Pending, tip + 1 + closest),
                (ModelPoolPhase::Gap, tip + closest),
                (ModelPoolPhase::Proposed, tip + 1),
            ];
            for (phase, observed) in observations {
                assert!(model_phase_owned_environment_observation(
                    phase, tip, closest, observed
                ));
                assert!(!model_phase_owned_environment_observation(
                    phase,
                    tip,
                    closest,
                    observed + 1
                ));
            }
        }
    }
    assert!(!model_phase_owned_environment_observation(
        ModelPoolPhase::Pending,
        u64::MAX,
        1,
        u64::MAX
    ));
}

#[test]
fn model_status_only_time_quotient_is_exact_or_conservative_for_legal_fibers() {
    assert_eq!(
        model_proposal_time_observation(ModelPoolPhase::Gap, 10, 2, 4, 10, 12),
        Some(ModelProposalTimeObservation {
            environment_block: 12,
            occurrence_commit_block: 12,
            relation: ModelProposalTimeRelation::Exact,
        })
    );
    assert_eq!(
        model_proposal_time_observation(ModelPoolPhase::Proposed, 10, 2, 4, 7, 11),
        Some(ModelProposalTimeObservation {
            environment_block: 11,
            occurrence_commit_block: 9,
            relation: ModelProposalTimeRelation::Conservative { excess_blocks: 2 },
        })
    );
    assert_eq!(
        model_proposal_time_observation(ModelPoolPhase::Proposed, 10, 1, 3, 10, 11),
        Some(ModelProposalTimeObservation {
            environment_block: 11,
            occurrence_commit_block: 11,
            relation: ModelProposalTimeRelation::Exact,
        })
    );
    assert_eq!(
        model_proposal_time_observation(ModelPoolPhase::Proposed, 10, 3, 5, 8, 11),
        Some(ModelProposalTimeObservation {
            environment_block: 11,
            occurrence_commit_block: 11,
            relation: ModelProposalTimeRelation::Exact,
        })
    );
    assert_eq!(
        model_proposal_time_observation(ModelPoolPhase::Gap, 10, 2, 4, 7, 12),
        None,
        "a concrete proposed-band height cannot be relabeled as Gap"
    );
    assert_eq!(
        model_proposal_time_observation(ModelPoolPhase::Proposed, 10, 1, 3, 10, 12),
        None,
        "the relation cannot repair a wrong production environment observation"
    );
}

#[test]
fn model_context_classifiers_are_total_and_rules_changes_dominate_detach() {
    assert_eq!(
        model_context_sensitivity(false, false),
        ModelContextSensitivity::Stable
    );
    for (has_since, has_cellbase) in [(true, false), (false, true), (true, true)] {
        assert_eq!(
            model_context_sensitivity(has_since, has_cellbase),
            ModelContextSensitivity::TipContext
        );
    }

    assert_eq!(
        model_accepted_validity(false, false),
        ModelAcceptedValidity::Preserved
    );
    assert_eq!(
        model_accepted_validity(false, true),
        ModelAcceptedValidity::ContextChanged
    );
    assert_eq!(
        model_accepted_validity(true, false),
        ModelAcceptedValidity::RulesChanged
    );
    assert_eq!(
        model_accepted_validity(true, true),
        ModelAcceptedValidity::RulesChanged
    );
}
