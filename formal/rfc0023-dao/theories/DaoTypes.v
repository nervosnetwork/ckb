(* DaoTypes.v
   Basic type definitions for RFC0023 DAO formal model.
   Corresponds to RFC0023 Section 4: deposit / withdraw rules.
*)

Require Import List.
Import ListNotations.

(* Basic numeric types used throughout the model.
   We use nat for simplicity. Underflow / overflow are not fully modeled yet.
   See docs/design.md for discussion of numeric model limitations. *)
Definition Capacity := nat.
Definition BlockNumber := nat.
Definition EpochNumber := nat.
Definition HeaderIndex := nat.
Definition Bytes := list nat.

(* Script represents a CKB script (lock or type).
   Corresponds to CKB Script structure in RFC0023. *)
Record Script := {
  code_hash : nat;
  hash_type : nat;
  args      : Bytes
}.

(* Cell represents a CKB cell.
   Corresponds to CKB Cell structure in RFC0023 Section 4.1. *)
Record Cell := {
  capacity    : Capacity;
  lock_script : Script;
  type_script : option Script;
  data        : Bytes
}.

(* DaoField represents the 32-byte `dao` field in a block header.
   Corresponds to RFC0023 Section 4.4.
   Fields:
   - total_issuance (C_i): total CKBytes issuance
   - accumulated_rate (AR_i): accumulated rate with 10^16 precision
   - total_unissued_secondary (S_i): total unissued secondary issuance
   - total_occupied_capacity (U_i): total occupied capacities *)
Record DaoField := {
  total_issuance           : nat;
  accumulated_rate         : nat;
  total_unissued_secondary : nat;
  total_occupied_capacity  : nat
}.

(* Header represents a block header containing DAO-related fields.
   Corresponds to CKB Block Header in RFC0023. *)
Record Header := {
  block_number : BlockNumber;
  epoch_number : EpochNumber;
  dao          : DaoField
}.

(* CellInput represents a transaction input with a since field.
   Corresponds to CKB CellInput structure in RFC0023 Section 4.3. *)
Record CellInput := {
  previous_cell : Cell;
  since         : nat
}.

(* Transaction represents a CKB transaction.
   Corresponds to CKB Transaction structure.
   We keep a simplified model for Phase 1/2 validation. *)
Record Transaction := {
  inputs      : list CellInput;
  outputs     : list Cell;
  cell_deps   : list Script;
  header_deps : list Header;
  witnesses   : list Bytes
}.
