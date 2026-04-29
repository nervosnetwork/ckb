(* DaoEncoding.v
   Encoding abstractions for RFC0023 DAO model.
   Corresponds to RFC0023 Section 4.4: DAO header field encoding.

   In Phase 1, we abstract uint64 little-endian encoding as parameters
   and axioms. A full encoding model will be added in a follow-up milestone.
*)

Require Import List.
Import ListNotations.
Require Import DaoTypes.

(* Abstract encoding function for uint64 little-endian.
   In a real implementation, this would produce 8 bytes representing
   the unsigned 64-bit little-endian encoding of n.
   Corresponds to RFC0023: "unsigned 64-bit little-endian number". *)
Parameter encode_u64_le : nat -> Bytes.

(* The encoding always produces exactly 8 bytes.
   This matches RFC0023's requirement that each field in the dao
   header is 8 bytes. *)
Axiom encode_u64_le_length :
  forall n, length (encode_u64_le n) = 8.

(* The encoding is injective: different numbers produce different bytes.
   This is a necessary property for the encoding to be reversible. *)
Axiom encode_u64_le_injective :
  forall a b,
    encode_u64_le a = encode_u64_le b ->
    a = b.

(* eight_zero_bytes represents the 8-byte all-zero data required
   for deposit cells.
   Corresponds to RFC0023 Section 4.1: "data: 8 bytes zero". *)
Definition eight_zero_bytes : Bytes :=
  [0; 0; 0; 0; 0; 0; 0; 0].
