/// from parity-bitcoin opcode.rs

use std::fmt;

#[derive(Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord)]
pub enum Opcode {
    // push value
    OP_0 = 0x00,
	OP_PUSHBYTES_1 = 0x01,
	OP_PUSHBYTES_2 = 0x02,
	OP_PUSHBYTES_3 = 0x03,
	OP_PUSHBYTES_4 = 0x04,
	OP_PUSHBYTES_5 = 0x05,
	OP_PUSHBYTES_6 = 0x06,
	OP_PUSHBYTES_7 = 0x07,
	OP_PUSHBYTES_8 = 0x08,
	OP_PUSHBYTES_9 = 0x09,
	OP_PUSHBYTES_10 = 0x0a,
	OP_PUSHBYTES_11 = 0x0b,
	OP_PUSHBYTES_12 = 0x0c,
	OP_PUSHBYTES_13 = 0x0d,
	OP_PUSHBYTES_14 = 0x0e,
	OP_PUSHBYTES_15 = 0x0f,
	OP_PUSHBYTES_16 = 0x10,
	OP_PUSHBYTES_17 = 0x11,
	OP_PUSHBYTES_18 = 0x12,
	OP_PUSHBYTES_19 = 0x13,
	OP_PUSHBYTES_20 = 0x14,
	OP_PUSHBYTES_21 = 0x15,
	OP_PUSHBYTES_22 = 0x16,
	OP_PUSHBYTES_23 = 0x17,
	OP_PUSHBYTES_24 = 0x18,
	OP_PUSHBYTES_25 = 0x19,
	OP_PUSHBYTES_26 = 0x1a,
	OP_PUSHBYTES_27 = 0x1b,
	OP_PUSHBYTES_28 = 0x1c,
	OP_PUSHBYTES_29 = 0x1d,
	OP_PUSHBYTES_30 = 0x1e,
	OP_PUSHBYTES_31 = 0x1f,
	OP_PUSHBYTES_32 = 0x20,
	OP_PUSHBYTES_33 = 0x21,
	OP_PUSHBYTES_34 = 0x22,
	OP_PUSHBYTES_35 = 0x23,
	OP_PUSHBYTES_36 = 0x24,
	OP_PUSHBYTES_37 = 0x25,
	OP_PUSHBYTES_38 = 0x26,
	OP_PUSHBYTES_39 = 0x27,
	OP_PUSHBYTES_40 = 0x28,
	OP_PUSHBYTES_41 = 0x29,
	OP_PUSHBYTES_42 = 0x2a,
	OP_PUSHBYTES_43 = 0x2b,
	OP_PUSHBYTES_44 = 0x2c,
	OP_PUSHBYTES_45 = 0x2d,
	OP_PUSHBYTES_46 = 0x2e,
	OP_PUSHBYTES_47 = 0x2f,
	OP_PUSHBYTES_48 = 0x30,
	OP_PUSHBYTES_49 = 0x31,
	OP_PUSHBYTES_50 = 0x32,
	OP_PUSHBYTES_51 = 0x33,
	OP_PUSHBYTES_52 = 0x34,
	OP_PUSHBYTES_53 = 0x35,
	OP_PUSHBYTES_54 = 0x36,
	OP_PUSHBYTES_55 = 0x37,
	OP_PUSHBYTES_56 = 0x38,
	OP_PUSHBYTES_57 = 0x39,
	OP_PUSHBYTES_58 = 0x3a,
	OP_PUSHBYTES_59 = 0x3b,
	OP_PUSHBYTES_60 = 0x3c,
	OP_PUSHBYTES_61 = 0x3d,
	OP_PUSHBYTES_62 = 0x3e,
	OP_PUSHBYTES_63 = 0x3f,
	OP_PUSHBYTES_64 = 0x40,
	OP_PUSHBYTES_65 = 0x41,
	OP_PUSHBYTES_66 = 0x42,
	OP_PUSHBYTES_67 = 0x43,
	OP_PUSHBYTES_68 = 0x44,
	OP_PUSHBYTES_69 = 0x45,
	OP_PUSHBYTES_70 = 0x46,
	OP_PUSHBYTES_71 = 0x47,
	OP_PUSHBYTES_72 = 0x48,
	OP_PUSHBYTES_73 = 0x49,
	OP_PUSHBYTES_74 = 0x4a,
	OP_PUSHBYTES_75 = 0x4b,
    OP_PUSHDATA1 = 0x4c,
    OP_PUSHDATA2 = 0x4d,
    OP_PUSHDATA4 = 0x4e,
    OP_1NEGATE = 0x4f,
    OP_RESERVED = 0x50,
    OP_1 = 0x51,
    OP_2 = 0x52,
    OP_3 = 0x53,
    OP_4 = 0x54,
    OP_5 = 0x55,
    OP_6 = 0x56,
    OP_7 = 0x57,
    OP_8 = 0x58,
    OP_9 = 0x59,
    OP_10 = 0x5a,
    OP_11 = 0x5b,
    OP_12 = 0x5c,
    OP_13 = 0x5d,
    OP_14 = 0x5e,
    OP_15 = 0x5f,
    OP_16 = 0x60,

