use crate::bytes::JsonBytes;
use crate::{
    BlockNumber, Byte32, Capacity, Cycle, EpochNumber, EpochNumberWithFraction, ProposalShortId,
    Timestamp, Uint128, Uint32, Uint64, Version,
};
use ckb_types::{core, packed, prelude::*, H256};
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::fmt;

/// Specifies how the script `code_hash` is used to match the script code and how to run the code.
///
/// Allowed kinds: "data", "type" and "data1".
///
/// Refer to the section [Code Locating](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0022-transaction-structure/0022-transaction-structure.md#code-locating)
/// and [Upgradable Script](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0022-transaction-structure/0022-transaction-structure.md#upgradable-script)
/// in the RFC *CKB Transaction Structure*.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ScriptHashType {
    /// Type "data" matches script code via cell data hash, and run the script code in v0 CKB VM.
    Data = 0,
    /// Type "type" matches script code via cell type script hash.
    Type = 1,
    /// Type "data" matches script code via cell data hash, and run the script code in v1 CKB VM.
    Data1 = 2,
}

impl Default for ScriptHashType {
    fn default() -> Self {
        ScriptHashType::Data
    }
}

impl From<ScriptHashType> for core::ScriptHashType {
    fn from(json: ScriptHashType) -> Self {
        match json {
            ScriptHashType::Data => core::ScriptHashType::Data,
            ScriptHashType::Type => core::ScriptHashType::Type,
            ScriptHashType::Data1 => core::ScriptHashType::Data1,
        }
    }
}

impl From<core::ScriptHashType> for ScriptHashType {
    fn from(core: core::ScriptHashType) -> ScriptHashType {
        match core {
            core::ScriptHashType::Data => ScriptHashType::Data,
            core::ScriptHashType::Type => ScriptHashType::Type,
            core::ScriptHashType::Data1 => ScriptHashType::Data1,
        }
    }
}

impl fmt::Display for ScriptHashType {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self {
            Self::Data => write!(f, "data"),
            Self::Type => write!(f, "type"),
            Self::Data1 => write!(f, "data1"),
        }
    }
}

