# RFC0023 DAO Formal Verification

This project contains a Coq/Rocq formal model for the Nervos DAO deposit / withdraw protocol defined in [RFC0023](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0023-dao-deposit-withdraw/0023-dao-deposit-withdraw.md).

## Overview

The goal is to formally verify the core protocol rules of RFC0023:
1. Deposit cell definition and properties
2. Withdraw Phase 1 transition rules
3. Withdraw Phase 2 locking period and since validation
4. DAO compensation calculation
5. DAO header field recursive update and monotonicity

## Project Structure

```
formal/rfc0023-dao/
  README.md
  _CoqProject
  Makefile

  docs/
    design.md
    proof-plan.md
    rfc0023-mapping.md

  theories/
    DaoTypes.v        - Basic type definitions
    DaoEncoding.v     - Encoding abstractions
    DaoCell.v         - DAO cell definitions
    DaoTransaction.v  - Transaction helpers
    DaoSince.v        - Locking period rules
    DaoCompensation.v - Compensation formulas
    DaoHeader.v       - DAO header recursive update
    DaoTransition.v   - Phase 1/2 transition rules
    DaoTheorems.v     - Core theorems and proofs

  examples/
    dao_compensation_cases.json
    dao_since_cases.json
    dao_phase1_cases.json

  scripts/
    check-no-admitted.sh
```

## How to Build

Requirements: Coq 8.18+ or Rocq

```bash
cd formal/rfc0023-dao
coq_makefile -f _CoqProject -o Makefile
make
```

## How to Check for Admitted

```bash
./scripts/check-no-admitted.sh
```

## Limitations

- This is an abstract protocol model, not a proof of the Rust implementation.
- `encode_u64_le` is currently abstracted as a parameter with axioms.
- Full transaction-level validation will be added in follow-up milestones.
- Rust test-vector integration is planned for later milestones.
- This model uses `nat` for simplicity. Underflow / overflow are not fully modeled yet.

## RFC0023 Coverage

| RFC Rule | Coq Definition | Theorem |
|---|---|---|
| Deposit cell must use DAO type script | `is_deposit_cell` | `deposit_cell_has_dao_type` |
| Deposit cell data must be 8 zero bytes | `is_deposit_cell` | `deposit_cell_data_is_zero` |
| Phase1 preserves capacity | `valid_phase1_pair` | `phase1_preserves_capacity` |
| Phase1 records deposit block number | `valid_phase1_pair` | `phase1_records_deposit_block_number` |
| Phase2 since must satisfy 180 epochs | `valid_dao_since` | `phase2_requires_valid_since` |
| Max withdrawable capacity formula | `max_withdrawable_capacity` | `max_withdrawable_definition` |
| AR monotonicity | `next_dao_field` | `accumulated_rate_monotonic` |

## License

MIT
