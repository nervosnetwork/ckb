/* Witness 0 contains: expected status (u8), source (u8), syscall (u16 LE).
 * Read cell 0's header or extension and compare its availability with that
 * status: 2 means ITEM_MISSING, 0 means SUCCESS. A mismatch exits with 42.
 */
.global _start
_start:
    addi sp, sp, -32
    li t0, 4
    sd t0, 0(sp)
    addi a0, sp, 8          /* Four-byte witness */
    mv a1, sp
    li a2, 0                /* Offset */
    li a3, 0                /* Index */
    li a4, 1                /* SOURCE_INPUT */
    li a7, 2074             /* LOAD_WITNESS */
    ecall
    bnez a0, exit
    lbu s0, 8(sp)           /* Expected return status */
    lbu a4, 9(sp)           /* SOURCE_INPUT (1) or SOURCE_CELL_DEP (3) */
    lhu a7, 10(sp)          /* LOAD_HEADER (2072) or LOAD_BLOCK_EXTENSION (2104) */
    sd zero, 0(sp)          /* Length = 0: check availability only. */
    li a0, 0
    mv a1, sp
    li a2, 0
    li a3, 0
    ecall
    bne a0, s0, mismatch
    li a0, 0
    j exit
mismatch:
    li a0, 42
exit:
    li a7, 93
    ecall
