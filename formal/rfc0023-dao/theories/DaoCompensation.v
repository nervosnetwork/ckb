(* DaoCompensation.v
   DAO compensation calculation for RFC0023.
   Corresponds to RFC0023 Section 4.4: compensation formula and max withdrawable capacity.

   IMPORTANT: This model uses nat for simplicity. nat subtraction is
   truncating (3 - 5 = 0), which differs from uint64 underflow semantics.
   All compensation functions require valid_compensation_inputs precondition
   to ensure subtraction is well-behaved.
*)

Require Import Arith.
Require Import Lia.
Require Import DaoTypes.

(* compensated_capacity_base computes the base compensated capacity.
   Corresponds to RFC0023 formula:
   (c_t - c_o) * AR_n / AR_m

   Parameters:
   - ct: total capacity (c_t)
   - co: occupied capacity (c_o)
   - ar_m: accumulated rate at deposit block (AR_m)
   - ar_n: accumulated rate at Phase 1 block (AR_n) *)
Definition compensated_capacity_base
  (ct co ar_m ar_n : nat) : nat :=
  ((ct - co) * ar_n) / ar_m.

(* dao_compensation computes the DAO compensation amount.
   Corresponds to RFC0023 formula:
   compensation = (c_t - c_o) * AR_n / AR_m - (c_t - c_o)

   Parameters:
   - ct: total capacity (c_t)
   - co: occupied capacity (c_o)
   - ar_m: accumulated rate at deposit block (AR_m)
   - ar_n: accumulated rate at Phase 1 block (AR_n) *)
Definition dao_compensation
  (ct co ar_m ar_n : nat) : nat :=
  compensated_capacity_base ct co ar_m ar_n - (ct - co).

(* max_withdrawable_capacity computes the maximum capacity that can be withdrawn.
   Corresponds to RFC0023 formula:
   max_withdrawable = (c_t - c_o) * AR_n / AR_m + c_o

   Parameters:
   - ct: total capacity (c_t)
   - co: occupied capacity (c_o)
   - ar_m: accumulated rate at deposit block (AR_m)
   - ar_n: accumulated rate at Phase 1 block (AR_n) *)
Definition max_withdrawable_capacity
  (ct co ar_m ar_n : nat) : nat :=
  compensated_capacity_base ct co ar_m ar_n + co.

(* valid_compensation_inputs defines the precondition for compensation calculations.
   These conditions ensure the formulas are well-defined and meaningful:
   - co <= ct: occupied capacity cannot exceed total capacity
   - ar_m > 0: accumulated rate at deposit must be positive (division by zero guard)
   - ar_n >= ar_m: accumulated rate is monotonic (proven separately in DaoHeader.v) *)
Definition valid_compensation_inputs
  (ct co ar_m ar_n : nat) : Prop :=
  co <= ct /\
  ar_m > 0 /\
  ar_n >= ar_m.