/// Describes the lock script and type script for a cell.
///
/// ## Examples
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::Script>(r#"
/// {
///   "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
///   "hash_type": "data",
///   "args": "0x"
/// }
/// # "#).unwrap();
/// ```
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct Script {
    /// The hash used to match the script code.
    pub code_hash: H256,
    /// Specifies how to use the `code_hash` to match the script code.
    pub hash_type: ScriptHashType,
    /// Arguments for script.
    pub args: JsonBytes,
}

impl From<Script> for packed::Script {
    fn from(json: Script) -> Self {
        let Script {
            args,
            code_hash,
            hash_type,
        } = json;
        let hash_type: core::ScriptHashType = hash_type.into();
        packed::Script::new_builder()
            .args(args.into_bytes().pack())
            .code_hash(code_hash.pack())
            .hash_type(hash_type.into())
            .build()
    }
}

impl From<packed::Script> for Script {
    fn from(input: packed::Script) -> Script {
        Script {
            code_hash: input.code_hash().unpack(),
            args: JsonBytes::from_bytes(input.args().unpack()),
            hash_type: core::ScriptHashType::try_from(input.hash_type())
                .expect("checked data")
                .into(),
        }
    }
}

/// The fields of an output cell except the cell data.
///
/// ## Examples
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::CellOutput>(r#"
/// {
///   "capacity": "0x2540be400",
///   "lock": {
///     "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
///     "hash_type": "data",
///     "args": "0x"
///   },
///   "type": null
/// }
/// # "#).unwrap();
/// ```
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct CellOutput {
    /// The cell capacity.
    ///
    /// The capacity of a cell is the value of the cell in Shannons. It is also the upper limit of
    /// the cell occupied storage size where every 100,000,000 Shannons give 1-byte storage.
    pub capacity: Capacity,
    /// The lock script.
    pub lock: Script,
    /// The optional type script.
    ///
    /// The JSON field name is "type".
    #[serde(rename = "type")]
    pub type_: Option<Script>,
}

impl From<packed::CellOutput> for CellOutput {
    fn from(input: packed::CellOutput) -> CellOutput {
        CellOutput {
            capacity: input.capacity().unpack(),
            lock: input.lock().into(),
            type_: input.type_().to_opt().map(Into::into),
        }
    }
}

impl From<CellOutput> for packed::CellOutput {
    fn from(json: CellOutput) -> Self {
        let CellOutput {
            capacity,
            lock,
            type_,
        } = json;
        let type_builder = packed::ScriptOpt::new_builder();
        let type_ = match type_ {
            Some(type_) => type_builder.set(Some(type_.into())),
            None => type_builder,
        }
        .build();
        packed::CellOutput::new_builder()
            .capacity(capacity.pack())
            .lock(lock.into())
            .type_(type_)
            .build()
    }
}

/// Reference to a cell via transaction hash and output index.
///
/// ## Examples
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::OutPoint>(r#"
/// {
///   "index": "0x0",
///   "tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
/// }
/// # "#).unwrap();
/// ```
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct OutPoint {
    /// Transaction hash in which the cell is an output.
    pub tx_hash: H256,
    /// The output index of the cell in the transaction specified by `tx_hash`.
    pub index: Uint32,
}

impl From<packed::OutPoint> for OutPoint {
    fn from(input: packed::OutPoint) -> OutPoint {
        let index: u32 = input.index().unpack();
        OutPoint {
            tx_hash: input.tx_hash().unpack(),
            index: index.into(),
        }
    }
}

impl From<OutPoint> for packed::OutPoint {
    fn from(json: OutPoint) -> Self {
        let OutPoint { tx_hash, index } = json;
        let index = index.value() as u32;
        packed::OutPoint::new_builder()
            .tx_hash(tx_hash.pack())
            .index(index.pack())
            .build()
    }
}

/// The input cell of a transaction.
///
/// ## Examples
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::CellInput>(r#"
/// {
///   "previous_output": {
///     "index": "0x0",
///     "tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
///   },
///   "since": "0x0"
/// }
/// # "#).unwrap();
/// ```
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct CellInput {
    /// Restrict when the transaction can be committed into the chain.
    ///
    /// See the RFC [Transaction valid since](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0017-tx-valid-since/0017-tx-valid-since.md).
    pub since: Uint64,
    /// Reference to the input cell.
    pub previous_output: OutPoint,
}

impl From<packed::CellInput> for CellInput {
    fn from(input: packed::CellInput) -> CellInput {
        CellInput {
            previous_output: input.previous_output().into(),
            since: input.since().unpack(),
        }
    }
}

impl From<CellInput> for packed::CellInput {
    fn from(json: CellInput) -> Self {
        let CellInput {
            previous_output,
            since,
        } = json;
        packed::CellInput::new_builder()
            .previous_output(previous_output.into())
            .since(since.pack())
            .build()
    }
}

/// The dep cell type. Allowed values: "code" and "dep_group".
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum DepType {
    /// Type "code".
    ///
    /// Use the cell itself as the dep cell.
    Code,
    /// Type "dep_group".
    ///
    /// The cell is a dep group which members are cells. These members are used as dep cells
    /// instead of the group itself.
    ///
    /// The dep group stores the array of `OutPoint`s serialized via molecule in the cell data.
    /// Each `OutPoint` points to one cell member.
    DepGroup,
}

impl Default for DepType {
    fn default() -> Self {
        DepType::Code
    }
}

impl From<DepType> for core::DepType {
    fn from(json: DepType) -> Self {
        match json {
            DepType::Code => core::DepType::Code,
            DepType::DepGroup => core::DepType::DepGroup,
        }
    }
}

impl From<core::DepType> for DepType {
    fn from(core: core::DepType) -> DepType {
        match core {
            core::DepType::Code => DepType::Code,
            core::DepType::DepGroup => DepType::DepGroup,
        }
    }
}

/// The cell dependency of a transaction.
///
/// ## Examples
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::CellDep>(r#"
/// {
///   "dep_type": "code",
///   "out_point": {
///     "index": "0x0",
///     "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
///   }
/// }
/// # "#).unwrap();
/// ```
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct CellDep {
    /// Reference to the cell.
    pub out_point: OutPoint,
    /// Dependency type.
    pub dep_type: DepType,
}

impl From<packed::CellDep> for CellDep {
    fn from(input: packed::CellDep) -> Self {
        CellDep {
            out_point: input.out_point().into(),
            dep_type: core::DepType::try_from(input.dep_type())
                .expect("checked data")
                .into(),
        }
    }
}

impl From<CellDep> for packed::CellDep {
    fn from(json: CellDep) -> Self {
        let CellDep {
            out_point,
            dep_type,
        } = json;
        let dep_type: core::DepType = dep_type.into();
        packed::CellDep::new_builder()
            .out_point(out_point.into())
            .dep_type(dep_type.into())
            .build()
    }
}

/// The transaction.
///
/// Refer to RFC [CKB Transaction Structure](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0022-transaction-structure/0022-transaction-structure.md).
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct Transaction {
    /// Reserved for future usage. It must equal 0 in current version.
    pub version: Version,
    /// An array of cell deps.
    ///
    /// CKB locates lock script and type script code via cell deps. The script also can uses syscalls
    /// to read the cells here.
    ///
    /// Unlike inputs, the live cells can be used as cell deps in multiple transactions.
    pub cell_deps: Vec<CellDep>,
    /// An array of header deps.
    ///
    /// The block must already be in the canonical chain.
    ///
    /// Lock script and type script can read the header information of blocks listed here.
    pub header_deps: Vec<H256>,
    /// An array of input cells.
    ///
    /// In the canonical chain, any cell can only appear as an input once.
    pub inputs: Vec<CellInput>,
    /// An array of output cells.
    pub outputs: Vec<CellOutput>,
    /// Output cells data.
    ///
    /// This is a parallel array of outputs. The cell capacity, lock, and type of the output i is
    /// `outputs[i]` and its data is `outputs_data[i]`.
    pub outputs_data: Vec<JsonBytes>,
    /// An array of variable-length binaries.
    ///
    /// Lock script and type script can read data here to verify the transaction.
    ///
    /// For example, the bundled secp256k1 lock script requires storing the signature in
    /// `witnesses`.
    pub witnesses: Vec<JsonBytes>,
}

/// The JSON view of a Transaction.
///
/// This structure is serialized into a JSON object with field `hash` and all the fields in
/// [`Transaction`](struct.Transaction.html).
///
/// ## Examples
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::TransactionView>(r#"
/// {
///   "cell_deps": [
///     {
///       "dep_type": "code",
///       "out_point": {
///         "index": "0x0",
///         "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
///       }
///     }
///   ],
///   "hash": "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3",
///   "header_deps": [
///     "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed"
///   ],
///   "inputs": [
///     {
///       "previous_output": {
///         "index": "0x0",
///         "tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
///       },
///       "since": "0x0"
///     }
///   ],
///   "outputs": [
///     {
///       "capacity": "0x2540be400",
///       "lock": {
///         "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
///         "hash_type": "data",
///         "args": "0x"
///       },
///       "type": null
///     }
///   ],
///   "outputs_data": [
///     "0x"
///   ],
///   "version": "0x0",
///   "witnesses": []
/// }
/// # "#).unwrap();
/// ```
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TransactionView {
    /// All the fields in `Transaction` are included in `TransactionView` in JSON.
    #[serde(flatten)]
    pub inner: Transaction,
    /// The transaction hash.
    pub hash: H256,
}

impl From<packed::Transaction> for Transaction {
    fn from(input: packed::Transaction) -> Self {
        let raw = input.raw();
        Self {
            version: raw.version().unpack(),
            cell_deps: raw.cell_deps().into_iter().map(Into::into).collect(),
            header_deps: raw
                .header_deps()
                .into_iter()
                .map(|d| Unpack::<H256>::unpack(&d))
                .collect(),
            inputs: raw.inputs().into_iter().map(Into::into).collect(),
            outputs: raw.outputs().into_iter().map(Into::into).collect(),
            outputs_data: raw.outputs_data().into_iter().map(Into::into).collect(),
            witnesses: input.witnesses().into_iter().map(Into::into).collect(),
        }
    }
}

impl From<core::TransactionView> for TransactionView {
    fn from(input: core::TransactionView) -> Self {
        Self {
            inner: input.data().into(),
            hash: input.hash().unpack(),
        }
    }
}

impl From<Transaction> for packed::Transaction {
    fn from(json: Transaction) -> Self {
        let Transaction {
            version,
            cell_deps,
            header_deps,
            inputs,
            outputs,
            witnesses,
            outputs_data,
        } = json;
        let raw = packed::RawTransaction::new_builder()
            .version(version.pack())
            .cell_deps(cell_deps.into_iter().map(Into::into).pack())
            .header_deps(header_deps.iter().map(Pack::pack).pack())
            .inputs(inputs.into_iter().map(Into::into).pack())
            .outputs(outputs.into_iter().map(Into::into).pack())
            .outputs_data(outputs_data.into_iter().map(Into::into).pack())
            .build();
        packed::Transaction::new_builder()
            .raw(raw)
            .witnesses(witnesses.into_iter().map(Into::into).pack())
            .build()
    }
}

/// The JSON view of a transaction as well as its status.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TransactionWithStatus {
    /// The transaction.
    pub transaction: Option<TransactionView>,
    /// The Transaction status.
    pub tx_status: TxStatus,
}

impl TransactionWithStatus {
    /// Build with pending status
    pub fn with_pending(tx: Option<core::TransactionView>) -> Self {
        Self {
            tx_status: TxStatus::pending(),
            transaction: tx.map(Into::into),
        }
    }

    /// Build with proposed status
    pub fn with_proposed(tx: Option<core::TransactionView>) -> Self {
        Self {
            tx_status: TxStatus::proposed(),
            transaction: tx.map(Into::into),
        }
    }

    /// Build with committed status
    pub fn with_committed(tx: Option<core::TransactionView>, hash: H256) -> Self {
        Self {
            tx_status: TxStatus::committed(hash),
            transaction: tx.map(Into::into),
        }
    }

    /// Build with rejected status
    pub fn with_rejected(reason: String) -> Self {
        Self {
            tx_status: TxStatus::rejected(reason),
            transaction: None,
        }
    }

    /// Build with rejected status
    pub fn with_unknown() -> Self {
        Self {
            tx_status: TxStatus::unknown(),
            transaction: None,
        }
    }

    /// Build with status only
    pub fn status_only(tx_status: TxStatus) -> Self {
        Self {
            tx_status,
            transaction: None,
        }
    }

    /// Returns true if the tx_status is Unknown.
    pub fn is_unknown(&self) -> bool {
        self.tx_status.is_unknown()
    }
}

/// Status for transaction
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Status "pending". The transaction is in the pool, and not proposed yet.
    Pending,
    /// Status "proposed". The transaction is in the pool and has been proposed.
    Proposed,
    /// Status "committed". The transaction has been committed to the canonical chain.
    Committed,
    /// Status "Unknown". The node has not seen the transaction,
    /// or it should be rejected but was cleared due to storage limitations.
    Unknown,
    /// Status "Rejected". The transaction has been recently removed from the pool.
    /// Due to storage limitations, the node can only hold the most recently removed transactions.
    Rejected,
}

/// Transaction status and the block hash if it is committed.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TxStatus {
    /// The transaction status, allowed values: "pending", "proposed" and "committed".
    pub status: Status,
    /// The block hash of the block which has committed this transaction in the canonical chain.
    pub block_hash: Option<H256>,
    /// The reason why the transaction is rejected
    pub reason: Option<String>,
}

impl TxStatus {
    /// Pending transaction which is in the memory pool and must be proposed first.
    pub fn pending() -> Self {
        Self {
            status: Status::Pending,
            block_hash: None,
            reason: None,
        }
    }

    /// Transaction which has been proposed but not committed yet.
    pub fn proposed() -> Self {
        Self {
            status: Status::Proposed,
            block_hash: None,
            reason: None,
        }
    }

    /// Transaction which has already been commited.
    ///
    /// ## Params
    ///
    /// * `hash` - the block hash in which the transaction is committed.
    pub fn committed(hash: H256) -> Self {
        Self {
            status: Status::Committed,
            block_hash: Some(hash),
            reason: None,
        }
    }

    /// Transaction which has already been rejected recently.
    ///
    /// ## Params
    ///
    /// * `reason` - the reason why the transaction is rejected.
    pub fn rejected(reason: String) -> Self {
        Self {
            status: Status::Rejected,
            block_hash: None,
            reason: Some(reason),
        }
    }

    /// The node has not seen the transaction,
    pub fn unknown() -> Self {
        Self {
            status: Status::Unknown,
            block_hash: None,
            reason: None,
        }
    }

    /// Returns true if the status is Unknown.
    pub fn is_unknown(&self) -> bool {
        matches!(self.status, Status::Unknown)
    }
}

/// The block header.
///
/// Refer to RFC [CKB Block Structure](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0027-block-structure/0027-block-structure.md).
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct Header {
    /// The block version.
    ///
    /// It must equal to 0 now and is reserved for future upgrades.
    pub version: Version,
    /// The block difficulty target.
    ///
    /// It can be converted to a 256-bit target. Miners must ensure the Eaglesong of the header is
    /// within the target.
    pub compact_target: Uint32,
    /// The block timestamp.
    ///
    /// It is a Unix timestamp in milliseconds (1 second = 1000 milliseconds).
    ///
    /// Miners should put the time when the block is created in the header, however, the precision
    /// is not guaranteed. A block with a higher block number may even have a smaller timestamp.
    pub timestamp: Timestamp,
    /// The consecutive block number starting from 0.
    pub number: BlockNumber,
    /// The epoch information of this block.
    ///
    /// See `EpochNumberWithFraction` for details.
    pub epoch: EpochNumberWithFraction,
    /// The header hash of the parent block.
    pub parent_hash: H256,
    /// The commitment to all the transactions in the block.
    ///
    /// It is a hash on two Merkle Tree roots:
    ///
    /// * The root of a CKB Merkle Tree, which items are the transaction hashes of all the transactions in the block.
    /// * The root of a CKB Merkle Tree, but the items are the transaction witness hashes of all the transactions in the block.
    pub transactions_root: H256,
    /// The hash on `proposals` in the block body.
    ///
    /// It is all zeros when `proposals` is empty, or the hash on all the bytes concatenated together.
    pub proposals_hash: H256,
    /// The hash on `uncles` and extension in the block body.
    ///
    /// The uncles hash is all zeros when `uncles` is empty, or the hash on all the uncle header hashes concatenated together.
    /// The extension hash is the hash of the extension.
    /// The extra hash is the hash on uncles hash and extension hash concatenated together.
    ///
    /// # Notice
    ///
    /// This field is renamed from `uncles_hash` since 0.100.0.
    /// More details can be found in [CKB RFC 0031].
    ///
    /// [CKB RFC 0031]: https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0031-variable-length-header-field/0031-variable-length-header-field.md
    pub extra_hash: H256,
    /// DAO fields.
    ///
    /// See RFC [Deposit and Withdraw in Nervos DAO](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0023-dao-deposit-withdraw/0023-dao-deposit-withdraw.md#calculation).
    pub dao: Byte32,
    /// Miner can modify this field to find a proper value such that the Eaglesong of the header is
    /// within the target encoded from `compact_target`.
    pub nonce: Uint128,
}

/// The JSON view of a Header.
///
/// This structure is serialized into a JSON object with field `hash` and all the fields in
/// [`Header`](struct.Header.html).
///
/// ## Examples
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::HeaderView>(r#"
/// {
///   "compact_target": "0x1e083126",
///   "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
///   "epoch": "0x7080018000001",
///   "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
///   "nonce": "0x0",
///   "number": "0x400",
///   "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
///   "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
///   "timestamp": "0x5cd2b117",
///   "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
///   "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
///   "version": "0x0"
/// }
/// # "#).unwrap();
/// ```
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct HeaderView {
    /// All the fields in `Header` are included in `HeaderView` in JSON.
    #[serde(flatten)]
    pub inner: Header,
    /// The header hash. It is also called the block hash.
    pub hash: H256,
}

impl From<packed::Header> for Header {
    fn from(input: packed::Header) -> Self {
        let raw = input.raw();
        Self {
            version: raw.version().unpack(),
            parent_hash: raw.parent_hash().unpack(),
            timestamp: raw.timestamp().unpack(),
            number: raw.number().unpack(),
            epoch: raw.epoch().unpack(),
            transactions_root: raw.transactions_root().unpack(),
            proposals_hash: raw.proposals_hash().unpack(),
            compact_target: raw.compact_target().unpack(),
            extra_hash: raw.extra_hash().unpack(),
            dao: raw.dao().into(),
            nonce: input.nonce().unpack(),
        }
    }
}

impl From<core::HeaderView> for HeaderView {
    fn from(input: core::HeaderView) -> Self {
        Self {
            inner: input.data().into(),
            hash: input.hash().unpack(),
        }
    }
}

impl From<HeaderView> for core::HeaderView {
    fn from(input: HeaderView) -> Self {
        let header: packed::Header = input.inner.into();
        header.into_view()
    }
}

impl From<Header> for packed::Header {
    fn from(json: Header) -> Self {
        let Header {
            version,
            parent_hash,
            timestamp,
            number,
            epoch,
            transactions_root,
            proposals_hash,
            compact_target,
            extra_hash,
            dao,
            nonce,
        } = json;
        let raw = packed::RawHeader::new_builder()
            .version(version.pack())
            .parent_hash(parent_hash.pack())
            .timestamp(timestamp.pack())
            .number(number.pack())
            .epoch(epoch.pack())
            .transactions_root(transactions_root.pack())
            .proposals_hash(proposals_hash.pack())
            .compact_target(compact_target.pack())
            .extra_hash(extra_hash.pack())
            .dao(dao.into())
            .build();
        packed::Header::new_builder()
            .raw(raw)
            .nonce(nonce.pack())
            .build()
    }
}

/// The uncle block used as a parameter in the RPC.
///
/// The chain stores only the uncle block header and proposal IDs. The header ensures the block is
/// covered by PoW and can pass the consensus rules on uncle blocks. Proposal IDs are there because
/// a block can commit transactions proposed in an uncle.
///
/// A block B1 is considered to be the uncle of another block B2 if all the following conditions are met:
///
/// 1. They are in the same epoch, sharing the same difficulty;
/// 2. B2 block number is larger than B1;
/// 3. B1's parent is either B2's ancestor or an uncle embedded in B2 or any of B2's ancestors.
/// 4. B2 is the first block in its chain to refer to B1.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct UncleBlock {
    /// The uncle block header.
    pub header: Header,
    /// Proposal IDs in the uncle block body.
    pub proposals: Vec<ProposalShortId>,
}

/// The uncle block.
///
/// The chain stores only the uncle block header and proposal IDs. The header ensures the block is
/// covered by PoW and can pass the consensus rules on uncle blocks. Proposal IDs are there because
/// a block can commit transactions proposed in an uncle.
///
/// A block B1 is considered to be the uncle of another block B2 if all the following conditions are met:
///
/// 1. They are in the same epoch, sharing the same difficulty;
/// 2. B2 block number is larger than B1;
/// 3. B1's parent is either B2's ancestor or an uncle embedded in B2 or any of B2's ancestors.
/// 4. B2 is the first block in its chain to refer to B1.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct UncleBlockView {
    /// The uncle block header.
    pub header: HeaderView,
    /// Proposal IDs in the uncle block body.
    pub proposals: Vec<ProposalShortId>,
}

impl From<packed::UncleBlock> for UncleBlock {
    fn from(input: packed::UncleBlock) -> Self {
        Self {
            header: input.header().into(),
            proposals: input.proposals().into_iter().map(Into::into).collect(),
        }
    }
}

impl From<core::UncleBlockView> for UncleBlockView {
    fn from(input: core::UncleBlockView) -> Self {
        let header = HeaderView {
            inner: input.data().header().into(),
            hash: input.hash().unpack(),
        };
        Self {
            header,
            proposals: input
                .data()
                .proposals()
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

impl From<UncleBlock> for packed::UncleBlock {
    fn from(json: UncleBlock) -> Self {
        let UncleBlock { header, proposals } = json;
        packed::UncleBlock::new_builder()
            .header(header.into())
            .proposals(proposals.into_iter().map(Into::into).pack())
            .build()
    }
}

/// The JSON view of a Block used as a parameter in the RPC.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct Block {
    /// The block header.
    pub header: Header,
    /// The uncles blocks in the block body.
    pub uncles: Vec<UncleBlock>,
    /// The transactions in the block body.
    pub transactions: Vec<Transaction>,
    /// The proposal IDs in the block body.
    pub proposals: Vec<ProposalShortId>,
    /// The extension in the block body.
    ///
    /// This field is optional. It's a reserved field, please leave it blank.
    #[doc(hidden)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<JsonBytes>,
}

/// The JSON view of a Block including header and body.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BlockView {
    /// The block header.
    pub header: HeaderView,
    /// The uncles blocks in the block body.
    pub uncles: Vec<UncleBlockView>,
    /// The transactions in the block body.
    pub transactions: Vec<TransactionView>,
    /// The proposal IDs in the block body.
    pub proposals: Vec<ProposalShortId>,
    /// The extension in the block body.
    ///
    /// This field is optional. It's a reserved field, please leave it blank.
    #[doc(hidden)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<JsonBytes>,
}

impl From<packed::Block> for Block {
    fn from(input: packed::Block) -> Self {
        Self {
            header: input.header().into(),
            uncles: input.uncles().into_iter().map(Into::into).collect(),
            transactions: input.transactions().into_iter().map(Into::into).collect(),
            proposals: input.proposals().into_iter().map(Into::into).collect(),
            extension: input.extension().map(Into::into),
        }
    }
}

impl From<core::BlockView> for BlockView {
    fn from(input: core::BlockView) -> Self {
        let block = input.data();
        let header = HeaderView {
            inner: block.header().into(),
            hash: input.hash().unpack(),
        };
        let uncles = block
            .uncles()
            .into_iter()
            .zip(input.uncle_hashes().into_iter())
            .map(|(uncle, hash)| {
                let header = HeaderView {
                    inner: uncle.header().into(),
                    hash: hash.unpack(),
                };
                UncleBlockView {
                    header,
                    proposals: uncle.proposals().into_iter().map(Into::into).collect(),
                }
            })
            .collect();
        let transactions = block
            .transactions()
            .into_iter()
            .zip(input.tx_hashes().iter())
            .map(|(tx, hash)| TransactionView {
                inner: tx.into(),
                hash: hash.unpack(),
            })
            .collect();
        Self {
            header,
            uncles,
            transactions,
            proposals: block.proposals().into_iter().map(Into::into).collect(),
            extension: block.extension().map(Into::into),
        }
    }
}

impl From<Block> for packed::Block {
    fn from(json: Block) -> Self {
        let Block {
            header,
            uncles,
            transactions,
            proposals,
            extension,
        } = json;
        if let Some(extension) = extension {
            let extension: packed::Bytes = extension.into();
            packed::BlockV1::new_builder()
                .header(header.into())
                .uncles(uncles.into_iter().map(Into::into).pack())
                .transactions(transactions.into_iter().map(Into::into).pack())
                .proposals(proposals.into_iter().map(Into::into).pack())
                .extension(extension)
                .build()
                .as_v0()
        } else {
            packed::Block::new_builder()
                .header(header.into())
                .uncles(uncles.into_iter().map(Into::into).pack())
                .transactions(transactions.into_iter().map(Into::into).pack())
                .proposals(proposals.into_iter().map(Into::into).pack())
                .build()
        }
    }
}

impl From<BlockView> for core::BlockView {
    fn from(input: BlockView) -> Self {
        let BlockView {
            header,
            uncles,
            transactions,
            proposals,
            extension,
        } = input;
        let block = Block {
            header: header.inner,
            uncles: uncles
                .into_iter()
                .map(|u| {
                    let UncleBlockView { header, proposals } = u;
                    UncleBlock {
                        header: header.inner,
                        proposals,
                    }
                })
                .collect(),
            transactions: transactions.into_iter().map(|tx| tx.inner).collect(),
            proposals,
            extension,
        };
        let block: packed::Block = block.into();
        block.into_view()
    }
}

/// JSON view of an epoch.
///
/// CKB adjusts difficulty based on epochs.
///
/// ## Examples
///
/// ```
/// # serde_json::from_str::<ckb_jsonrpc_types::EpochView>(r#"
/// {
///   "compact_target": "0x1e083126",
///   "length": "0x708",
///   "number": "0x1",
///   "start_number": "0x3e8"
/// }
/// # "#).unwrap();
/// ```
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct EpochView {
    /// Consecutive epoch number starting from 0.
    pub number: EpochNumber,
    /// The block number of the first block in the epoch.
    ///
    /// It also equals the total count of blocks in all the epochs which epoch number is
    /// less than this epoch.
    pub start_number: BlockNumber,
    /// The number of blocks in this epoch.
    pub length: BlockNumber,
    /// The difficulty target for any block in this epoch.
    pub compact_target: Uint32,
}

impl EpochView {
    /// Creates the view from the stored ext.
    pub fn from_ext(ext: packed::EpochExt) -> EpochView {
        EpochView {
            number: ext.number().unpack(),
            start_number: ext.start_number().unpack(),
            length: ext.length().unpack(),
            compact_target: ext.compact_target().unpack(),
        }
    }
}

/// Block base rewards.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BlockIssuance {
    /// The primary base rewards.
    pub primary: Capacity,
    /// The secondary base rewards.
    pub secondary: Capacity,
}

impl From<core::BlockIssuance> for BlockIssuance {
    fn from(core: core::BlockIssuance) -> Self {
        Self {
            primary: core.primary.into(),
            secondary: core.secondary.into(),
        }
    }
}

impl From<BlockIssuance> for core::BlockIssuance {
    fn from(json: BlockIssuance) -> Self {
        Self {
            primary: json.primary.into(),
            secondary: json.secondary.into(),
        }
    }
}

/// Block rewards for miners.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct MinerReward {
    /// The primary base block reward allocated to miners.
    pub primary: Capacity,
    /// The secondary base block reward allocated to miners.
    pub secondary: Capacity,
    /// The transaction fees that are rewarded to miners because the transaction is committed in the block.
    ///
    /// Miners get 60% of the transaction fee for each transaction committed in the block.
    pub committed: Capacity,
    /// The transaction fees that are rewarded to miners because the transaction is proposed in the block or
    /// its uncles.
    ///
    /// Miners get 40% of the transaction fee for each transaction proposed in the block and
    /// committed later in its active commit window.
    pub proposal: Capacity,
}

impl From<core::MinerReward> for MinerReward {
    fn from(core: core::MinerReward) -> Self {
        Self {
            primary: core.primary.into(),
            secondary: core.secondary.into(),
            committed: core.committed.into(),
            proposal: core.proposal.into(),
        }
    }
}

impl From<MinerReward> for core::MinerReward {
    fn from(json: MinerReward) -> Self {
        Self {
            primary: json.primary.into(),
            secondary: json.secondary.into(),
            committed: json.committed.into(),
            proposal: json.proposal.into(),
        }
    }
}

/// Block Economic State.
///
/// It includes the rewards details and when it is finalized.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BlockEconomicState {
    /// Block base rewards.
    pub issuance: BlockIssuance,
    /// Block rewards for miners.
    pub miner_reward: MinerReward,
    /// The total fees of all transactions committed in the block.
    pub txs_fee: Capacity,
    /// The block hash of the block which creates the rewards as cells in its cellbase transaction.
    pub finalized_at: H256,
}

