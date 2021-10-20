/*  Script Description:
 *  - No Argsj
 *  - Returns CKB_SUCCESS always.
 *  - Copy from https://github.com/nervosnetwork/ckb-vm/blob/edb5b83084c3b3b978e26298151ac72a10769fd6/tests/programs/mop_adc.S.
 *  Compile Environment:
 *  - Toolchains: nervos/ckb-riscv-gnu-toolchain@sha256:7b168b4b109a0f741078a71b7c4dddaf1d283a5244608f7851f5714fbad273ba
 *  - Commands:
 *    ```shell
 *    riscv64-unknown-elf-as -o mop_adc_lock.o mop_adc_lock.S
 *    riscv64-unknown-elf-ld -o mop_adc_lock mop_adc_lock.o
 *    riscv64-unknown-elf-objcopy --strip-debug --strip-all mop_adc_lock
 *    ```
 */

.global _start
_start:
  li a0, 0x7fffffffffffffff
  li a1, 1
  li a2, 0x8000000000000000
  add a0, a0, a1
  sltu a1, a0, a1
  add a0, a0, a2
  sltu a2, a0, a2
  or a1, a1, a2
  li t0, 0
  bne a0, t0, fail
  li t0, 1
  bne a1, t0, fail
  li t0, 1
  bne a2, t0, fail

  li a0, 0x390a5a5fad56f578
  li a1, 0xdd8f0883fdd78883
  li a2, 0x7f6dbfa760006b9e
  add a0, a0, a1
  sltu a1, a0, a1
  add a0, a0, a2
  sltu a2, a0, a2
  or a1, a1, a2
  li t0, 0x9607228b0b2ee999
  bne a0, t0, fail
  li t0, 1
  bne a1, t0, fail
  li t0, 0
  bne a2, t0, fail

  li a1, 1
  li a2, 1
  add zero, zero, a1
  sltu a1, zero, a1
  add zero, zero, a2
  sltu a2, zero, a2
  or a1, a1, a2
  li t0, 1
  bne a1, t0, fail
  li t0, 1
  bne a2, t0, fail

  li a0, 0
  li a7, 93
  ecall
fail:
  li a0, 1
  li a7, 93
  ecall
