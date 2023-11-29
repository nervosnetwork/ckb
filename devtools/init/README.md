# CKB Init Scripts

## Run CKB in deamon mode

CKB has a builtin deamon mode, command to run CKB in deamon mode(only for Linux/MacOS):

```bash
ckb run --deamon
```

Check deamon satus:

```bash
ckb deamon --check
```

Stop deamon process:

```bash
ckb deamon --stop
```

The deamon mode is only for Linux/MacOS, and the CKB service will not be started automatically after reboot.

## Init/Service Scripts

This folder provides the init/service scripts to start CKB node and miner as
daemons on various Unix like distributions.

See the README in each folder for the detailed instructions.

### Disclaimer

Users are expected to know how to administer their system, and these files
should be considered as only a guide or suggestion to setup CKB.
