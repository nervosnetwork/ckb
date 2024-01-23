# How to test miner on dev chain

`dev` chain is a local chain for development, its pow algorithm is `Dummy`, which is not a real pow algorithm, it just mines a block in a fixed or random time interval. If you want to test your miner or mining pool related features, you may change the pow algorithm to `Eaglesong` and use dev chain to test. The following steps will show you how to do it.

## Initialize a dev chain

First, you need to prepare one public/private keypair, then initialize a dev chain by passing the public key hash to `--ba-arg` option:

```shell
mkdir ckb-dev-miner-test
cd ckb-dev-miner-test
ckb init --chain dev --ba-arg 0x470dcdc5e44064909650113a274b3b36aecb6dc7
```

## Modify the config file

Then you need to modify the config file `specs/dev.toml`, change the option `func` under the section `[pow]` from `Dummy` to `Eaglesong`:

```diff
- func = "Dummy"
+ func = "Eaglesong"
```

## Start the ckb node

Start the ckb node:

```shell
ckb run
```

You will see the node is listening on the default rpc port `8114` by checking the log output:

```
main INFO ckb_rpc::server  Listen HTTP RPCServer on address: 127.0.0.1:8114
```

## Verify the setup

ckb provides a built-in miner which can be used to verify the setup, you need to modify the section `[[miner.workers]]` of config file `ckb-miner.toml`:

```diff
- worker_type = "Dummy"
- delay_type = "Constant"
- value = 9500
+ worker_type = "EaglesongSimple"
+ threads = 1
```

then start the miner (please note that you need to start the miner in another terminal):

```shell
ckb miner
```

if you see the following similar output, it means the setup is correct:

```
Found! #38 0xb7c7a93578e72316ad35db9131822e9dc52745ad511673b521150cf10ccb2280
Found! #39 0x0c78d91f877764e90e88f11dede4b6e70933bb70d87ef3b14fb42291833ddb86
Found! #40 0xc37b8c375f9ec4b671dc8a553791630d2ffc7a0a4a3fe7d6195121754b08c458
Found! #41 0x43768c4a1a8b723f6b4a23426fda14c044ecc501d56bc2f18fef50829d8e132b
...
```

Now you can stop the miner process and start your own miner or mining pool to test your features.
