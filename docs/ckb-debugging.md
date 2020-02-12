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
  make build-for-profiling`
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

## References:

- [JEMALLOC: Use Case: Leak Checking](https://github.com/jemalloc/jemalloc/wiki/Use-Case%3A-Leak-Checking)
- [JEMALLOC: Use Case: Heap Profiling](https://github.com/jemalloc/jemalloc/wiki/Use-Case%3A-Heap-Profiling)
