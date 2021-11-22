/*  Script Description:
 *  - No Argsj
 *  - Returns CKB_SUCCESS always.
 *  - Copy from https://github.com/nervosnetwork/ckb-vm/blob/5f1e124862071b62aeb0bc02587bea1844a04a30/tests/programs/cadd_hints.S
 */

.global _start
_start:
  c.add zero, a0
  li a0, 0
  li a7, 93
  ecall
