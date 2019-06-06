# [Nervos CKB](https://www.nervos.org/) - The Common Knowledge Base

[![Version](https://img.shields.io/badge/version-0.15.0--pre-orange.svg)](https://github.com/nervosnetwork/ckb/releases)
[![Telegram Group](https://cdn.jsdelivr.net/gh/Patrolavia/telegram-badge@8fe3382b3fd3a1c533ba270e608035a27e430c2e/chat.svg)](https://t.me/nervosnetwork)
[![Nervos Talk](https://img.shields.io/badge/discuss-on%20Nervos%20Talk-3CC68A.svg)](https://talk.nervos.org/)

Branch    | Tests | Coverage
----------|-------|----------
master    | [![TravisCI](https://travis-ci.com/nervosnetwork/ckb.svg?branch=master)](https://travis-ci.com/nervosnetwork/ckb) | [![Codecov](https://codecov.io/gh/nervosnetwork/ckb/branch/master/graph/badge.svg)](https://codecov.io/gh/nervosnetwork/ckb/branch/master)
develop   | [![TravisCI](https://travis-ci.com/nervosnetwork/ckb.svg?branch=develop)](https://travis-ci.com/nervosnetwork/ckb) | [![Codecov](https://codecov.io/gh/nervosnetwork/ckb/branch/develop/graph/badge.svg)](https://codecov.io/gh/nervosnetwork/ckb/branch/develop)

---

## About CKB

CKB is the layer 1 of Nervos Network, a public/permissionless blockchain. CKB uses [Proof of Work](https://en.wikipedia.org/wiki/Proof-of-work_system) and [improved Nakamoto concensus](https://medium.com/nervosnetwork/breaking-the-throughput-limit-of-nakamoto-consensus-ccdf65fe0832) to achieve maximized performance on average hardware and internet condition, without sacrificing decentralization and security which are the core value of blockchain.

CKB supports scripting in any programming language with its own [CKB-VM](https://github.com/nervosnetwork/ckb-vm/), a virtual machine fully compatible with RISC-V ISA. CKB is a [General Verification Network](https://medium.com/nervosnetwork/https-medium-com-nervosnetwork-cell-model-7323fca57571), its programming model focuses on state verification, leaves state generation to layer 2 applications/protocols.

[Nervos project](https://www.nervos.org) defines [a suite of scalable and interoperable blockchain protocols](https://github.com/nervosnetwork/rfcs) to create a self-evolving distributed economy, [CKB](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0002-ckb/0002-ckb.md) is among them.

**Notice**: The ckb process will send stack trace to sentry on Rust panics.
This is enabled by default before mainnet, which can be opted out by setting
the option `dsn` to empty in the config file.

## Testnet Rylai

CKB Testnet *Rylai* is the testing network before its mainnet launch. Switch
to branch [master] to see the documentations.

## Mainnet

TBD.

## Mining

Mining on testnet Rylai is for testing purpose only, the mining algorithm **WILL BE CHANGED SOON**.

At this time Rylai will be **RESET** every two weeks.

Please check [here](https://docs.nervos.org/getting-started/mine) for more information.

## License [![FOSSA Status](https://app.fossa.io/api/projects/git%2Bgithub.com%2Fnervosnetwork%2Fckb.svg?type=shield)](https://app.fossa.io/projects/git%2Bgithub.com%2Fnervosnetwork%2Fckb?ref=badge_shield)

Nervos CKB is released under the terms of the MIT license. See [COPYING](COPYING) for more information or see [https://opensource.org/licenses/MIT](https://opensource.org/licenses/MIT).

## Development Process

This project is still in development, and it's NOT in production-ready status.
The board also lists some [known issues](https://github.com/nervosnetwork/ckb/projects/2) that we are currently working on.

The `master` branch is regularly built and tested, however, it is not guaranteed to be completely stable; The `develop` branch is the work branch to merge new features, and it's not stable. The CHANGELOG is available in [Releases](https://github.com/nervosnetwork/ckb/releases) and [CHANGELOG.md](https://github.com/nervosnetwork/ckb/blob/master/CHANGELOG.md) in the `master` branch.

## How to Contribute

The contribution workflow is described in [CONTRIBUTING.md](CONTRIBUTING.md), and security policy is described in [SECURITY.md](SECURITY.md). To propose new protocol or standard for Nervos, see [Nervos RFC](https://github.com/nervosnetwork/rfcs).

---

## Documentations

[Latest version](https://github.com/nervosnetwork/ckb#documentations) is hosted in GitHub.

The default branch in GitHub is `develop`, if you are looking for docs for the
Testnet Rylai, switch to the branch [master].

[master]: https://github.com/nervosnetwork/ckb/tree/master#documentations

- [Get CKB](docs/get-ckb.md)
- [Quick Start](docs/quick-start.md)
- [Run CKB with Docker](docs/run-ckb-with-docker.md)
- [Configure CKB](docs/configure.md)
- [CKB Core Development](docs/ckb-core-dev.md)

You can find a more comprehensive document website at [https://docs.nervos.org](https://docs.nervos.org).
