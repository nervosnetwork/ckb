# ckb-tx-pool

The transaction pool admits and verifies transactions, maintains dependencies and
replacement history, reconciles chain changes, and supplies relay observations
and block-template inputs. Consensus and script verification remain in their
canonical crates.

One immutable owner represents each retained transaction. Decisions carry their
original observations into a single commit that updates owners, indexes, charges
and required publication obligations together. Compatible ordinary transactions
can commit concurrently; callbacks and network-facing effects run after locks
are released.

## Reading paths

| Task | Start here |
|---|---|
| Understand the design and responsibility boundaries | [Architecture](docs/ARCHITECTURE.md) |
| Review the implementation and its evidence | [Review guide](docs/REVIEW_GUIDE.md) |
| Change policy, integrate callers or diagnose operation | [Maintenance](docs/MAINTENANCE.md) |
| Reproduce a comparison and interpret its statistics | [Benchmark](docs/BENCHMARK.md) |
| Find CPU, allocation or async scheduling costs | [Profiling and development tools](docs/PROFILING.md) |
| Read the current performance report | [Performance](docs/PERFORMANCE.md) |
| Migrate from the prior pool | [Migration guidance](docs/MAINTENANCE.md#integrating-callers-and-stored-data) and [changelog](CHANGELOG.md) |
