<img src="https://raw.githubusercontent.com/poshboytl/tuchuang/master/nervos-logo-dark.png" width="256">

# [Nervos CKB](https://www.nervos.org/) - The Common Knowledge Base

[![TravisCI](https://travis-ci.com/nervosnetwork/ckb.svg?token=y9uR6ygmT3geQaMJ4jpJ&branch=develop)](https://travis-ci.com/nervosnetwork/ckb)
[![Telegram Group](https://cdn.rawgit.com/Patrolavia/telegram-badge/8fe3382b/chat.svg)](https://t.me/nervos_ckb_dev)

---

## About CKB

CKB is the layer 1 of Nervos Network, a public blockchain with PoW, improved Nakamoto concensus, cell model and a new programming model for dApps.

[Nervos project](https://www.nervos.org) defines [a suite of scalable and interoperable blockchain protocols](https://github.com/nervosnetwork/rfcs) to create a self-evolving distributed economy, [CKB](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0002-ckb/0002-ckb.md) is among them.

**Notice**: The ckb process will send stack trace to sentry on Rust panics.
This is enabled by default before mainnet, which can be opted out by setting
the option `dsn` to empty in the config file.

## Testnet Rylai

CKB Testnet *Rylai* is the testing network before its mainnet launch. At this time Rylai will be **RESET** every two weeks.

## Mainnet

TBD.

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

- [Get CKB](docs/get-ckb.md)
- [Quick Start](docs/quick-start.md)
- [Run CKB with Docker](docs/run-ckb-with-docker.md)
- [Configure CKB](docs/configure.md)
- [CKB Core Development](docs/ckb-core-dev.md)

You can find a more comprehensive document website at [https://docs.nervos.org](https://docs.nervos.org).
