# RFC0023 to Coq Mapping

This document maps RFC0023 rules to their Coq formalizations.

## Deposit Rules

| RFC0023 Rule | Coq Definition | Coq Theorem | Rust Location |
|---|---|---|---|
| Deposit cell uses DAO type script | `is_deposit_cell` | `deposit_cell_has_dao_type` | 待定位 |
| Deposit cell data is 8 bytes zero | `is_deposit_cell` | `deposit_cell_data_is_zero` | 待定位 |

## Withdraw Phase 1

| RFC0023 Rule | Coq Definition | Coq Theorem | Rust Location |
|---|---|---|---|
| Phase 1 output[i] corresponds to input[i] | `valid_phase1_pair` / `valid_phase1_tx` | 后续补 | 待定位 |
| Phase 1 capacity unchanged | `valid_phase1_pair` | `phase1_preserves_capacity` | 待定位 |
| Phase 1 data writes deposit block number | `valid_phase1_pair` | `phase1_records_deposit_block_number` | 待定位 |

## Withdraw Phase 2

| RFC0023 Rule | Coq Definition | Coq Theorem | Rust Location |
|---|---|---|---|
| Phase 2 since satisfies 180 epochs | `valid_dao_since` | `phase2_requires_valid_since` | 待定位 |
| Compensation formula | `dao_compensation` | 后续补 | 待定位 |
| Max withdrawable capacity | `max_withdrawable_capacity` | `max_withdrawable_definition` | 待定位 |

## DAO Header

| RFC0023 Rule | Coq Definition | Coq Theorem | Rust Location |
|---|---|---|---|
| Header field recursive update | `next_dao_field` | `accumulated_rate_monotonic` | 待定位 |
| AR monotonicity | `next_dao_field` | `accumulated_rate_monotonic` | 待定位 |

## Notes

- Rust 对接位置第一版可以先写 "待定位"，后续再去 CKB 仓库里定位 `verification`、`script`、`util`、`test` 中的 DAO 相关代码
- 完整 transaction-level 验证将在后续 milestone 中补充
