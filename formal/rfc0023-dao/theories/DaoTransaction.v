(* DaoTransaction.v
   Transaction-level definitions and helper functions for RFC0023 DAO model.
   Corresponds to CKB Transaction structure used in Phase 1 and Phase 2 validation.
*)

Require Import List.
Import ListNotations.
Require Import Arith.
Require Import Lia.
Require Import DaoTypes.

(* Sum of capacities of a list of cells. *)
Fixpoint cell_capacity_sum (cells : list Cell) : nat :=
  match cells with
  | [] => 0
  | c :: cs => capacity c + cell_capacity_sum cs
  end.

(* Total input capacity of a transaction. *)
Definition input_capacity_sum (tx : Transaction) : nat :=
  cell_capacity_sum (map previous_cell (inputs tx)).

(* Total output capacity of a transaction. *)
Definition output_capacity_sum (tx : Transaction) : nat :=
  cell_capacity_sum (outputs tx).

(* Get header at index i from header_deps, if valid. *)
Definition header_dep_at (tx : Transaction) (i : nat) : option Header :=
  nth_error (header_deps tx) i.

(* Get witness at index i from witnesses, if valid. *)
Definition witness_at (tx : Transaction) (i : nat) : option Bytes :=
  nth_error (witnesses tx) i.

(* Check if a script is in cell_deps. *)
Definition has_cell_dep (tx : Transaction) (s : Script) : Prop :=
  In s (cell_deps tx).