	// control
	OP_NOP = 0x61,
	OP_VER = 0x62,
	OP_IF = 0x63,
	OP_NOTIF = 0x64,
	OP_VERIF = 0x65,
	OP_VERNOTIF = 0x66,
	OP_ELSE = 0x67,
	OP_ENDIF = 0x68,
	OP_VERIFY = 0x69,
	OP_RETURN = 0x6a,

	// stack ops
	OP_TOALTSTACK = 0x6b,
	OP_FROMALTSTACK = 0x6c,
	OP_2DROP = 0x6d,
	OP_2DUP = 0x6e,
	OP_3DUP = 0x6f,
	OP_2OVER = 0x70,
	OP_2ROT = 0x71,
	OP_2SWAP = 0x72,
	OP_IFDUP = 0x73,
	OP_DEPTH = 0x74,
	OP_DROP = 0x75,
	OP_DUP = 0x76,
	OP_NIP = 0x77,
	OP_OVER = 0x78,
	OP_PICK = 0x79,
	OP_ROLL = 0x7a,
	OP_ROT = 0x7b,
	OP_SWAP = 0x7c,
	OP_TUCK = 0x7d,

	// splice ops
	OP_CAT = 0x7e,
	OP_SUBSTR = 0x7f,
	OP_LEFT = 0x80,
	OP_RIGHT = 0x81,
	OP_SIZE = 0x82,

	// bit logic
	OP_INVERT = 0x83,
	OP_AND = 0x84,
	OP_OR = 0x85,
	OP_XOR = 0x86,
	OP_EQUAL = 0x87,
	OP_EQUALVERIFY = 0x88,
	OP_RESERVED1 = 0x89,
	OP_RESERVED2 = 0x8a,

	// numeric
	OP_1ADD = 0x8b,
	OP_1SUB = 0x8c,
	OP_2MUL = 0x8d,
	OP_2DIV = 0x8e,
	OP_NEGATE = 0x8f,
	OP_ABS = 0x90,
	OP_NOT = 0x91,
	OP_0NOTEQUAL = 0x92,

	OP_ADD = 0x93,
	OP_SUB = 0x94,
	OP_MUL = 0x95,
	OP_DIV = 0x96,
	OP_MOD = 0x97,
	OP_LSHIFT = 0x98,
	OP_RSHIFT = 0x99,

	OP_BOOLAND = 0x9a,
	OP_BOOLOR = 0x9b,
	OP_NUMEQUAL = 0x9c,
	OP_NUMEQUALVERIFY = 0x9d,
	OP_NUMNOTEQUAL = 0x9e,
	OP_LESSTHAN = 0x9f,
	OP_GREATERTHAN = 0xa0,
	OP_LESSTHANOREQUAL = 0xa1,
	OP_GREATERTHANOREQUAL = 0xa2,
	OP_MIN = 0xa3,
	OP_MAX = 0xa4,

	OP_WITHIN = 0xa5,

	// crypto
	OP_RIPEMD160 = 0xa6,
	OP_SHA1 = 0xa7,
	OP_SHA256 = 0xa8,
	OP_HASH160 = 0xa9,
	OP_HASH256 = 0xaa,
	OP_CODESEPARATOR = 0xab,
	OP_CHECKSIG = 0xac,
	OP_CHECKSIGVERIFY = 0xad,
	OP_CHECKMULTISIG = 0xae,
	OP_CHECKMULTISIGVERIFY = 0xaf,

	// expansion
	OP_NOP1 = 0xb0,
	OP_CHECKLOCKTIMEVERIFY = 0xb1,
	//OP_NOP2 = OP_CHECKLOCKTIMEVERIFY,
	OP_CHECKSEQUENCEVERIFY = 0xb2,
	//OP_NOP3 = OP_CHECKSEQUENCEVERIFY,
	OP_NOP4 = 0xb3,
	OP_NOP5 = 0xb4,
	OP_NOP6 = 0xb5,
	OP_NOP7 = 0xb6,
	OP_NOP8 = 0xb7,
	OP_NOP9 = 0xb8,
	OP_NOP10 = 0xb9,
}

impl fmt::Display for Opcode {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		fmt::Debug::fmt(self, f)
	}
}

