# Run Dev Chain Using Existing Mainnet or Testnet Data

This guide explains how to run a local `dev` chain using an existing mainnet or
testnet data directory.

This setup is useful when you want local testing with existing chain data.

## 1. Copy the Existing Data Directory

Do not operate on your original data directory directly. Copy it to a new one:

```shell
cp -r /path/to/ckb-data /path/to/ckb-dev-fork
cd /path/to/ckb-dev-fork
```

All commands below assume the current directory is the copied directory.

## 2. Get the Source Chain Spec File

Download the source chain spec file that matches your copied data:

- Mainnet: https://github.com/nervosnetwork/ckb/blob/develop/resource/specs/mainnet.toml
- Testnet: https://github.com/nervosnetwork/ckb/blob/develop/resource/specs/testnet.toml

For example:

```shell
curl -L -o mainnet.toml \
  https://raw.githubusercontent.com/nervosnetwork/ckb/develop/resource/specs/mainnet.toml
```

## 3. Initialize `dev` Chain and Import the Source Spec

```shell
ckb init --chain dev --import-spec ./mainnet.toml --force
```

If you copied testnet data, replace `mainnet.toml` with `testnet.toml`.

## 4. Update `specs/dev.toml`

Set `Dummy` PoW for local development. The `[params]` section differs between
mainnet and testnet — pick the matching block below.

> Why this matters: `genesis_epoch_length` (together with the epoch reward
> fields) participates in the genesis cellbase reward calculation, which
> determines the genesis block hash. If the value here does not match the
> value the source chain was launched with, the node will refuse to start with
> `chainspec error: ChainSpec: genesis hash mismatch`.

### Mainnet

Mainnet was launched with `genesis_epoch_length = 1743`, so this value must be
preserved:

```toml
[params]
genesis_epoch_length = 1743
cellbase_maturity = 0
permanent_difficulty_in_dummy = true

[pow]
func = "Dummy"
```

### Testnet

The bundled testnet spec has no `[params]` section and was launched with the
default `genesis_epoch_length = 1000`. Do **not** add `genesis_epoch_length`
here — leaving it unset lets it fall back to the default and keeps the genesis
hash consistent:

```toml
[params]
cellbase_maturity = 0
permanent_difficulty_in_dummy = true

[pow]
func = "Dummy"
```

`cellbase_maturity = 0` makes locally mined cellbase outputs immediately
spendable, which is convenient for development.

`permanent_difficulty_in_dummy = true` keeps the difficulty constant when
running with `Dummy` PoW. Its default is `false`, which would let difficulty be
recalculated from the dummy block timestamps and swing wildly once you start
mining locally; the bundled `resource/specs/dev.toml` therefore enables it by
default and the same is recommended here.

## 5. First Run Requires Spec-Check Flags

The copied database still records the original chain spec hash, so first startup
must include:

```shell
ckb run --skip-spec-check --overwrite-spec
```

After the first successful run, `ckb run` can be used normally.

## Troubleshooting

If you see a log like `init_snapshot Spec(GenesisMismatch(...))`, the running
spec and database spec do not match. Ensure:

1. You imported the correct source chain spec.
2. The first run uses `--skip-spec-check --overwrite-spec`.
3. You are operating in the copied data directory, not the original one.
