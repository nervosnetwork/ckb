# ckb-tx-pool

The transaction pool admits and verifies transactions, maintains dependencies and
replacement history, reconciles chain changes, and supplies relay observations
and block-template inputs. Consensus and script verification remain in their
canonical crates.

- [Architecture](docs/ARCHITECTURE.md): ownership, commit and execution contracts.
- [Maintenance](docs/MAINTENANCE.md): configuration, caller completion, diagnosis
  and migration. Release changes are in the [changelog](CHANGELOG.md).
- [Benchmarking](docs/BENCHMARK.md): reproducible comparisons and measurement limits.
- [Profiling](docs/PROFILING.md): CPU, allocation and async scheduling tools.
