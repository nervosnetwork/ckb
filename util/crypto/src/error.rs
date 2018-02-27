#[derive(Debug, Fail)]
pub enum Error {
    #[fail(display = "invalid privkey")] InvalidPrivKey,
    #[fail(display = "invalid pubkey")] InvalidPubKey,
    #[fail(display = "invalid signature")] InvalidSignature,
    #[fail(display = "invalid message")] InvalidMessage,
    #[fail(display = "invalid recovery_id")] InvalidRecoveryId,
    #[fail(display = "{}", _0)] Io(#[cause] ::std::io::Error),
    #[fail(display = "{}", _0)] Other(String),
}
