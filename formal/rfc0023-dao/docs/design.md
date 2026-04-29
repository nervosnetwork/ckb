# Design Document

## Overview

This document describes the design decisions for the RFC0023 DAO formal verification model.

## Numeric Model

We use `nat` (natural numbers) for all numeric types in Phase 1. This is a deliberate simplification with known limitations:

- **nat subtraction is truncating**: `3 - 5 = 0` in Coq's `nat`, which differs from uint64 underflow semantics
- **No overflow modeling**: `nat` can represent arbitrarily large numbers, unlike uint64
- **Mitigation**: All compensation and header functions include preconditions (`valid_compensation_inputs`, `valid_dao_step_precondition`) to ensure operations are well-defined

Future milestones will migrate to `N` or `Z` with explicit uint64 range constraints.

## Encoding Abstraction

`encode_u64_le` is abstracted as a parameter with axioms:
- Always produces exactly 8 bytes
- Is injective (different inputs produce different outputs)

This allows us to reason about deposit cell data requirements without implementing the full encoding. A concrete encoding model will be added in a follow-up milestone.

## DAO Type Script Abstraction

The actual Nervos DAO type script values are abstracted as a parameter. This is appropriate because:
- The protocol rules only depend on script equality, not specific values
- Different networks (mainnet, testnet) may use different script hashes

## Locking Period Model

The 180-epoch locking period is modeled using existential quantification:

```
unlock_epoch = deposit_epoch + k * 180 for some k > 0
```

This directly captures RFC0023's requirement that withdrawals must occur at multiples of 180 epochs from the deposit epoch.

## Separation of Concerns

We strictly separate:
- **Compensation period**: deposit block -> withdrawing block (determines compensation amount)
- **Locking period**: deposit epoch -> unlock epoch (determines when funds can be withdrawn)

These are independent per RFC0023 specification.

## Axiom Usage

Axioms are used sparingly and only for:
1. `encode_u64_le` properties (encoding abstraction)
2. `nervos_dao_type_script` value (script abstraction)

No axioms are used to prove protocol properties - all theorems are derived from definitions.

## Proof Strategy

All theorems use direct unfolding and automation:
- `unfold` to expand definitions
- `destruct` to break apart conjunctions
- `intros`/`assumption` for basic logic
- `reflexivity` for equality
- `apply Nat.le_add_l`/`Nat.le_0_l` for arithmetic

This keeps proofs simple, readable, and maintainable.
