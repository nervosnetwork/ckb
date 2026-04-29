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

Set `Dummy` PoW for local development and keep `genesis_epoch_length` unchanged:

```toml
[params]
genesis_epoch_length = 1743
initial_primary_epoch_reward = 1_917_808_21917808
secondary_epoch_reward = 613_698_63013698
max_block_cycles = 10_000_000_000
cellbase_maturity = 0
primary_epoch_reward_halving_interval = 8760
epoch_duration_target = 14400
permanent_difficulty_in_dummy = true

[pow]
func = "Dummy"
```

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
