# Run CKB with Docker

Start latest CKB release with default configuration:

```bash
docker run --rm -it nervos/ckb:latest run
```

Starting from v0.26.1, the docker container connects to mainnet by default,
which can be overrode by the environment variable `CKB_CHAIN`, for example


```bash
docker run -e CKB_CHAIN=testnet --rm -it nervos/ckb:latest run
```

See other
[tags](https://hub.docker.com/r/nervos/ckb/tags)
listed in DockerHub.

-   Tag `latest` is always the latest release, which is built from the latest
    master branch.
-   Tags `vx.y.z` are history releases.
-   Tags `vx.y.z-rc` are the preview of the release candidates.

It is recommended to mount a volume at `/var/lib/ckb` in the container.
Following is an example to mount a volume, generate config files in the volume
and start CKB from it.

First, create a volume.

```bash
docker volume create ckb-mainnet
```

Then init the directory with mainnet chain spec.

```bash
docker run --rm -it \
  -v ckb-mainnet:/var/lib/ckb \
  nervos/ckb:latest init --chain mainnet --force
```

Create a container `ckb-mainnet-node` to run a node:

```bash
docker create -it \
  -v ckb-mainnet:/var/lib/ckb \
  --name ckb-mainnet-node \
  nervos/ckb:latest run
```

Copy the generated config files from the container:

```bash
docker cp ckb-mainnet-node:/var/lib/ckb/ckb.toml .
docker cp ckb-mainnet-node:/var/lib/ckb/ckb-miner.toml .
```

Edit the config files as you like. If you want to run a miner, remember to
replace `[block_assembler]` section in `ckb.toml`.

Copy back the edited config files back to container:

```bash
tar --owner=1000 --group=1000 -cf - ckb.toml ckb-miner.toml | \
  docker cp - ckb-mainnet-node:/var/lib/ckb/
```

Now start the node:

```bash
docker start -i ckb-mainnet-node
```

And start the miner in the same container

```bash
docker exec ckb-mainnet-node ckb miner
```
