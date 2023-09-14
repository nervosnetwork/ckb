# CKB Platform Support

Support for different platforms are organized into three tiers, each with a different set of guarantees.

The CKB Rust code base uses the [Assembly based interpreter mode](https://github.com/nervosnetwork/ckb-vm#notes-on-different-modes) (ASM mode) of the CKB VM. This must be considered as a consensus rule in Mirana, the mainnet and Pudge, the testnet.

Miners connecting to Mirana (the mainnet) and Pudge (the testnet) must use Tier 1 or Tier 2 platforms, where Tier 1 is recommended.

The other nodes should use Tier 1 or Tier 2 platforms as well.

## Tier 1

Tier 1 platforms can be thought of fully conforming to the CKB consensus and having the guaranteed performance to work.

We ensure that these platforms will satisfy the following requirements:

-   Official binary releases are provided for the platforms.
-   They are fully tested via CI (Continuous Integration).
-   Issues related to these platforms have the top priority.

| OS | Arch | CKB VM Mode |
| --- | --- | --- |
| Ubuntu 18.04 | x64 | ASM |
| macOS | x64 | ASM |
| Windows | x64 | ASM |

The Tier 1 requires CPU to support at least SSE4.2, and AVX is recommended.

## Tier 2

Tier 2 platforms are known to work. But either there are known performance issues or we don't run enough tests in these platforms.

The official binary releases are also provided for the Tier 2 platforms.

| OS | Arch | CKB VM Mode |
| --- | --- | --- |
| Ubuntu 20.04 | x64 | ASM |
| Debian Stretch | x64 | ASM |
| Debian Buster | x64 | ASM |
| Arch Linux | x64 | ASM |
| CentOS 7 | x64 | ASM |
| Ubuntu 20.04 | AArch64 | ASM |
| macOS | AArch64 | ASM |

The Tier 2 requires CPU to support following instructions: call (MODE64), cmovbe (CMOV), xorps (SSE1), movq (SSE2). The provided binaries cannot run on the platforms without these instructions.

## Tier 3

Tier 3 platforms are those which the Rust code base has support for,  but which are not built or tested automatically. Or they have no the working ASM mode of CKB VM thus they are not guaranteed to fully conform to the CKB consensus.

| OS | Arch | CKB VM Mode |
| --- | --- | --- |
| Any OS in Tier 1 and 2 | AArch64 | Rust |
