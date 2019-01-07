use ckb_vm::{
    instructions::{i, m, rvc},
    Instruction,
};

pub fn instruction_cycles(i: &Instruction) -> u64 {
    match i {
        Instruction::I(i) => match i {
            i::Instruction::I(i) => match i.inst() {
                i::ItypeInstruction::JALR => 3,
                i::ItypeInstruction::LD => 2,
                i::ItypeInstruction::LW => 3,
                i::ItypeInstruction::LH => 3,
                i::ItypeInstruction::LB => 3,
                i::ItypeInstruction::LWU => 3,
                i::ItypeInstruction::LHU => 3,
                i::ItypeInstruction::LBU => 3,
                _ => 1,
            },
            i::Instruction::S(s) => match s.inst() {
                // Here we choose to be explicit so as to avoid potential confusions.
                i::StypeInstruction::SB => 3,
                i::StypeInstruction::SH => 3,
                i::StypeInstruction::SW => 3,
                i::StypeInstruction::SD => 2,
            },
            i::Instruction::B(_) => 3,
            // Cycles for Env instructions will be processed in the Env code.
            i::Instruction::Env(_) => 0,
            i::Instruction::JAL { .. } => 3,
            _ => 1,
        },
        Instruction::RVC(i) => match i {
            rvc::Instruction::Iu(i) => match i.inst() {
                rvc::ItypeUInstruction::LW => 3,
                rvc::ItypeUInstruction::LD => 2,
                _ => 1,
            },
            rvc::Instruction::Su(s) => match s.inst() {
                rvc::StypeUInstruction::SW => 3,
                rvc::StypeUInstruction::SD => 2,
                _ => 1,
            },
            rvc::Instruction::Uu(u) => match u.inst() {
                rvc::UtypeUInstruction::LWSP => 3,
                rvc::UtypeUInstruction::LDSP => 2,
                _ => 1,
            },
            rvc::Instruction::CSS(c) => match c.inst() {
                rvc::CSSformatInstruction::SWSP => 3,
                rvc::CSSformatInstruction::SDSP => 2,
                _ => 1,
            },
            rvc::Instruction::BEQZ { .. } => 3,
            rvc::Instruction::BNEZ { .. } => 3,
            rvc::Instruction::JAL { .. } => 3,
            rvc::Instruction::J { .. } => 3,
            rvc::Instruction::JR { .. } => 3,
            rvc::Instruction::JALR { .. } => 3,
            rvc::Instruction::EBREAK => 0,
            _ => 1,
        },
        Instruction::M(m::Instruction(i)) => match i.inst() {
            m::RtypeInstruction::MUL => 5,
            m::RtypeInstruction::MULW => 5,
            m::RtypeInstruction::MULH => 5,
            m::RtypeInstruction::MULHU => 5,
            m::RtypeInstruction::MULHSU => 5,
            m::RtypeInstruction::DIV => 16,
            m::RtypeInstruction::DIVW => 16,
            m::RtypeInstruction::DIVU => 16,
            m::RtypeInstruction::DIVUW => 16,
            m::RtypeInstruction::REM => 16,
            m::RtypeInstruction::REMW => 16,
            m::RtypeInstruction::REMU => 16,
            m::RtypeInstruction::REMUW => 16,
        },
    }
}
