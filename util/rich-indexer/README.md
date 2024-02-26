# CKB Rich-Indexer

CKB Rich-Indexer is a new built-in indexing implementation in CKB. It is based on a relational database and supports more flexible SQL queries.

Currently, Rich-Indexer supports two types of database drivers.

1. One is the embedded SQLite, which users can start using without any configuration.
2. The other is PostgreSQL, which can undergo independent, customized configurations for both software and hardware. Additionally, it allows users to engage in secondary development based on it.

## Hardware Requirements

In order to run a CKB node with the Rich-Indexer enabled, it is recommended to adhere to the following minimum hardware specifications:

- Processor: 4 core
- RAM: 8 GB

## Quick Start

Running the Rich-Indexer with the default SQLite-based setting is as simple as using the original CKB Indexer. No extra configuration is needed; just apply the `--rich-indexer` command-line option.

```bash
ckb run -C <path> --rich-indexer
```

Similar to the original CKB Indexer, enabling the Rich-Indexer initiates a process of synchronizing blocks and creating indices. This process takes several hours. Taking CKB Testnet as an example, on hardware with 4 cores and 8 GB of RAM, syncing up to a height of around 11,000,000 takes approximately 48 hours.

In addition to full indexing, the Rich-Indexer can reuse all configurations from the original CKB Indexer, such as `block_filter`, `cell_filter`, `init_tip_hash`, to create customized indexes. This approach accelerates synchronization completion and requires less disk space.

ckb.toml:

```toml
# CKB built-in indexer/rich-indexer settings.
# Utilize the `ckb reset-data --indexer` and `ckb reset-data --rich-indexer` subcommands to efficiently clean existing indexes.
[indexer_v2]
# # Indexing the pending txs in the ckb tx-pool
# index_tx_pool = false
# # Customize block filtering rules to index only retained blocks
block_filter = "block.header.number.to_uint() >= \"0x0\".to_uint()"
# # Customize cell filtering rules to index only retained cells
cell_filter = "let script = output.type;script!=() && script.code_hash == \"0x00000000000000000000000000000000000000000000000000545950455f4944\""
# # The initial tip can be set higher than the current indexer tip as the starting height for indexing.
init_tip_hash = "0x8fbd0ec887159d2814cee475911600e3589849670f5ee1ed9798b38fdeef4e44"
```

Once the Rich-Indexer is activated, the CKB node's RPC based on the Rich-Indexer will gain additional capabilities.

| INDEXER RPC          | Indexer           | Rich-Indexer       |
|---------------------|-------------------|--------------------|
| `get_cells` script args `partial` mode search     | ❌     | ✔️      |
| `get_cells` cell data filter(`prefix\|exact\|partial`)     | ✔️     | ✔️      |
| `get_transactions` script args `partial` mode search | ❌          | ✔️   |
| `get_transactions` cell data filter(`prefix\|exact\|partial`)     | ❌         |  ✔️      |
| `get_cells_capacity` script args `partial` mode search        |    ❌    |    ✔️     |
| `get_cells_capacity` cell data filter(`prefix\|exact\|partial`)        |    ✔️    |   ✔️     |

Note that CKB starting options `--indexer` and `--rich-indexer` can only be used exclusively; you can choose only one for startup.

## Enabling Rich Indexer with PostgreSQL

To enable PostgreSQL, you must first set up a functional PostgreSQL service on your own. Please refer to [Server Administration](https://www.postgresql.org/docs/16/admin.html) for guidance. It is recommended to install version 12 or above.

For hardware with 4 cores and 8 GB of RAM, it is recommended to make the following two configuration parameter adjustments in PostgreSQL to achieve optimal query performance.

postgresql.conf:

```conf
#------------------------------------------------------------------------------
# RESOURCE USAGE (except WAL)
#------------------------------------------------------------------------------

# - Memory -

shared_buffers = 2GB                    # min 128kB
```

```conf
#------------------------------------------------------------------------------
# QUERY TUNING
#------------------------------------------------------------------------------

# - Other Planner Options -

jit = off                               # allow JIT compilation
```

Next, configure the PostgreSQL connection parameters for the CKB node.

ckb.toml:

```toml
# CKB rich-indexer has its unique configuration.
[indexer_v2.rich_indexer]
# By default, it uses an embedded SQLite database.
# Alternatively, you can set up a PostgreSQL database service and provide the connection parameters.
db_type = "postgres"
db_name = "ckb-rich-indexer"
db_host = "127.0.0.1"
db_port = 5432
db_user = "postgres"
db_password = "123456"
```

Finally, start the CKB node:

```bash
ckb run -C <path> --rich-indexer
```

## Secondary development using SQL

The RPCs based on the Rich-Indexer have already gained additional query capabilities facilitated by SQL. However, if the current RPCs exposed by the CKB node are insufficient to meet your needs, an alternative option is to directly interact with the Rich-Indexer's PostgreSQL database using SQL through any programming language.

Please refer to the SQL file for the database table design and index information:

- [create_postgres_table.sql](./resources/create_postgres_table.sql)
- [create_postgres_index.sql](./resources/create_postgres_index.sql)

**Note** that when undertaking secondary development on the Rich-Indexer's PostgreSQL database, perform queries strictly in a read-only manner to avoid conflicts with the Rich-Indexer's synchronous writes.











