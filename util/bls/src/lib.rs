#[link(name = "pbc")]
#[link(name = "gmp")]
extern "C" {
    pub fn sign_c(out: *mut u8, out_len: *mut usize, msg: *mut u8, msg_len: usize, data: *mut u8);
    pub fn verify_c(
        msg: *mut u8,
        msg_len: usize,
        data_s: *mut u8,
        data_g: *mut u8,
        data_p: *mut u8,
    ) -> i32;
    pub fn key_gen_c(
        out_sk: *mut u8,
        sk_len: *mut usize,
        out_pk: *mut u8,
        pk_len: *mut usize,
        out_g: *mut u8,
        g_len: *mut usize,
    );
}

//sign the msg, only msg[0..20] is used.
pub fn sign(mut msg: Vec<u8>, mut private_key: Vec<u8>) -> Vec<u8> {
    unsafe {
        let mut sig = Vec::with_capacity(30);
        let mut sig_len = 0usize;
        let msg_len = msg.len();

        sign_c(
            sig.as_mut_ptr(),
            &mut sig_len,
            msg.as_mut_ptr(),
            msg_len,
            private_key.as_mut_ptr(),
        );

        sig.set_len(sig_len);
        sig
    }
}

pub fn verify(mut msg: Vec<u8>, mut sig: Vec<u8>, mut public_key: Vec<u8>, mut g: Vec<u8>) -> bool {
    unsafe {
        let msg_len = msg.len();

        let r = verify_c(
            msg.as_mut_ptr(),
            msg_len,
            sig.as_mut_ptr(),
            g.as_mut_ptr(),
            public_key.as_mut_ptr(),
        );

        r != 0
    }
}

pub fn key_gen() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    unsafe {
        let mut private_key = Vec::with_capacity(30);
        let mut private_len = 0usize;
        let mut public_key = Vec::with_capacity(40);
        let mut public_len = 0usize;
        let mut g = Vec::with_capacity(40);
        let mut g_len = 0usize;

        key_gen_c(
            private_key.as_mut_ptr(),
            &mut private_len,
            public_key.as_mut_ptr(),
            &mut public_len,
            g.as_mut_ptr(),
            &mut g_len,
        );

        private_key.set_len(private_len);
        public_key.set_len(public_len);
        g.set_len(g_len);
        (private_key, public_key, g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_and_verify() {
        let (private_key, public_key, g) = key_gen();
        let msg = vec![1; 20];
        let sig = sign(msg.clone(), private_key.clone());
        let r = verify(msg, sig.clone(), public_key.clone(), g.clone());
        assert_eq!(r, true);

        let msg = vec![2; 20];
        let r = verify(msg, sig.clone(), public_key.clone(), g.clone());
        assert_eq!(r, false);
    }

}
