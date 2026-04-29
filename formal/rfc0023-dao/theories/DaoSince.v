(* DaoSince.v
   DAO locking period definitions for RFC0023.
   Corresponds to RFC0023 Section 4.3: "input since must satisfy Nervos DAO 180 epochs locking period".
*)

Require Import Arith.
Require Import Lia.
Require Import DaoTypes.

(* DAO locking period in epochs.
   Corresponds to RFC0023: "180 epochs locking period". *)
Definition dao_lock_period : nat := 180.

(* valid_dao_unlock_epoch defines when an unlock epoch is valid
   for a given deposit and withdraw epoch.
   Corresponds to RFC0023 Section 4.3 rules:
   - unlock_epoch >= withdraw_epoch (cannot unlock before Phase 1)
   - unlock_epoch = deposit_epoch + k * 180 for some k > 0

   This models the absolute epoch number form of since required by
   RFC0023: "DAO type script only accepts absolute epoch number form of since".

   Parameters:
   - deposit_epoch: epoch number of the deposit block
   - withdraw_epoch: epoch number of the Phase 1 block
   - unlock_epoch: the epoch number when withdrawal is allowed *)
Definition valid_dao_unlock_epoch
  (deposit_epoch : EpochNumber)
  (withdraw_epoch : EpochNumber)
  (unlock_epoch : EpochNumber) : Prop :=
  unlock_epoch >= withdraw_epoch /\
  exists k,
    k > 0 /\
    unlock_epoch = deposit_epoch + k * dao_lock_period.

(* valid_dao_since defines when a since value is valid for DAO withdrawal.
   The since value is the unlock epoch number itself (absolute epoch form).
   Corresponds to RFC0023: "input since must satisfy Nervos DAO 180 epochs locking period".

   Parameters:
   - since_value: the since field value (absolute epoch number)
   - deposit_epoch: epoch number of the deposit block
   - withdraw_epoch: epoch number of the Phase 1 block *)
Definition valid_dao_since
  (since_value : nat)
  (deposit_epoch : EpochNumber)
  (withdraw_epoch : EpochNumber) : Prop :=
  valid_dao_unlock_epoch deposit_epoch withdraw_epoch since_value.
