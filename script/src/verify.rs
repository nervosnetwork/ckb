use super::sign::TransactionInputSigner;
use crypto::secp::Signature;

pub trait SignatureVerifier {
    fn verify(&self, signature: &Signature) -> bool;
}

#[derive(Debug)]
pub struct TransactionSignatureVerifier {
    pub signer: TransactionInputSigner,
    pub input_index: usize,
}

impl SignatureVerifier for TransactionSignatureVerifier {
    fn verify(&self, signature: &Signature) -> bool {
        let hash = self.signer.signature_hash(self.input_index);
        let pubkey = signature.recover_schnorr(&hash).unwrap();
        pubkey.verify_schnorr(&hash, signature).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::super::sign::TransactionInputSigner;
    use super::*;
    use bigint::H256;
    use core::script::Script;
    use core::transaction::{CellInput, CellOutput, OutPoint, Transaction};
    use crypto::secp::Generator;

    #[test]
    fn check_signature() {
        let inputs = vec![CellInput::new(OutPoint::null(), Script::default())];
        let outputs = vec![CellOutput::new(500, Vec::new(), H256::from(0))];
        let tx = Transaction::new(0, Vec::new(), inputs, outputs);
        let signer: TransactionInputSigner = tx.into();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let signature = signer.signed_input(&privkey, 0).unlock.arguments[0]
            .clone()
            .into();

        let verifier = TransactionSignatureVerifier {
            signer,
            input_index: 0,
        };

        assert!(verifier.verify(&signature));
    }
}