impl From<core::BlockEconomicState> for BlockEconomicState {
    fn from(core: core::BlockEconomicState) -> Self {
        Self {
            issuance: core.issuance.into(),
            miner_reward: core.miner_reward.into(),
            txs_fee: core.txs_fee.into(),
            finalized_at: core.finalized_at.unpack(),
        }
    }
}

impl From<BlockEconomicState> for core::BlockEconomicState {
    fn from(json: BlockEconomicState) -> Self {
        Self {
            issuance: json.issuance.into(),
            miner_reward: json.miner_reward.into(),
            txs_fee: json.txs_fee.into(),
            finalized_at: json.finalized_at.pack(),
        }
    }
}

/// Merkle proof for transactions in a block.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TransactionProof {
    /// Block hash
    pub block_hash: H256,
    /// Merkle root of all transactions' witness hash
    pub witnesses_root: H256,
    /// Merkle proof of all transactions' hash
    pub proof: MerkleProof,
}

/// Proof of CKB Merkle Tree.
///
/// CKB Merkle Tree is a [CBMT](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0006-merkle-tree/0006-merkle-tree.md) using CKB blake2b hash as the merge function.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct MerkleProof {
    /// Leaves indices in the CBMT that are proved present in the block.
    ///
    /// These are indices in the CBMT tree not the transaction indices in the block.
    pub indices: Vec<Uint32>,
    /// Hashes of all siblings along the paths to root.
    pub lemmas: Vec<H256>,
}

