## bench CKB IBD sync time cost

1. build bench_ibd_sync:
```shell
make bench_ibd_sync
```
2. run bench_ibd_sync:

`bench_ibd_sync` need a fully synced CKB node, because mock nodes need to read the data from the fully synced node's database, which should be specified by `--shared-ckb-db-path`

The mock nodes and main node's configuration will be put under `--work-dir`, and main node only write log to file.

`bench_ibd_sync` will exit when main node's tip height reach `--target-height`

```shell
./target/debug/bench_ibd_sync \
      --main-node-binary-path "[ckb full path for benchmark]" \
      --main-node-log-filter "info,ckb-sync=debug" \
      --shared-ckb-db-path "[rocksdb's full path for an already synced to latest tip height node]" \
      --work-dir "[should be an empty dir, bench_ibd_sync will put data file here for mock nodes and main node]" \
      --target-height 8000000
```