(* DaoTransition.v
   DAO state transition definitions for RFC0023.
   Corresponds to RFC0023 Section 4.2 (Withdraw Phase 1) and Section 4.3 (Withdraw Phase 2).
*)

Require Import DaoTypes.
Require Import DaoCell.
Require Import DaoEncoding.
Require Import DaoSince.

(* valid_phase1_pair defines when a deposit cell and withdrawing cell
   form a valid Phase 1 transition pair.
   Corresponds to RFC0023 Section 4.2 rules:
   1. deposit cell must be a valid deposit cell
   2. withdrawing cell must have the same type script as deposit
   3. withdrawing cell capacity must equal deposit cell capacity
   4. withdrawing cell data must encode the deposit block number

   Parameters:
   - deposit: the input deposit cell
   - withdrawing: the output withdrawing cell
   - deposit_header: the header of the block where deposit was included *)
Definition valid_phase1_pair
  (deposit : Cell)
  (withdrawing : Cell)
  (deposit_header : Header) : Prop :=
  is_deposit_cell deposit /\
  type_script withdrawing = type_script deposit /\
  capacity withdrawing = capacity deposit /\
  data withdrawing = encode_u64_le (block_number deposit_header).

(* valid_phase2_input defines when a transaction input is a valid
   Phase 2 withdrawing input.
   Corresponds to RFC0023 Section 4.3 rules:
   1. input cell must be a valid withdrawing cell
   2. input since must satisfy DAO locking period

   Parameters:
   - input: the transaction input
   - deposit_header: the header of the original deposit block
   - withdrawing_header: the header of the block where Phase 1 occurred *)
Definition valid_phase2_input
  (input : CellInput)
  (deposit_header : Header)
  (withdrawing_header : Header) : Prop :=
  is_withdrawing_cell
    (previous_cell input)
    (block_number deposit_header) /\
  valid_dao_since
    (since input)
    (epoch_number deposit_header)
    (epoch_number withdrawing_header).