/// Two protocol parameters `closest` and `farthest` define the closest
/// and farthest on-chain distance between a transaction's proposal
/// and commitment.
///
/// A non-cellbase transaction is committed at height h_c if all of the following conditions are met:
/// 1) it is proposed at height h_p of the same chain, where w_close <= h_c − h_p <= w_far ;
/// 2) it is in the commitment zone of the main chain block with height h_c ;
///
/// ```text
///   ProposalWindow { closest: 2, farthest: 10 }
///       propose
///          \
///           \
///           13 14 [15 16 17 18 19 20 21 22 23]
///                  \_______________________/
///                               \
///                             commit
/// ```
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub struct ProposalWindow {
    /// The closest distance between the proposal and the commitment.
    pub closest: BlockNumber,
    /// The farthest distance between the proposal and the commitment.
    pub farthest: BlockNumber,
}

/// Consensus defines various parameters that influence chain consensus
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Consensus {
    /// Names the network.
    pub id: String,
    /// The genesis block hash
    pub genesis_hash: H256,
    /// The dao type hash
    pub dao_type_hash: Option<H256>,
    /// The secp256k1_blake160_sighash_all_type_hash
    pub secp256k1_blake160_sighash_all_type_hash: Option<H256>,
    /// The secp256k1_blake160_multisig_all_type_hash
    pub secp256k1_blake160_multisig_all_type_hash: Option<H256>,
    /// The initial primary_epoch_reward
    pub initial_primary_epoch_reward: Capacity,
    /// The secondary primary_epoch_reward
    pub secondary_epoch_reward: Capacity,
    /// The maximum amount of uncles allowed for a block
    pub max_uncles_num: Uint64,
    /// The expected orphan_rate
    pub orphan_rate_target: core::RationalU256,
    /// The expected epoch_duration
    pub epoch_duration_target: Uint64,
    /// The two-step-transaction-confirmation proposal window
    pub tx_proposal_window: ProposalWindow,
    /// The two-step-transaction-confirmation proposer reward ratio
    pub proposer_reward_ratio: core::RationalU256,
    /// The Cellbase maturity
    pub cellbase_maturity: EpochNumberWithFraction,
    /// This parameter indicates the count of past blocks used in the median time calculation
    pub median_time_block_count: Uint64,
    /// Maximum cycles that all the scripts in all the commit transactions can take
    pub max_block_cycles: Cycle,
    /// Maximum number of bytes to use for the entire block
    pub max_block_bytes: Uint64,
    /// The block version number supported
    pub block_version: Version,
    /// The tx version number supported
    pub tx_version: Version,
    /// The "TYPE_ID" in hex
    pub type_id_code_hash: H256,
    /// The Limit to the number of proposals per block
    pub max_block_proposals_limit: Uint64,
    /// Primary reward is cut in half every halving_interval epoch
    pub primary_epoch_reward_halving_interval: Uint64,
    /// Keep difficulty be permanent if the pow is dummy
    pub permanent_difficulty_in_dummy: bool,
    /// Hardfork features
    pub hardfork_features: Vec<HardForkFeature>,
}

/// The information about one hardfork feature.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct HardForkFeature {
    /// The related RFC ID.
    pub rfc: String,
    /// The first epoch when the feature is enabled.
    pub epoch_number: Option<EpochNumber>,
}

fn convert(number: core::EpochNumber) -> Option<EpochNumber> {
    if number == core::EpochNumber::MAX {
        None
    } else {
        Some(number.into())
    }
}

impl HardForkFeature {
    /// Creates a new struct.
    pub fn new(rfc: &str, epoch_number: Option<EpochNumber>) -> Self {
        Self {
            rfc: rfc.to_owned(),
            epoch_number,
        }
    }

    /// Returns a list of hardfork features from a hardfork switch.
    pub fn load_list_from_switch(switch: &core::hardfork::HardForkSwitch) -> Vec<Self> {
        vec![
            Self::new("0028", convert(switch.rfc_0028())),
            Self::new("0029", convert(switch.rfc_0029())),
            Self::new("0030", convert(switch.rfc_0030())),
            Self::new("0031", convert(switch.rfc_0031())),
            Self::new("0032", convert(switch.rfc_0032())),
            Self::new("0036", convert(switch.rfc_0036())),
            Self::new("0038", convert(switch.rfc_0038())),
        ]
    }
}
