# [Nervos CKB](https://www.nervos.org/) - The Common Knowledge Base

[![Version](https://img.shields.io/badge/version-0.118.0--rc2-orange.svg)](https://github.com/nervosnetwork/ckb/releases)
[![Nervos Talk](https://img.shields.io/badge/discuss-on%20Nervos%20Talk-3CC68A.svg)](https://talk.nervos.org/t/where-to-discuss-ckb-and-how-to-ask-for-support/6024)

master | develop
-------|----------
[![Unit Tests](https://github.com/nervosnetwork/ckb/actions/workflows/ci_unit_tests_ubuntu.yaml/badge.svg?branch=master)](https://github.com/nervosnetwork/ckb/actions/workflows/ci_unit_tests_ubuntu.yaml?query=branch%3Amaster) | [![Unit Tests](https://github.com/nervosnetwork/ckb/actions/workflows/ci_unit_tests_ubuntu.yaml/badge.svg?branch=develop)](https://github.com/nervosnetwork/ckb/actions/workflows/ci_unit_tests_ubuntu.yaml?query=branch%3Adevelop)
[![Integration Tests](https://github.com/nervosnetwork/ckb/actions/workflows/ci_integration_tests_ubuntu.yaml/badge.svg?branch=master)](https://github.com/nervosnetwork/ckb/actions/workflows/ci_integration_tests_ubuntu.yaml?query=branch%3Amaster) | [![Integration Tests](https://github.com/nervosnetwork/ckb/actions/workflows/ci_integration_tests_ubuntu.yaml/badge.svg?branch=develop)](https://github.com/nervosnetwork/ckb/actions/workflows/ci_integration_tests_ubuntu.yaml?query=branch%3Adevelop)

---

## About CKB

CKB is a public and permissionless layer-1 blockchain. CKB uses [Proof of Work](https://en.wikipedia.org/wiki/Proof-of-work_system) and [improved Nakamoto consensus](https://medium.com/nervosnetwork/breaking-the-throughput-limit-of-nakamoto-consensus-ccdf65fe0832) to achieve maximized performance on average hardware and network bandwidth, without sacrificing layer-1's core values - decentralization and security.

CKB supports scripting in any programming language with its own [CKB-VM](https://github.com/nervosnetwork/ckb-vm/), a virtual machine fully compatible with RISC-V ISA.
CKB is a [Universal Verification Layer](https://medium.com/nervosnetwork/https-medium-com-nervosnetwork-cell-model-7323fca57571) which focuses on verification, leaves computation to layer 2 (and higher) applications/protocols.

[CKB](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0002-ckb/0002-ckb.md) is a part of [Nervos Network](https://www.nervos.org), which defines [a suite of scalable and interoperable blockchain protocols](https://github.com/nervosnetwork/rfcs) to create a self-evolving distributed economy.

Support for different platforms are organized into [three tiers](docs/platform-support.md), each with a different set of guarantees.

**Notice**: The ckb process will send stack trace to sentry on Rust panics. This is enabled by default before the mainnet launch, which can be opted out by setting
the option `dsn` to empty in the config file.


## License [![FOSSA Status](https://app.fossa.io/api/projects/git%2Bgithub.com%2Fnervosnetwork%2Fckb.svg?type=shield)](https://app.fossa.io/projects/git%2Bgithub.com%2Fnervosnetwork%2Fckb?ref=badge_shield)

Nervos CKB is released under the terms of the MIT license. See [COPYING](COPYING) for more information or see [https://opensource.org/licenses/MIT](https://opensource.org/licenses/MIT).


## Join a Network

- Mainnet Mirana: Use the [latest release](https://github.com/nervosnetwork/ckb/releases/latest) and run `ckb init --chain mainnet` to initialize the node.
    - Mirana is active since the epoch 5414, see the [migration guide](https://github.com/jordanmack/nervos-ckb2021-hard-fork-migration-guide) to upgrade from Lina.
- Testnet Pudge: Use the [latest release](https://github.com/nervosnetwork/ckb/releases/latest) and run `ckb init --chain testnet` to initialize the node.
    - Pudge is active since the epoch 3113.


## Mining

CKB uses the [Eaglesong](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0010-eaglesong/0010-eaglesong.md) mining algorithm.


## Development Process

The `master` branch is regularly built and tested. It is considered already production ready; The `develop` branch is the work branch to merge new features, and it's not stable. The CHANGELOG is available in [Releases](https://github.com/nervosnetwork/ckb/releases) and [CHANGELOG.md](https://github.com/nervosnetwork/ckb/blob/master/CHANGELOG.md) in the `master` branch.


## How to Contribute

The contribution workflow is described in [CONTRIBUTING.md](CONTRIBUTING.md), and security policy is described in [SECURITY.md](SECURITY.md). To propose new protocol or standard for Nervos, see [Nervos RFC](https://github.com/nervosnetwork/rfcs).

---

## Documentations

[Latest version](https://github.com/nervosnetwork/ckb#documentations) is hosted in GitHub.

The default branch in GitHub is `develop`, if you are looking for docs for the
Mainnet Mirana or Testnet Pudge, switch to the branch [master].

[master]: https://github.com/nervosnetwork/ckb/tree/master#documentations

- [Quick Start](docs/quick-start.md)
- [Configure CKB](docs/configure.md)
- [Platform Support](docs/platform-support.md)
- [How to Download or Build CKB Binary](https://docs.nervos.org/docs/basics/guides/get-ckb)
- [How to Download or Build CKB Binary on Windows](https://docs.nervos.org/docs/basics/guides/ckb-on-windows)
- [How to test miner on dev chain](docs/dev-miner.md)

You can find a more comprehensive document website at [https://docs.nervos.org](https://docs.nervos.org).
