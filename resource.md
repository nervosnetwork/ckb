# General

## [Nervos Network RFCs](https://github.com/nervosnetwork/rfcs)

These RFCs (Request for Comments) are proposals, standards, and documentation related to the Nervos Network.

## [Nervos Development Workshops](https://www.youtube.com/watch?v=3i0ElISMIoA&list=PLRke1-EE4VWF_LtJoj7jdsRqGLLc8jTke)

This video series provides the tools and knowledge you need to start building on Nervos. Our experienced developers will take you through the process of building on Nervos, step by step.

## [A Beginner's Guide To Nervos](https://www.notion.so/A-Beginner-s-Guide-To-Nervos-c3e0ed3fd9284263a033018ef554fa37)

This manual, created by the Nervos community, is designed to help newcomers get started with Nervos. It covers basic concepts of Nervos, such as tokenomics, wallets, NervosDAO, mining, sUDT, and includes videos focusing on the UTXO model. (This manual is also available in [Chinese language](https://www.notion.so/Nervos-6836c451287f44cfa7c4375102f8d778).)

# Tutorials

## [Learn CKB From Zero](https://zero2ckb.ckbapp.dev/learn)

An interactive web page to learn CKB from scratch, no code or software needed.

## [Construct and Send a Transaction via Replit in Java](https://blog.cryptape.com/construct-and-send-your-first-ckb-transaction)

An easy-to-follow tutorial on how to construct and send a transaction in CKB

## [L1 Developer Training Course](https://nervos.gitbook.io/developer-training-course/)

A course series that helps intermediate blockchain developers build common application components on Nervos CKB L1 through practical exercises.

## ****[DApps with CKB Workshop](https://www.youtube.com/watch?v=iVjccs3z5q0&list=PLRke1-EE4VWFirxtxtXmW7enINVZP2Lk6)****

A workshop series presented by our senior engineers on CKB dApp development. The series comprises three lectures: 1) basic concepts of CKB development, including several commonly used development patterns; 2) a demonstration of using Capsule, the tool for writing on-chain contracts in Rust; 3) a demonstration of using Lumos, another framework for building dApps in JavaScript or TypeScript, to interact with the blockchain.

## [Develop a CKB DApp with Kyper](https://www.youtube.com/watch?app=desktop&v=i-gQ0enK5cY)

A video tutorials that walk you through Keyper, a standard design for key management between wallets and dApps and a lock management SDK with plugin support.

# Tools

## [CKB-CLI](https://github.com/nervosnetwork/ckb-cli)

