### How to rebuild all test scripts?

- Download All Dependencies

  - Create a directory named `deps`.

  - Clone https://github.com/XuJiandong/ckb-c-stdlib.git with branch `syscall-spawn` into `deps`.

- Build all scripts with `docker`.

  ```shell
  make all-in-docker
  ```
