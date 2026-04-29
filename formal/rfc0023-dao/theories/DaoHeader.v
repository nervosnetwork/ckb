(* DaoHeader.v
   DAO header field and accumulated rate definitions for RFC0023.
   Corresponds to RFC0023 Section 4.4: DAO header field and its recursive update rules.
*)

Require Import Arith.
Require Import Lia.
Require Import DaoTypes.

(* BlockIssuance represents the per-block issuance and occupancy changes.
   Corresponds to RFC0023 variables:
   - primary_issuance (p_i): primary issuance for this block
   - secondary_issuance (s_i): secondary issuance for this block
   - occupied_inputs (U_in,i): occupied capacity consumed as inputs
   - occupied_outputs (U_out,i): occupied capacity created as outputs
   - completed_comp (I_i): completed compensation (withdrawn from DAO) *)
Record BlockIssuance := {
  primary_issuance   : nat;
  secondary_issuance : nat;
  occupied_inputs    : nat;
  occupied_outputs   : nat;
  completed_comp     : nat
}.

(* next_dao_field computes the next DAO field given the previous state
   and block issuance.
   Corresponds to RFC0023 recursive formulas:
   C_i  = C_{i-1} + p_i + s_i
   U_i  = U_{i-1} + U_out,i - U_in,i
   S_i  = S_{i-1} - I_i + s_i - floor(s_i * U_{i-1} / C_{i-1})
   AR_i = AR_{i-1} + floor(AR_{i-1} * s_i / C_{i-1})

   Parameters:
   - prev: previous DAO field state
   - bi: block issuance for the current block *)
Definition next_dao_field
  (prev : DaoField)
  (bi : BlockIssuance) : DaoField :=
  {|
    total_issuance :=
      total_issuance prev
      + primary_issuance bi
      + secondary_issuance bi;

    total_occupied_capacity :=
      total_occupied_capacity prev
      + occupied_outputs bi
      - occupied_inputs bi;

    total_unissued_secondary :=
      total_unissued_secondary prev
      - completed_comp bi
      + secondary_issuance bi
      - ((secondary_issuance bi * total_occupied_capacity prev)
          / total_issuance prev);

    accumulated_rate :=
      accumulated_rate prev
      + ((accumulated_rate prev * secondary_issuance bi)
          / total_issuance prev)
  |}.

(* valid_dao_step_precondition defines when a DAO step is valid.
   These conditions ensure the recursive formulas are well-defined:
   - total_issuance prev > 0: avoids division by zero
   - occupied_inputs bi <= total_occupied_capacity prev + occupied_outputs bi:
     ensures total_occupied_capacity doesn't underflow *)
Definition valid_dao_step_precondition
  (prev : DaoField)
  (bi : BlockIssuance) : Prop :=
  total_issuance prev > 0 /\
  occupied_inputs bi <= total_occupied_capacity prev + occupied_outputs bi.
