# Run CKB with Docker

Start latest CKB release with default configuration:

```bash
docker run -it nervos/ckb:latest run
```

See other
[tags](https://hub.docker.com/r/nervos/ckb/tags)
listed in DockerHub.

- Tag `latest` is always the latest release, which is built from the latest
  master branch.
- Tag `develop` is built from the latest develop branch.
- Tags `vx.y.z` are history releases.
- Tags `vx.y.z-rc` are the preview of the release candidates.

It is recommended to mount a volume at `/var/lib/ckb` in the container.
Following is an example to mount a volume, generate config files in the volume
and start CKB from it. The example will use a local directory as the volume.

First, create the directory.

```bash
mkdir ckb-testnet
```

Then init the directory with testnet chain spec.

```bash
docker run --rm -it \
  -v "$(pwd)/ckb-testnet:/var/lib/ckb" \
  nervos/ckb:latest init --spec testnet
```

Check the directory `ckb-testnet`. It should contains two config files now:
`ckb.toml` and `ckb-miner.toml`.

Edit the files if you like, then start a node from the volume:

```bash
docker run -it
  -v "$(pwd)/ckb-testnet:/var/lib/ckb" \
  nervos/ckb:latest run
```

You can also start a miner with the following command. But you have to publish
ports and edit the RPC address in `ckb-miner.toml` so that the miner can
connect to the node.

```bash
docker run -it
  -v "$(pwd)/ckb-testnet:/var/lib/ckb" \
  nervos/ckb:latest miner
```
