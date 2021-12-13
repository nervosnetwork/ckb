### How to rebuild all test scripts?

- Download All Dependencies

  - Create a directory named `deps`.

  - Clone https://github.com/nervosnetwork/ckb-c-stdlib into `deps`.

    Checkout the commit 6665e3a289648a0da69b818ea620aeb5e8d74e3b.

- Build all scripts with `docker`.

  ```shell
  make all-in-docker
  ```