A command-line tool that allows you to perform various actions, such as managing accounts, [transferring CKBytes](https://docs.nervos.org/docs/basics/guides/devchain/#transferring-ckbytes-using-ckb-cli), and checking account balances, by directly invoking RPCs.

## [Capsule](https://github.com/nervosnetwork/capsule)

Capsule provides handy smart contract support for Rust developers when creating scripts on CKB, covering the entire lifecycle of script development: writing, debugging, testing, and deployment. Click [here](https://docs.nervos.org/docs/labs/sudtbycapsule/) to discover more details.

## [Lumos](https://github.com/ckb-js/lumos)

A JavaScript/TypeScript based toolbox designed to make building DApps on Nervos CKB easier. Follow the [Lumos Tutorial](https://lumos-website.vercel.app/) site for more information and step-by-step guidance.

## [Mercury](https://github.com/nervosnetwork/mercury)

A high-level middleware solution for simple integration use cases, such as wallets and exchanges.

## SDKs

SDKs provide basic and versatile support in the language of your choice.

### [Rust](https://github.com/nervosnetwork/ckb-sdk-rust)

### [Golang](https://github.com/nervosnetwork/ckb-sdk-go)

### [Java](https://github.com/nervosnetwork/ckb-sdk-java)

### [Javascript](https://lumos-website.vercel.app/)

# Cell Model

Cell is the most basic structure for representing a single piece of data in Nervos. For a better comprehension of this concept, in addition to the essays, [Cell](https://docs.nervos.org/docs/reference/cell/) and [Cell Model](https://docs.nervos.org/docs/basics/concepts/cell-model/), on the documentation site, here are a few more pieces you can read:

[Cell Model: A generalized UTXO as state storage](https://medium.com/nervosnetwork/https-medium-com-nervosnetwork-cell-model-7323fca57571)

[How CKB Turns User Defined Cryptos Into First-Class Assets](https://blog.cryptape.com/how-ckb-turns-user-defined-cryptos-into-first-class-assets)

[Understanding the Nervos DAO and Cell Model](https://medium.com/nervosnetwork/understanding-the-nervos-dao-and-cell-model-d68f38272c24)

# Consensus

## NC-Max: Breaking the Security-Performance Tradeoff in Nakamoto Consensus

An extended presentation by Dr. Ren Zhang elaborates on the innovative features of NC-Max, which uses NC (Nakamoto Consensus) as a backbone to achieve security, flexibility, and simplicity while breaking the security-performance tradeoff. The original talk was given at the NDSS Symposium in 2022.

Read the paper and watch the video [here](https://blog.cryptape.com/lay-down-the-common-metrics-evaluating-proof-of-work-consensus-protocols-security).

## ****[Nervos CKB - Consensus Mechanism](https://www.youtube.com/watch?v=HSXzbgVRH_M&list=PLRke1-EE4VWFMz-7sMYURt6woRLqmkDR6&index=6)****

Dr. Ren Zhang explains the design logic of Nervos consensus protocol — NC-Max, a variant of Nakamoto Consensus with higher throughput.

## ****[Three Major Innovations in NC-Max](https://www.youtube.com/watch?v=79vjzBXIb_g)****

An exploration into the three major innovations in NC-Max: 1) Two-step transaction confirmation to reduce orphan rate; 2) Dynamic block interval & block reward to maximize bandwidth utilization; 3) Consideration of all blocks in difficulty adjustment to defend against selfish

## ****[ZKPodcast: Testing PoW Consensus Algorithm Security with Ren Zhang from Nervos](https://www.youtube.com/watch?v=iJK_6BbLTAc)****

A podcast session hosted by ZKPoscast where Dr. Zhang Ren chats about an earlier work he did on evaluating PoW consensus protocol security and explore his more recent work on NC-Max.

# Layer 1

## ****[Nervos CKB, Layer 1 for Layer 2](https://www.youtube.com/watch?v=LJeSUXc_nfI&list=PLRke1-EE4VWFMz-7sMYURt6woRLqmkDR6&index=10)****

In this keynote speech, Jan, co-founder and chief architect of Nervos discusses the layered architecture needed to support scalability and decentralization. He argues that L1 must be powerful enough to support L2 solutions, and the ledger should be modeled from the perspective of assets rather than accounts, to maintain consistency between layers and reduce complexity.

[data:image/svg+xml,%3csvg%20xmlns=%27http://www.w3.org/2000/svg%27%20version=%271.1%27%20width=%2730%27%20height=%2730%27/%3e](data:image/svg+xml,%3csvg%20xmlns=%27http://www.w3.org/2000/svg%27%20version=%271.1%27%20width=%2730%27%20height=%2730%27/%3e)

## ****[Why building a new Layer 1 blockchain?](https://www.youtube.com/watch?v=F1nfrTsGJqk&list=PLRke1-EE4VWFMz-7sMYURt6woRLqmkDR6&index=6)****

Jan elaborates on the architecture of CKB, which is designed to be the best fit for its layered architecture. He also outlines the reasoning behind the adoption of cell model to store data and manage transactions, and how the total state size in CKB is bound by the token in circulation, which curbs state explosion and bloating, and other layered-architecture-related topics.

# CKB-VM

## [An Introduction to Nervos CKB-VM](https://medium.com/nervosnetwork/an-introduction-to-ckb-vm-9d95678a7757)

An essay from our engineer Xuejie who has expertise in the development of the CKB-VM.

## ****[Nervos CKB VM Walkthrough](https://www.youtube.com/watch?v=qUGU5_o5Lo4&list=PLRke1-EE4VWHFfYNXuQGYNy0V5NjxX1bf&index=14)****

Xuejie discusses the design logic behind CKB-VM and explains the rationale behind choosing RISC-V over WASM as the platform for building the VM.

## [RISC-V & Its Benefits to Blockchain](https://twitter.com/NervosNetwork/status/1638888542922493953?s=20)

A Twitter thread breaking down CKB’s RISC-V-based virtual machine and the benefits.

## [Dear Blockchain: Say Hello to RISC-V](https://www.youtube.com/watch?v=QHjmykiyT5Q&list=PLRke1-EE4VWFMz-7sMYURt6woRLqmkDR6&index=7)

An educational event introducing RISC-V to the Nervos community.

# NFT

## [NFT Standards](https://linktr.ee/NFT_Games)

A compilation of Nervos CKB NFT related links

# Wallet

## **[Third Party Wallets](https://linktr.ee/thirdpartywallets)**

A compilation of wallets that support storage of CKBytes.

# Update & AMA

## [Hashing it Out](https://www.youtube.com/watch?v=q6YK1q301Rw&list=PLRke1-EE4VWHlDLTmbd11FZTIO3VC4M_g)

A monthly in-depth and engaging program broadcasted on YouTube, where community members can connect with the co-founders and engineers of Nervos, making it a valuable opportunity to stay up-to-date with the ecosystem.
