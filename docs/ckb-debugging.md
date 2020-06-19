# CKB Debugging

## Memory

**Only linux versions supported.**

### Tracking Memory Usage in Logs

Add the follow configuration into `ckb.toml`:

```toml
[logger]
filter = "error,ckb-memory-tracker=trace"

[memory_tracker]
# Seconds between checking the process, 0 is disable, default is 0.
interval = 600
```

### Memory Profiling

- Compile `ckb` with feature `profiling`.

  ```sh
  make build-for-profiling
  ```

  After compiling, a script named `jeprof` will be generated in `target` direcotry.

  ```sh
  find target/ -name "jeprof"
  ```

- Enable RPC module `Debug` in `ckb.toml`.

  ```toml
  [rpc]
  modules = ["Debug"]
  ```

- Run `ckb`.

- Dump memory usage to a file via call RPC `jemalloc_profiling_dump`.

  ```sh
  curl -H 'content-type: application/json' -d '{ "id": 2, "jsonrpc": "2.0", "method": "jemalloc_profiling_dump", "params": [] }' http://localhost:8114
  ```

  Then, a file named `ckb-jeprof.$TIMESTAMP.heap` will be generated in the working directory of the running `ckb`.

- Generate a PDF of the call graph.

  **Required**: graphviz and ghostscript

  ```sh
  jeprof --show_bytes --pdf target/debug/ckb ckb-jeprof.$TIMESTAMP.heap > call-graph.pdf
  ```

## Fail-point

### Enable fail-points

- Compile ckb with feature `failpoints`:

  ```shell
  cargo build --release --features failpoints
  ```

- Specify which fail-points and error partterns to enable in `ckb.toml`. You can find more detail from [fail-rs](https://github.com/tikv/fail-rs/blob/v0.4.0/src/lib.rs#L638-L667). Here is an example:

  ```toml
  [failpoints]
  recv_relaytransactions      = "0.1%return"
  recv_getblockproposal       = "0.2%panic"
  send_inibd                  = "sleep(100)"
  send_relaytransactions      = "0.1%print(message)"
  ```

## References:

- [JEMALLOC: Use Case: Leak Checking](https://github.com/jemalloc/jemalloc/wiki/Use-Case%3A-Leak-Checking)
- [JEMALLOC: Use Case: Heap Profiling](https://github.com/jemalloc/jemalloc/wiki/Use-Case%3A-Heap-Profiling)
- [RocksDB: Memory usage in RocksDB](https://github.com/facebook/rocksdb/wiki/Memory-usage-in-RocksDB)