impl Opcode {
	pub fn from_u8(u: u8) -> Option<Self> {
		use self::Opcode::*;
		match u {
			0x00 => Some(OP_0),
			0x01 => Some(OP_PUSHBYTES_1),
			0x02 => Some(OP_PUSHBYTES_2),
			0x03 => Some(OP_PUSHBYTES_3),
			0x04 => Some(OP_PUSHBYTES_4),
			0x05 => Some(OP_PUSHBYTES_5),
			0x06 => Some(OP_PUSHBYTES_6),
			0x07 => Some(OP_PUSHBYTES_7),
			0x08 => Some(OP_PUSHBYTES_8),
			0x09 => Some(OP_PUSHBYTES_9),
			0x0a => Some(OP_PUSHBYTES_10),
			0x0b => Some(OP_PUSHBYTES_11),
			0x0c => Some(OP_PUSHBYTES_12),
			0x0d => Some(OP_PUSHBYTES_13),
			0x0e => Some(OP_PUSHBYTES_14),
			0x0f => Some(OP_PUSHBYTES_15),
			0x10 => Some(OP_PUSHBYTES_16),
			0x11 => Some(OP_PUSHBYTES_17),
			0x12 => Some(OP_PUSHBYTES_18),
			0x13 => Some(OP_PUSHBYTES_19),
			0x14 => Some(OP_PUSHBYTES_20),
			0x15 => Some(OP_PUSHBYTES_21),
			0x16 => Some(OP_PUSHBYTES_22),
			0x17 => Some(OP_PUSHBYTES_23),
			0x18 => Some(OP_PUSHBYTES_24),
			0x19 => Some(OP_PUSHBYTES_25),
			0x1a => Some(OP_PUSHBYTES_26),
			0x1b => Some(OP_PUSHBYTES_27),
			0x1c => Some(OP_PUSHBYTES_28),
			0x1d => Some(OP_PUSHBYTES_29),
			0x1e => Some(OP_PUSHBYTES_30),
			0x1f => Some(OP_PUSHBYTES_31),
			0x20 => Some(OP_PUSHBYTES_32),
			0x21 => Some(OP_PUSHBYTES_33),
			0x22 => Some(OP_PUSHBYTES_34),
			0x23 => Some(OP_PUSHBYTES_35),
			0x24 => Some(OP_PUSHBYTES_36),
			0x25 => Some(OP_PUSHBYTES_37),
			0x26 => Some(OP_PUSHBYTES_38),
			0x27 => Some(OP_PUSHBYTES_39),
			0x28 => Some(OP_PUSHBYTES_40),
			0x29 => Some(OP_PUSHBYTES_41),
			0x2a => Some(OP_PUSHBYTES_42),
			0x2b => Some(OP_PUSHBYTES_43),
			0x2c => Some(OP_PUSHBYTES_44),
			0x2d => Some(OP_PUSHBYTES_45),
			0x2e => Some(OP_PUSHBYTES_46),
			0x2f => Some(OP_PUSHBYTES_47),
			0x30 => Some(OP_PUSHBYTES_48),
			0x31 => Some(OP_PUSHBYTES_49),
			0x32 => Some(OP_PUSHBYTES_50),
			0x33 => Some(OP_PUSHBYTES_51),
			0x34 => Some(OP_PUSHBYTES_52),
			0x35 => Some(OP_PUSHBYTES_53),
			0x36 => Some(OP_PUSHBYTES_54),
			0x37 => Some(OP_PUSHBYTES_55),
			0x38 => Some(OP_PUSHBYTES_56),
			0x39 => Some(OP_PUSHBYTES_57),
			0x3a => Some(OP_PUSHBYTES_58),
			0x3b => Some(OP_PUSHBYTES_59),
			0x3c => Some(OP_PUSHBYTES_60),
			0x3d => Some(OP_PUSHBYTES_61),
			0x3e => Some(OP_PUSHBYTES_62),
			0x3f => Some(OP_PUSHBYTES_63),
			0x40 => Some(OP_PUSHBYTES_64),
			0x41 => Some(OP_PUSHBYTES_65),
			0x42 => Some(OP_PUSHBYTES_66),
			0x43 => Some(OP_PUSHBYTES_67),
			0x44 => Some(OP_PUSHBYTES_68),
			0x45 => Some(OP_PUSHBYTES_69),
			0x46 => Some(OP_PUSHBYTES_70),
			0x47 => Some(OP_PUSHBYTES_71),
			0x48 => Some(OP_PUSHBYTES_72),
			0x49 => Some(OP_PUSHBYTES_73),
			0x4a => Some(OP_PUSHBYTES_74),
			0x4b => Some(OP_PUSHBYTES_75),
			0x4c => Some(OP_PUSHDATA1),
			0x4d => Some(OP_PUSHDATA2),
			0x4e => Some(OP_PUSHDATA4),
			0x4f => Some(OP_1NEGATE),
			0x50 => Some(OP_RESERVED),
			0x51 => Some(OP_1),
			0x52 => Some(OP_2),
			0x53 => Some(OP_3),
			0x54 => Some(OP_4),
			0x55 => Some(OP_5),
			0x56 => Some(OP_6),
			0x57 => Some(OP_7),
			0x58 => Some(OP_8),
			0x59 => Some(OP_9),
			0x5a => Some(OP_10),
			0x5b => Some(OP_11),
			0x5c => Some(OP_12),
			0x5d => Some(OP_13),
			0x5e => Some(OP_14),
			0x5f => Some(OP_15),
			0x60 => Some(OP_16),

			// control
			0x61 => Some(OP_NOP),
			0x62 => Some(OP_VER),
			0x63 => Some(OP_IF),
			0x64 => Some(OP_NOTIF),
			0x65 => Some(OP_VERIF),
			0x66 => Some(OP_VERNOTIF),
			0x67 => Some(OP_ELSE),
			0x68 => Some(OP_ENDIF),
			0x69 => Some(OP_VERIFY),
			0x6a => Some(OP_RETURN),

			// stack ops
			0x6b => Some(OP_TOALTSTACK),
			0x6c => Some(OP_FROMALTSTACK),
			0x6d => Some(OP_2DROP),
			0x6e => Some(OP_2DUP),
			0x6f => Some(OP_3DUP),
			0x70 => Some(OP_2OVER),
			0x71 => Some(OP_2ROT),
			0x72 => Some(OP_2SWAP),
			0x73 => Some(OP_IFDUP),
			0x74 => Some(OP_DEPTH),
			0x75 => Some(OP_DROP),
			0x76 => Some(OP_DUP),
			0x77 => Some(OP_NIP),
			0x78 => Some(OP_OVER),
			0x79 => Some(OP_PICK),
			0x7a => Some(OP_ROLL),
			0x7b => Some(OP_ROT),
			0x7c => Some(OP_SWAP),
			0x7d => Some(OP_TUCK),

			// splice ops
			0x7e => Some(OP_CAT),
			0x7f => Some(OP_SUBSTR),
			0x80 => Some(OP_LEFT),
			0x81 => Some(OP_RIGHT),
			0x82 => Some(OP_SIZE),

			// bit logic
			0x83 => Some(OP_INVERT),
			0x84 => Some(OP_AND),
			0x85 => Some(OP_OR),
			0x86 => Some(OP_XOR),
			0x87 => Some(OP_EQUAL),
			0x88 => Some(OP_EQUALVERIFY),
			0x89 => Some(OP_RESERVED1),
			0x8a => Some(OP_RESERVED2),

			// numeric
			0x8b => Some(OP_1ADD),
			0x8c => Some(OP_1SUB),
			0x8d => Some(OP_2MUL),
			0x8e => Some(OP_2DIV),
			0x8f => Some(OP_NEGATE),
			0x90 => Some(OP_ABS),
			0x91 => Some(OP_NOT),
			0x92 => Some(OP_0NOTEQUAL),

			0x93 => Some(OP_ADD),
			0x94 => Some(OP_SUB),
			0x95 => Some(OP_MUL),
			0x96 => Some(OP_DIV),
			0x97 => Some(OP_MOD),
			0x98 => Some(OP_LSHIFT),
			0x99 => Some(OP_RSHIFT),

			0x9a => Some(OP_BOOLAND),
			0x9b => Some(OP_BOOLOR),
			0x9c => Some(OP_NUMEQUAL),
			0x9d => Some(OP_NUMEQUALVERIFY),
			0x9e => Some(OP_NUMNOTEQUAL),
			0x9f => Some(OP_LESSTHAN),
			0xa0 => Some(OP_GREATERTHAN),
			0xa1 => Some(OP_LESSTHANOREQUAL),
			0xa2 => Some(OP_GREATERTHANOREQUAL),
			0xa3 => Some(OP_MIN),
			0xa4 => Some(OP_MAX),

			0xa5 => Some(OP_WITHIN),

			// crypto
			0xa6 => Some(OP_RIPEMD160),
			0xa7 => Some(OP_SHA1),
			0xa8 => Some(OP_SHA256),
			0xa9 => Some(OP_HASH160),
			0xaa => Some(OP_HASH256),
			0xab => Some(OP_CODESEPARATOR),
			0xac => Some(OP_CHECKSIG),
			0xad => Some(OP_CHECKSIGVERIFY),
			0xae => Some(OP_CHECKMULTISIG),
			0xaf => Some(OP_CHECKMULTISIGVERIFY),

			// expansion
			0xb0 => Some(OP_NOP1),
			0xb1 => Some(OP_CHECKLOCKTIMEVERIFY),
			//OP_NOP2 = OP_CHECKLOCKTIMEVERIFY,
			0xb2 => Some(OP_CHECKSEQUENCEVERIFY),
			//OP_NOP3 = OP_CHECKSEQUENCEVERIFY,
			0xb3 => Some(OP_NOP4),
			0xb4 => Some(OP_NOP5),
			0xb5 => Some(OP_NOP6),
			0xb6 => Some(OP_NOP7),
			0xb7 => Some(OP_NOP8),
			0xb8 => Some(OP_NOP9),
			0xb9 => Some(OP_NOP10),
			_ => None,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::Opcode;

	#[test]
	fn test_to_from_opcode() {

		// push value
		assert_eq!(Opcode::OP_0, Opcode::from_u8(Opcode::OP_0 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_1, Opcode::from_u8(Opcode::OP_PUSHBYTES_1 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_2, Opcode::from_u8(Opcode::OP_PUSHBYTES_2 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_3, Opcode::from_u8(Opcode::OP_PUSHBYTES_3 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_4, Opcode::from_u8(Opcode::OP_PUSHBYTES_4 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_5, Opcode::from_u8(Opcode::OP_PUSHBYTES_5 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_6, Opcode::from_u8(Opcode::OP_PUSHBYTES_6 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_7, Opcode::from_u8(Opcode::OP_PUSHBYTES_7 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_8, Opcode::from_u8(Opcode::OP_PUSHBYTES_8 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_9, Opcode::from_u8(Opcode::OP_PUSHBYTES_9 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_10, Opcode::from_u8(Opcode::OP_PUSHBYTES_10 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_11, Opcode::from_u8(Opcode::OP_PUSHBYTES_11 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_12, Opcode::from_u8(Opcode::OP_PUSHBYTES_12 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_13, Opcode::from_u8(Opcode::OP_PUSHBYTES_13 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_14, Opcode::from_u8(Opcode::OP_PUSHBYTES_14 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_15, Opcode::from_u8(Opcode::OP_PUSHBYTES_15 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_16, Opcode::from_u8(Opcode::OP_PUSHBYTES_16 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_17, Opcode::from_u8(Opcode::OP_PUSHBYTES_17 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_18, Opcode::from_u8(Opcode::OP_PUSHBYTES_18 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_19, Opcode::from_u8(Opcode::OP_PUSHBYTES_19 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_20, Opcode::from_u8(Opcode::OP_PUSHBYTES_20 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_21, Opcode::from_u8(Opcode::OP_PUSHBYTES_21 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_22, Opcode::from_u8(Opcode::OP_PUSHBYTES_22 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_23, Opcode::from_u8(Opcode::OP_PUSHBYTES_23 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_24, Opcode::from_u8(Opcode::OP_PUSHBYTES_24 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_25, Opcode::from_u8(Opcode::OP_PUSHBYTES_25 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_26, Opcode::from_u8(Opcode::OP_PUSHBYTES_26 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_27, Opcode::from_u8(Opcode::OP_PUSHBYTES_27 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_28, Opcode::from_u8(Opcode::OP_PUSHBYTES_28 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_29, Opcode::from_u8(Opcode::OP_PUSHBYTES_29 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_30, Opcode::from_u8(Opcode::OP_PUSHBYTES_30 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_31, Opcode::from_u8(Opcode::OP_PUSHBYTES_31 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_32, Opcode::from_u8(Opcode::OP_PUSHBYTES_32 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_33, Opcode::from_u8(Opcode::OP_PUSHBYTES_33 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_34, Opcode::from_u8(Opcode::OP_PUSHBYTES_34 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_35, Opcode::from_u8(Opcode::OP_PUSHBYTES_35 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_36, Opcode::from_u8(Opcode::OP_PUSHBYTES_36 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_37, Opcode::from_u8(Opcode::OP_PUSHBYTES_37 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_38, Opcode::from_u8(Opcode::OP_PUSHBYTES_38 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_39, Opcode::from_u8(Opcode::OP_PUSHBYTES_39 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_40, Opcode::from_u8(Opcode::OP_PUSHBYTES_40 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_41, Opcode::from_u8(Opcode::OP_PUSHBYTES_41 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_42, Opcode::from_u8(Opcode::OP_PUSHBYTES_42 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_43, Opcode::from_u8(Opcode::OP_PUSHBYTES_43 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_44, Opcode::from_u8(Opcode::OP_PUSHBYTES_44 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_45, Opcode::from_u8(Opcode::OP_PUSHBYTES_45 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_46, Opcode::from_u8(Opcode::OP_PUSHBYTES_46 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_47, Opcode::from_u8(Opcode::OP_PUSHBYTES_47 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_48, Opcode::from_u8(Opcode::OP_PUSHBYTES_48 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_49, Opcode::from_u8(Opcode::OP_PUSHBYTES_49 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_50, Opcode::from_u8(Opcode::OP_PUSHBYTES_50 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_51, Opcode::from_u8(Opcode::OP_PUSHBYTES_51 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_52, Opcode::from_u8(Opcode::OP_PUSHBYTES_52 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_53, Opcode::from_u8(Opcode::OP_PUSHBYTES_53 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_54, Opcode::from_u8(Opcode::OP_PUSHBYTES_54 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_55, Opcode::from_u8(Opcode::OP_PUSHBYTES_55 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_56, Opcode::from_u8(Opcode::OP_PUSHBYTES_56 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_57, Opcode::from_u8(Opcode::OP_PUSHBYTES_57 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_58, Opcode::from_u8(Opcode::OP_PUSHBYTES_58 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_59, Opcode::from_u8(Opcode::OP_PUSHBYTES_59 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_60, Opcode::from_u8(Opcode::OP_PUSHBYTES_60 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_61, Opcode::from_u8(Opcode::OP_PUSHBYTES_61 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_62, Opcode::from_u8(Opcode::OP_PUSHBYTES_62 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_63, Opcode::from_u8(Opcode::OP_PUSHBYTES_63 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_64, Opcode::from_u8(Opcode::OP_PUSHBYTES_64 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_65, Opcode::from_u8(Opcode::OP_PUSHBYTES_65 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_66, Opcode::from_u8(Opcode::OP_PUSHBYTES_66 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_67, Opcode::from_u8(Opcode::OP_PUSHBYTES_67 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_68, Opcode::from_u8(Opcode::OP_PUSHBYTES_68 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_69, Opcode::from_u8(Opcode::OP_PUSHBYTES_69 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_70, Opcode::from_u8(Opcode::OP_PUSHBYTES_70 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_71, Opcode::from_u8(Opcode::OP_PUSHBYTES_71 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_72, Opcode::from_u8(Opcode::OP_PUSHBYTES_72 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_73, Opcode::from_u8(Opcode::OP_PUSHBYTES_73 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_74, Opcode::from_u8(Opcode::OP_PUSHBYTES_74 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHBYTES_75, Opcode::from_u8(Opcode::OP_PUSHBYTES_75 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHDATA1, Opcode::from_u8(Opcode::OP_PUSHDATA1 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHDATA2, Opcode::from_u8(Opcode::OP_PUSHDATA2 as u8).unwrap());
		assert_eq!(Opcode::OP_PUSHDATA4, Opcode::from_u8(Opcode::OP_PUSHDATA4 as u8).unwrap());
		assert_eq!(Opcode::OP_1NEGATE, Opcode::from_u8(Opcode::OP_1NEGATE as u8).unwrap());
		assert_eq!(Opcode::OP_RESERVED, Opcode::from_u8(Opcode::OP_RESERVED as u8).unwrap());
		assert_eq!(Opcode::OP_1, Opcode::from_u8(Opcode::OP_1 as u8).unwrap());
		assert_eq!(Opcode::OP_2, Opcode::from_u8(Opcode::OP_2 as u8).unwrap());
		assert_eq!(Opcode::OP_3, Opcode::from_u8(Opcode::OP_3 as u8).unwrap());
		assert_eq!(Opcode::OP_4, Opcode::from_u8(Opcode::OP_4 as u8).unwrap());
		assert_eq!(Opcode::OP_5, Opcode::from_u8(Opcode::OP_5 as u8).unwrap());
		assert_eq!(Opcode::OP_6, Opcode::from_u8(Opcode::OP_6 as u8).unwrap());
		assert_eq!(Opcode::OP_7, Opcode::from_u8(Opcode::OP_7 as u8).unwrap());
		assert_eq!(Opcode::OP_8, Opcode::from_u8(Opcode::OP_8 as u8).unwrap());
		assert_eq!(Opcode::OP_9, Opcode::from_u8(Opcode::OP_9 as u8).unwrap());
		assert_eq!(Opcode::OP_10, Opcode::from_u8(Opcode::OP_10 as u8).unwrap());
		assert_eq!(Opcode::OP_11, Opcode::from_u8(Opcode::OP_11 as u8).unwrap());
		assert_eq!(Opcode::OP_12, Opcode::from_u8(Opcode::OP_12 as u8).unwrap());
		assert_eq!(Opcode::OP_13, Opcode::from_u8(Opcode::OP_13 as u8).unwrap());
		assert_eq!(Opcode::OP_14, Opcode::from_u8(Opcode::OP_14 as u8).unwrap());
		assert_eq!(Opcode::OP_15, Opcode::from_u8(Opcode::OP_15 as u8).unwrap());
		assert_eq!(Opcode::OP_16, Opcode::from_u8(Opcode::OP_16 as u8).unwrap());

		// control
		assert_eq!(Opcode::OP_NOP, Opcode::from_u8(Opcode::OP_NOP as u8).unwrap());
		assert_eq!(Opcode::OP_VER, Opcode::from_u8(Opcode::OP_VER as u8).unwrap());
		assert_eq!(Opcode::OP_IF, Opcode::from_u8(Opcode::OP_IF as u8).unwrap());
		assert_eq!(Opcode::OP_NOTIF, Opcode::from_u8(Opcode::OP_NOTIF as u8).unwrap());
		assert_eq!(Opcode::OP_VERIF, Opcode::from_u8(Opcode::OP_VERIF as u8).unwrap());
		assert_eq!(Opcode::OP_VERNOTIF, Opcode::from_u8(Opcode::OP_VERNOTIF as u8).unwrap());
		assert_eq!(Opcode::OP_ELSE, Opcode::from_u8(Opcode::OP_ELSE as u8).unwrap());
		assert_eq!(Opcode::OP_ENDIF, Opcode::from_u8(Opcode::OP_ENDIF as u8).unwrap());
		assert_eq!(Opcode::OP_VERIFY, Opcode::from_u8(Opcode::OP_VERIFY as u8).unwrap());
		assert_eq!(Opcode::OP_RETURN, Opcode::from_u8(Opcode::OP_RETURN as u8).unwrap());

		// stack ops
		assert_eq!(Opcode::OP_TOALTSTACK, Opcode::from_u8(Opcode::OP_TOALTSTACK as u8).unwrap());
		assert_eq!(Opcode::OP_FROMALTSTACK, Opcode::from_u8(Opcode::OP_FROMALTSTACK as u8).unwrap());
		assert_eq!(Opcode::OP_2DROP, Opcode::from_u8(Opcode::OP_2DROP as u8).unwrap());
		assert_eq!(Opcode::OP_2DUP, Opcode::from_u8(Opcode::OP_2DUP as u8).unwrap());
		assert_eq!(Opcode::OP_3DUP, Opcode::from_u8(Opcode::OP_3DUP as u8).unwrap());
		assert_eq!(Opcode::OP_2OVER, Opcode::from_u8(Opcode::OP_2OVER as u8).unwrap());
		assert_eq!(Opcode::OP_2ROT, Opcode::from_u8(Opcode::OP_2ROT as u8).unwrap());
		assert_eq!(Opcode::OP_2SWAP, Opcode::from_u8(Opcode::OP_2SWAP as u8).unwrap());
		assert_eq!(Opcode::OP_IFDUP, Opcode::from_u8(Opcode::OP_IFDUP as u8).unwrap());
		assert_eq!(Opcode::OP_DEPTH, Opcode::from_u8(Opcode::OP_DEPTH as u8).unwrap());
		assert_eq!(Opcode::OP_DROP, Opcode::from_u8(Opcode::OP_DROP as u8).unwrap());
		assert_eq!(Opcode::OP_DUP, Opcode::from_u8(Opcode::OP_DUP as u8).unwrap());
		assert_eq!(Opcode::OP_NIP, Opcode::from_u8(Opcode::OP_NIP as u8).unwrap());
		assert_eq!(Opcode::OP_OVER, Opcode::from_u8(Opcode::OP_OVER as u8).unwrap());
		assert_eq!(Opcode::OP_PICK, Opcode::from_u8(Opcode::OP_PICK as u8).unwrap());
		assert_eq!(Opcode::OP_ROLL, Opcode::from_u8(Opcode::OP_ROLL as u8).unwrap());
		assert_eq!(Opcode::OP_ROT, Opcode::from_u8(Opcode::OP_ROT as u8).unwrap());
		assert_eq!(Opcode::OP_SWAP, Opcode::from_u8(Opcode::OP_SWAP as u8).unwrap());
		assert_eq!(Opcode::OP_TUCK, Opcode::from_u8(Opcode::OP_TUCK as u8).unwrap());

		// splice ops
		assert_eq!(Opcode::OP_CAT, Opcode::from_u8(Opcode::OP_CAT as u8).unwrap());
		assert_eq!(Opcode::OP_SUBSTR, Opcode::from_u8(Opcode::OP_SUBSTR as u8).unwrap());
		assert_eq!(Opcode::OP_LEFT, Opcode::from_u8(Opcode::OP_LEFT as u8).unwrap());
		assert_eq!(Opcode::OP_RIGHT, Opcode::from_u8(Opcode::OP_RIGHT as u8).unwrap());
		assert_eq!(Opcode::OP_SIZE, Opcode::from_u8(Opcode::OP_SIZE as u8).unwrap());

		// bit logic
		assert_eq!(Opcode::OP_INVERT, Opcode::from_u8(Opcode::OP_INVERT as u8).unwrap());
		assert_eq!(Opcode::OP_AND, Opcode::from_u8(Opcode::OP_AND as u8).unwrap());
		assert_eq!(Opcode::OP_OR, Opcode::from_u8(Opcode::OP_OR as u8).unwrap());
		assert_eq!(Opcode::OP_XOR, Opcode::from_u8(Opcode::OP_XOR as u8).unwrap());
		assert_eq!(Opcode::OP_EQUAL, Opcode::from_u8(Opcode::OP_EQUAL as u8).unwrap());
		assert_eq!(Opcode::OP_EQUALVERIFY, Opcode::from_u8(Opcode::OP_EQUALVERIFY as u8).unwrap());
		assert_eq!(Opcode::OP_RESERVED1, Opcode::from_u8(Opcode::OP_RESERVED1 as u8).unwrap());
		assert_eq!(Opcode::OP_RESERVED2, Opcode::from_u8(Opcode::OP_RESERVED2 as u8).unwrap());

		// numeric
		assert_eq!(Opcode::OP_1ADD, Opcode::from_u8(Opcode::OP_1ADD as u8).unwrap());
		assert_eq!(Opcode::OP_1SUB, Opcode::from_u8(Opcode::OP_1SUB as u8).unwrap());
		assert_eq!(Opcode::OP_2MUL, Opcode::from_u8(Opcode::OP_2MUL as u8).unwrap());
		assert_eq!(Opcode::OP_2DIV, Opcode::from_u8(Opcode::OP_2DIV as u8).unwrap());
		assert_eq!(Opcode::OP_NEGATE, Opcode::from_u8(Opcode::OP_NEGATE as u8).unwrap());
		assert_eq!(Opcode::OP_ABS, Opcode::from_u8(Opcode::OP_ABS as u8).unwrap());
		assert_eq!(Opcode::OP_NOT, Opcode::from_u8(Opcode::OP_NOT as u8).unwrap());
		assert_eq!(Opcode::OP_0NOTEQUAL, Opcode::from_u8(Opcode::OP_0NOTEQUAL as u8).unwrap());

		assert_eq!(Opcode::OP_ADD, Opcode::from_u8(Opcode::OP_ADD as u8).unwrap());
		assert_eq!(Opcode::OP_SUB, Opcode::from_u8(Opcode::OP_SUB as u8).unwrap());
		assert_eq!(Opcode::OP_MUL, Opcode::from_u8(Opcode::OP_MUL as u8).unwrap());
		assert_eq!(Opcode::OP_DIV, Opcode::from_u8(Opcode::OP_DIV as u8).unwrap());
		assert_eq!(Opcode::OP_MOD, Opcode::from_u8(Opcode::OP_MOD as u8).unwrap());
		assert_eq!(Opcode::OP_LSHIFT, Opcode::from_u8(Opcode::OP_LSHIFT as u8).unwrap());
		assert_eq!(Opcode::OP_RSHIFT, Opcode::from_u8(Opcode::OP_RSHIFT as u8).unwrap());

		assert_eq!(Opcode::OP_BOOLAND, Opcode::from_u8(Opcode::OP_BOOLAND as u8).unwrap());
		assert_eq!(Opcode::OP_BOOLOR, Opcode::from_u8(Opcode::OP_BOOLOR as u8).unwrap());
		assert_eq!(Opcode::OP_NUMEQUAL, Opcode::from_u8(Opcode::OP_NUMEQUAL as u8).unwrap());
		assert_eq!(Opcode::OP_NUMEQUALVERIFY, Opcode::from_u8(Opcode::OP_NUMEQUALVERIFY as u8).unwrap());
		assert_eq!(Opcode::OP_NUMNOTEQUAL, Opcode::from_u8(Opcode::OP_NUMNOTEQUAL as u8).unwrap());
		assert_eq!(Opcode::OP_LESSTHAN, Opcode::from_u8(Opcode::OP_LESSTHAN as u8).unwrap());
		assert_eq!(Opcode::OP_GREATERTHAN, Opcode::from_u8(Opcode::OP_GREATERTHAN as u8).unwrap());
		assert_eq!(Opcode::OP_LESSTHANOREQUAL, Opcode::from_u8(Opcode::OP_LESSTHANOREQUAL as u8).unwrap());
		assert_eq!(Opcode::OP_GREATERTHANOREQUAL, Opcode::from_u8(Opcode::OP_GREATERTHANOREQUAL as u8).unwrap());
		assert_eq!(Opcode::OP_MIN, Opcode::from_u8(Opcode::OP_MIN as u8).unwrap());
		assert_eq!(Opcode::OP_MAX, Opcode::from_u8(Opcode::OP_MAX as u8).unwrap());

		assert_eq!(Opcode::OP_WITHIN, Opcode::from_u8(Opcode::OP_WITHIN as u8).unwrap());

		// crypto
		assert_eq!(Opcode::OP_RIPEMD160, Opcode::from_u8(Opcode::OP_RIPEMD160 as u8).unwrap());
		assert_eq!(Opcode::OP_SHA1, Opcode::from_u8(Opcode::OP_SHA1 as u8).unwrap());
		assert_eq!(Opcode::OP_SHA256, Opcode::from_u8(Opcode::OP_SHA256 as u8).unwrap());
		assert_eq!(Opcode::OP_HASH160, Opcode::from_u8(Opcode::OP_HASH160 as u8).unwrap());
		assert_eq!(Opcode::OP_HASH256, Opcode::from_u8(Opcode::OP_HASH256 as u8).unwrap());
		assert_eq!(Opcode::OP_CODESEPARATOR, Opcode::from_u8(Opcode::OP_CODESEPARATOR as u8).unwrap());
		assert_eq!(Opcode::OP_CHECKSIG, Opcode::from_u8(Opcode::OP_CHECKSIG as u8).unwrap());
		assert_eq!(Opcode::OP_CHECKSIGVERIFY, Opcode::from_u8(Opcode::OP_CHECKSIGVERIFY as u8).unwrap());
		assert_eq!(Opcode::OP_CHECKMULTISIG, Opcode::from_u8(Opcode::OP_CHECKMULTISIG as u8).unwrap());
		assert_eq!(Opcode::OP_CHECKMULTISIGVERIFY, Opcode::from_u8(Opcode::OP_CHECKMULTISIGVERIFY as u8).unwrap());

		// expansion
		assert_eq!(Opcode::OP_NOP1, Opcode::from_u8(Opcode::OP_NOP1 as u8).unwrap());
		assert_eq!(Opcode::OP_CHECKLOCKTIMEVERIFY, Opcode::from_u8(Opcode::OP_CHECKLOCKTIMEVERIFY as u8).unwrap());
		assert_eq!(Opcode::OP_CHECKSEQUENCEVERIFY, Opcode::from_u8(Opcode::OP_CHECKSEQUENCEVERIFY as u8).unwrap());
		assert_eq!(Opcode::OP_NOP4, Opcode::from_u8(Opcode::OP_NOP4 as u8).unwrap());
		assert_eq!(Opcode::OP_NOP5, Opcode::from_u8(Opcode::OP_NOP5 as u8).unwrap());
		assert_eq!(Opcode::OP_NOP6, Opcode::from_u8(Opcode::OP_NOP6 as u8).unwrap());
		assert_eq!(Opcode::OP_NOP7, Opcode::from_u8(Opcode::OP_NOP7 as u8).unwrap());
		assert_eq!(Opcode::OP_NOP8, Opcode::from_u8(Opcode::OP_NOP8 as u8).unwrap());
		assert_eq!(Opcode::OP_NOP9, Opcode::from_u8(Opcode::OP_NOP9 as u8).unwrap());
		assert_eq!(Opcode::OP_NOP10, Opcode::from_u8(Opcode::OP_NOP10 as u8).unwrap());
	}
}
