# Proof Plan

## Completed Theorems

### Milestone 1: Deposit Cell Soundness
- [x] `deposit_cell_has_dao_type` - Every deposit cell has DAO type script
- [x] `deposit_cell_data_is_zero` - Every deposit cell has 8 zero bytes

### Milestone 2: Phase 1 Properties
- [x] `phase1_preserves_capacity` - Phase 1 preserves capacity
- [x] `phase1_records_deposit_block_number` - Phase 1 records deposit block number

### Milestone 3: Phase 2 Locking
- [x] `phase2_requires_valid_since` - Phase 2 requires valid since

### Milestone 4: Compensation
- [x] `max_withdrawable_definition` - Max withdrawable definition
- [x] `compensation_nonnegative` - Compensation is non-negative
- [x] `max_withdrawable_ge_occupied` - Max withdrawable >= occupied capacity

### Milestone 5: DAO Header
- [x] `accumulated_rate_monotonic` - AR is non-decreasing

## Future Theorems (Phase 2)

### Transaction-Level Validation
- [ ] `valid_phase1_tx` - Full transaction-level Phase 1 validation
- [ ] `valid_phase2_tx` - Full transaction-level Phase 2 validation
- [ ] `phase1_input_output_correspondence` - Input[i] corresponds to output[i]

### Compensation Properties
- [ ] `compensation_formula_correct` - Compensation matches RFC0023 formula
- [ ] `compensation_independent_of_phase2` - Compensation doesn't depend on Phase 2 block
- [ ] `phase2_output_capacity_bound` - Phase 2 outputs don't exceed max withdrawable

### Header Properties
- [ ] `total_issuance_non_decreasing` - Total issuance never decreases
- [ ] `dao_field_well_formed` - All DAO fields remain well-formed

### Security Properties
- [ ] `no_early_unlock` - Cannot withdraw before locking period expires
- [ ] `capacity_conservation` - Total capacity is conserved (minus compensation)
- [ ] `type_script_preservation` - DAO type script is preserved through transitions
