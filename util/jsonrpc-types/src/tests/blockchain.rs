use ckb_types::{bytes::Bytes, core, h256, packed, prelude::*};
use lazy_static::lazy_static;
use proptest::{collection::size_range, prelude::*};
use regex::Regex;

use crate::{
    blockchain::{BlockView, Script, ScriptHashType},
    bytes::JsonBytes,
};

fn mock_script(arg: Bytes) -> packed::Script {
    packed::ScriptBuilder::default()
        .code_hash(packed::Byte32::zero())
        .args(arg.pack())
        .hash_type(core::ScriptHashType::Data.into())
        .build()
}

fn mock_cell_output(arg: Bytes) -> packed::CellOutput {
    packed::CellOutputBuilder::default()
        .capacity(core::Capacity::zero().pack())
        .lock(packed::Script::default())
        .type_(Some(mock_script(arg)).pack())
        .build()
}

fn mock_cell_input() -> packed::CellInput {
    packed::CellInput::new(packed::OutPoint::default(), 0)
}

fn mock_full_tx(data: Bytes, arg: Bytes) -> core::TransactionView {
    core::TransactionBuilder::default()
        .inputs(vec![mock_cell_input()])
        .outputs(vec![mock_cell_output(arg.clone())])
        .outputs_data(vec![data.pack()])
        .witness(arg.pack())
        .build()
}

fn mock_uncle() -> core::UncleBlockView {
    core::BlockBuilder::default()
        .proposals(vec![packed::ProposalShortId::default()].pack())
        .build()
        .as_uncle()
}

fn mock_full_block(data: Bytes, arg: Bytes) -> core::BlockView {
    core::BlockBuilder::default()
        .transactions(vec![mock_full_tx(data, arg)])
        .uncles(vec![mock_uncle()])
        .proposals(vec![packed::ProposalShortId::default()])
        .build()
}

#[test]
fn test_script_serialization() {
    for (original, entity) in &[
        (
            "{\
                \"code_hash\":\"0x00000000000000000000000000000000\
                                00000000000000000000000000000000\",\
                \"hash_type\":\"data\",\
                \"args\":\"0x\"\
            }",
            Script {
                code_hash: h256!("0x0"),
                hash_type: ScriptHashType::Data,
                args: JsonBytes::default(),
            },
        ),
        (
            "{\
                \"code_hash\":\"0x00000000000000000000000000000000\
                                00000000000000000000000000000000\",\
                \"hash_type\":\"type\",\
                \"args\":\"0x\"\
            }",
            Script {
                code_hash: h256!("0x0"),
                hash_type: ScriptHashType::Type,
                args: JsonBytes::default(),
            },
        ),
        (
            "{\
                \"code_hash\":\"0x00000000000000000000000000000000\
                                  00000000000000000000000000000001\",\
                \"hash_type\":\"data1\",\
                \"args\":\"0x\"\
            }",
            Script {
                code_hash: h256!("0x1"),
                hash_type: ScriptHashType::Data1,
                args: JsonBytes::default(),
            },
        ),
    ] {
        let decoded: Script = serde_json::from_str(original).unwrap();
        assert_eq!(&decoded, entity);
        let encoded = serde_json::to_string(&decoded).unwrap();
        assert_eq!(&encoded, original);
    }
    for malformed in &[
        "{\
            \"code_hash\":\"0x00000000000000000000000000000000\
                            00000000000000000000000000000000\",\
            \"args\":\"0x\"\
        }",
        "{\
            \"code_hash\":\"0x00000000000000000000000000000000\
                            00000000000000000000000000000000\",\
            \"hash_type\":null,\
            \"args\":\"0x\"\
        }",
        "{\
            \"code_hash\":\"0x00000000000000000000000000000000\
                            00000000000000000000000000000000\",\
            \"hash_type\":type,\
            \"args\":\"0x\"\
        }",
        "{\
            \"code_hash\":\"0x00000000000000000000000000000000\
                            00000000000000000000000000000000\",\
            \"hash_type\":\"data2\",\
            \"args\":\"0x\"\
        }",
        "{\
            \"code_hash\":\"0x00000000000000000000000000000000\
                            00000000000000000000000000000000\",\
            \"hash_type\":\"data\",\
            \"unknown_field\":0,\
            \"args\":\"0x\"\
        }",
    ] {
        let result: Result<Script, _> = serde_json::from_str(malformed);
        assert!(
            result.is_err(),
            "should reject malformed json: [{malformed}]"
        )
    }
}

fn _test_block_convert(data: Bytes, arg: Bytes) -> Result<(), TestCaseError> {
    let block = mock_full_block(data, arg);
    let json_block: BlockView = block.clone().into();
    let encoded = serde_json::to_string(&json_block).unwrap();
    let decode: BlockView = serde_json::from_str(&encoded).unwrap();
    let decode_block: core::BlockView = decode.into();
    header_field_format_check(&encoded);
    prop_assert_eq!(decode_block.data(), block.data());
    prop_assert_eq!(decode_block, block);
    Ok(())
}

fn header_field_format_check(json: &str) {
    lazy_static! {
        static ref RE: Regex = Regex::new("\"(version|compact_target|parent_hash|timestamp|number|epoch|transactions_root|proposals_hash|extra_hash|dao|nonce)\":\"(?P<value>.*?\")").unwrap();
    }
    for caps in RE.captures_iter(json) {
        assert!(&caps["value"].starts_with("0x"));
    }
}

proptest! {
    #[test]
    fn test_block_convert(
        data in any_with::<Vec<u8>>(size_range(80).lift()),
        arg in any_with::<Vec<u8>>(size_range(80).lift()),
    ) {
        _test_block_convert(Bytes::from(data), Bytes::from(arg))?;
    }
}
