### How to rebuild all test scripts?

- Download All Dependencies

  - Create a directory named `deps`.

  - Clone https://github.com/nervosnetwork/ckb-c-stdlib into `deps`.

- Build all scripts with `docker`.

  ```shell
  make all-in-docker
  ```

`archive_extension` is a self-contained RISC-V consumer used by the real archive
tests. It uses the normal GNU compiler selection above. With LLVM's Clang and
LLD on the path, it can also be rebuilt without the C standard library:

```sh
make archive_extension BIN_CC='clang --target=riscv64 -fuse-ld=lld'
```
