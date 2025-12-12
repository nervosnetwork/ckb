# ckb-hash

This crate is a component of [ckb](https://github.com/nervosnetwork/ckb).

CKB default hash function.

## Usage

If used in ***On-Chain Script***, you need to disable the default features and enable `ckb-contract`.
```
default-features = false, features = ["ckb-contract"]
```

Example:
```rust
use ckb_hash::blake2b_256;

let input = b"ckb";
let digest = blake2b_256(&input);
println!("ckbhash({:?}) = {:?}", input, digest);
```
