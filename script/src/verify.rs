use super::sign::TransactionInputSigner;
use core::script::Script;
use crypto::secp::{Pubkey, Signature};

pub trait SignatureChecker {
    fn check(&self, signature: &Signature, pubkey: &Pubkey, script: &Script) -> bool;
}

#[derive(Debug)]
pub struct TransactionSignatureChecker {
    pub signer: TransactionInputSigner,
    pub input_index: usize,
    pub input_amount: u32,
}

impl SignatureChecker for TransactionSignatureChecker {
    fn check(&self, _signature: &Signature, _pubkey: &Pubkey, _script: &Script) -> bool {
        unimplemented!()
        // let hash = self.signer.sign(self.input_index, self.input_amount, script);
        // pubkey.verify(&hash, signature).unwrap_or(false)
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use super::super::sign::TransactionInputSigner;
//     use bigint::H256;
//     use core::transaction::{CellInput, CellOutput, OutPoint, Transaction};
//     // use crypto::secp::{Generator};

//     #[test]
// 	fn check_signature() {
//         let inputs = vec![CellInput::new(OutPoint::null(), Vec::new())];
//         let outputs = vec![CellOutput::new(0, 50, Vec::new(), H256::from(0))];
//         let tx = Transaction { version: 0, inputs, outputs, deps: Vec::new(), hash: None};
//         let signer: TransactionInputSigner = tx.into();

//         let checker = TransactionSignatureChecker {
// 			signer,
// 			input_index: 0,
// 			input_amount: 0,
// 		};

//         // let script = Script::from("<SIG> <PUBKEY> DUP HASH160 <PUBKEYHASH> EQ CHECKSIG");
//         // let gen = Generator::new();
//         // let (privkey, pubkey) = gen.random_keypair().unwrap();
//         // let signature = privkey.sign_recoverable(&script.hash()).unwrap();

//         // assert!(checker.check(&signature, &pubkey, &script));
//     }
// }
