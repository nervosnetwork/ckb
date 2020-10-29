use crate::ScriptError;
use byteorder::{ByteOrder, LittleEndian};
use ckb_types::core::TransactionView;
use ckb_vm::{
    instructions::{extract_opcode, i, m, rvc, Instruction, Itype, Stype},
    registers::{RA, ZERO},
};
use ckb_vm_definitions::instructions as insts;
use goblin::elf::{section_header::SHF_EXECINSTR, Elf};

const CKB_VM_ISSUE_92: &str = "https://github.com/nervosnetwork/ckb-vm/issues/92";

/// TODO(doc): @doitian
pub struct IllTransactionChecker<'a> {
    tx: &'a TransactionView,
}

impl<'a> IllTransactionChecker<'a> {
    /// TODO(doc): @doitian
    pub fn new(tx: &'a TransactionView) -> Self {
        IllTransactionChecker { tx }
    }

    /// TODO(doc): @doitian
    pub fn check(&self) -> Result<(), ScriptError> {
        for (i, data) in self.tx.outputs_data().into_iter().enumerate() {
            IllScriptChecker::new(&data.raw_data(), i).check()?;
        }
        Ok(())
    }
}

struct IllScriptChecker<'a> {
    data: &'a [u8],
    index: usize,
}

impl<'a> IllScriptChecker<'a> {
    pub fn new(data: &'a [u8], index: usize) -> Self {
        IllScriptChecker { data, index }
    }

    pub fn check(&self) -> Result<(), ScriptError> {
        if self.data.is_empty() {
            return Ok(());
        }
        let elf = match Elf::parse(self.data) {
            Ok(elf) => elf,
            // If the data cannot be parsed as ELF format, we will treat
            // it as a non-script binary data. The checking will be skipped
            // here.
            Err(_) => return Ok(()),
        };
        for section_header in elf.section_headers {
            if section_header.sh_flags & u64::from(SHF_EXECINSTR) != 0 {
                let mut pc = section_header.sh_offset;
                let end = section_header.sh_offset + section_header.sh_size;
                while pc < end {
                    match self.decode_instruction(pc) {
                        (Some(i), len) => {
                            match extract_opcode(i) {
                                insts::OP_JALR => {
                                    let i = Itype(i);
                                    if i.rs1() == i.rd() && i.rd() != ZERO {
                                        return Err(ScriptError::EncounteredKnownBugs(
                                            CKB_VM_ISSUE_92.to_string(),
                                            self.index,
                                        ));
                                    }
                                }
                                insts::OP_RVC_JALR => {
                                    let i = Stype(i);
                                    if i.rs1() == RA {
                                        return Err(ScriptError::EncounteredKnownBugs(
                                            CKB_VM_ISSUE_92.to_string(),
                                            self.index,
                                        ));
                                    }
                                }
                                _ => (),
                            };
                            pc += len;
                        }
                        (None, len) => {
                            pc += len;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn decode_instruction(&self, pc: u64) -> (Option<Instruction>, u64) {
        if pc + 2 > self.data.len() as u64 {
            return (None, 2);
        }
        let mut i = u32::from(LittleEndian::read_u16(&self.data[pc as usize..]));
        let len = if i & 0x3 == 0x3 { 4 } else { 2 };
        if len == 4 {
            if pc + 4 > self.data.len() as u64 {
                return (None, 4);
            }
            i = LittleEndian::read_u32(&self.data[pc as usize..]);
        }
        let factories = [rvc::factory::<u64>, i::factory::<u64>, m::factory::<u64>];
        for factory in &factories {
            if let Some(instruction) = factory(i) {
                return (Some(instruction), len);
            }
        }
        (None, len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::read;
    use std::path::Path;

    #[test]
    fn check_good_binary() {
        let data =
            read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/verify")).unwrap();
        assert!(IllScriptChecker::new(&data, 13).check().is_ok());
    }

    #[test]
    fn check_defected_binary() {
        let data =
            read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/defected_binary"))
                .unwrap();
        assert_eq!(
            IllScriptChecker::new(&data, 13).check().unwrap_err(),
            ScriptError::EncounteredKnownBugs(CKB_VM_ISSUE_92.to_string(), 13),
        );
    }

    #[test]
    fn check_jalr_zero_binary() {
        let data = read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/jalr_zero"))
            .unwrap();
        assert!(IllScriptChecker::new(&data, 13).check().is_ok());
    }
}
