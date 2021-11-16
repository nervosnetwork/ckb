# Script Description

- Args:
  - A little endian unsigned integer: `number`.
  - The `code_hash`(`data_hash`) of a shared library which contains the method `is_even`.

- Returns `CKB_SUCCESS` if and only if any follow conditions satisfied:
  - `number` is zero.
  - `number` is not even.

- Compile Environment
  - Checkout the `ckb-system-scripts v0.5.4`.
  - Put this directory into the root directory of `ckb-system-scripts v0.5.4`.
  - Checkout `ckb-c-stdlib rev=6665e3a289648a0da69b818ea620aeb5e8d74e3b` into `deps` directory of `ckb-system-scripts v0.5.4`.
  - Run `make all-in-docker`.
