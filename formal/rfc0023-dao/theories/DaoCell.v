(* DaoCell.v
   DAO cell definitions for RFC0023.
   Corresponds to RFC0023 Section 4.1 (Deposit) and Section 4.2 (Withdraw Phase 1).
*)

Require Import DaoTypes.
Require Import DaoEncoding.

(* Nervos DAO type script.
   Corresponds to RFC0023: "type script = Nervos DAO type script".
   We abstract the actual script values as a parameter. *)
Parameter nervos_dao_type_script : Script.

(* is_dao_type checks if a cell uses the Nervos DAO type script.
   Corresponds to RFC0023 deposit rule: type script must be Nervos DAO. *)
Definition is_dao_type (c : Cell) : Prop :=
  type_script c = Some nervos_dao_type_script.

(* is_deposit_cell defines a valid DAO deposit cell.
   Corresponds to RFC0023 Section 4.1:
   - type script = Nervos DAO type script
   - data = 8 bytes zero *)
Definition is_deposit_cell (c : Cell) : Prop :=
  is_dao_type c /\
  data c = eight_zero_bytes.

(* is_withdrawing_cell defines a valid DAO withdrawing cell.
   Corresponds to RFC0023 Section 4.2 (Withdraw Phase 1):
   - type script = Nervos DAO type script (same as deposit)
   - data = deposit block number encoded as uint64 little-endian

   Parameters:
   - c: the cell to check
   - deposit_block_number: the block number where the deposit was included *)
Definition is_withdrawing_cell
  (c : Cell)
  (deposit_block_number : BlockNumber) : Prop :=
  is_dao_type c /\
  data c = encode_u64_le deposit_block_number.
