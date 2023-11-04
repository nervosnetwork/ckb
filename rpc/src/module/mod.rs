//! CKB RPC modules
//!
//! This RPC document is generated by Rust Doc, so it will take some concept conversions to map
//! from the Rust structures to the JSONRPC.
//!
//! ## JSONRPC Methods
//!
//! The section [Traits](#traits) lists all the RPC modules. CKB allows enabling and disabling RPC
//! methods by modules. The default enabled ones are enabled modules are "Net", "Pool", "Miner",
//! "Chain", "Stats", "Subscription", "Experiment". As you can see, the `Rpc` suffix is removed in
//! the config file.
//!
//! The section *Required methods* lists all the RPC methods in the module. See module
//! [PoolRpc](trait.PoolRpc.html#required-methods).
//!
//! Use the RPC [`send_transaction`](trait.PoolRpc.html#tymethod.send_transaction) in the module `PoolRpc` as an example.
//!
//! ```text
//! fn send_transaction(
//!    ^^^^^^^^^^^^^^^^
//!                `-- JSONRPC method name
//!     &self,
//!     ^^^^^
//!       `-- ignore this
//!
//!   ,--------------------------------------------
//!   | tx: Transaction,
//!   | outputs_validator: Option<OutputsValidator>
//!   `--------------------------------------------
//!       `-- Request params list as pairs of "name: Type"
//!
//! ) -> Result<H256>;
//!             ^^^^
//!              `-- Response Type
//! ```
//!
//! * `send_transaction` - The JSONRPC method name.
//! * `tx: Transaction` - The first param in the request params list which name is `tx` and type is `Transaction`. The type links to the JSON object definition of a CKB transaction.
//! * `outputs_validator: Option<OutputsValidator>` - The second param. The `Option` shows that this argument is optional. The document for `OutputsValidator` shows that `outputs_validator` is an enum type which possible values include "well_known_scripts_only" and "passthrough".
//! * `-> Result<H256>` - The type inside the `Result` after `->` is the response type. In this example, it is `H256` which is a 32-bytes binary encoded as a hex string.
//!
//! The RPC errors are documented in [`RPCError`](../enum.RPCError.html).
//!
//! ## JSONRPC Deprecation Process
//!
//! A CKB RPC method is deprecated in three steps.
//!
//! First, the method is marked as deprecated in the CKB release notes and RPC document. However,
//! the RPC method is still available. The RPC document will have the suggestion of alternative
//! solutions.
//!
//! The CKB dev team will disable any deprecated RPC methods starting from the next minor version
//! release. Users can enable the deprecated methods via the config file option `rpc.enable_deprecated_rpc`.
//!
//! Once a deprecated method is disabled, the CKB dev team will remove it in a future minor version release.
//!
//! For example, a method is marked as deprecated in 0.35.0, it can be disabled in 0.36.0 and
//! removed in 0.37.0. The minor versions are released monthly, so there's at least a two-month
//! buffer for a deprecated RPC method.
//!
//! ## JSON Cheatsheet
//!
//! CKB uses a framework to serialize into and deserialize from JSON. Some Rust std-lib
//! structures will be used in requests and responses. The following cheatsheet shows how to
//! map them into JSON values.
//!
//! | Rust        | JSON                 |
//! | ----------- | -------------------- |
//! | `()`        | `null`               |
//! | `bool`      | `boolean`            |
//! | `String`    | `string`             |
//! | `Option<T>` | either `null` or `T` |
//! | `Vec<T>`    | array of `T`         |
//!
//! CKB RPC does not use JSON numbers because of the precision problem. Float point numbers are not
//! used in the RPC, and integers are encoded as 0x-prefixed hex string such as `0x10` for decimal
//! value 16.
//!
//! The other types will have their own documentation pages. Unless the JSON format is explicitly
//! described in the documentation page, the rust Struct is serialized as a JSON object, and Enum is
//! serialized as a JSON string.
//!
//! For example, `OutPoint` is a struct having the following fields
//!
//! ```text
//! tx_hash: H256
//! index: Uint32
//! ```
//!
//! An example `OutPoint` JSON looks like
//!
//! ```json
//! {
//!   "index": "0xffffffff",
//!    "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
//! }
//! ```
//!
//! `Status` is a Rust enum
//!
//! ```text
//! pub enum Status {
//!     Pending,
//!     Proposed,
//!     Committed,
//! }
//! ```
//!
//! The enum values are represented as JSON strings in the lowercase, underscore-concatenated form. So, in
//! JSON, `Status` can be one of "pending", "proposed" or "committed".
#![allow(deprecated)]

mod alert;
pub(crate) mod chain;
mod debug;
mod experiment;
mod indexer;
mod indexer_r;
mod miner;
mod net;
pub(crate) mod pool;
mod stats;
mod subscription;
mod test;

pub(crate) use self::alert::AlertRpcImpl;
pub(crate) use self::chain::ChainRpcImpl;
pub(crate) use self::debug::DebugRpcImpl;
pub(crate) use self::experiment::ExperimentRpcImpl;
pub(crate) use self::indexer::IndexerRpcImpl;
pub(crate) use self::indexer_r::IndexerRRpcImpl;
pub(crate) use self::miner::MinerRpcImpl;
pub(crate) use self::net::NetRpcImpl;
pub(crate) use self::pool::PoolRpcImpl;
pub(crate) use self::stats::StatsRpcImpl;
pub(crate) use self::subscription::{SubscriptionRpcImpl, SubscriptionSession};
pub(crate) use self::test::IntegrationTestRpcImpl;

pub use self::alert::AlertRpc;
pub use self::chain::ChainRpc;
pub use self::debug::DebugRpc;
pub use self::experiment::ExperimentRpc;
pub use self::indexer::IndexerRpc;
pub use self::indexer_r::IndexerRRpc;
pub use self::miner::MinerRpc;
pub use self::net::NetRpc;
pub use self::pool::PoolRpc;
pub use self::stats::StatsRpc;
pub use self::subscription::SubscriptionRpc;
pub use self::test::IntegrationTestRpc;
