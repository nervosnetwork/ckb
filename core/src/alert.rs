use crate::Bytes;
use canonical_serializer::{CanonicalSerialize, CanonicalSerializer, Result as SerializeResult};
use hash::Blake2bWriter;
use numext_fixed_hash::H256;
use std::io::Write;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Alert {
    pub id: u32,
    // cancel id if cancel is greater than 0
    pub cancel: u32,
    // TODO use service flag to distinguish network
    //network: String,
    pub min_version: Option<String>,
    pub max_version: Option<String>,
    pub priority: u32,
    pub notice_until: u64,
    pub message: String,
    pub signatures: Vec<Bytes>,
}

impl CanonicalSerialize for Alert {
    fn serialize<W: Write>(&self, serializer: &mut CanonicalSerializer<W>) -> SerializeResult<()> {
        serializer
            .encode_u32(self.id)?
            .encode_u32(self.cancel)?
            .encode_option_ref(&self.min_version.clone().map(|v| v.as_bytes().to_vec()))?
            .encode_option_ref(&self.max_version.clone().map(|v| v.as_bytes().to_vec()))?
            .encode_u32(self.priority)?
            .encode_u64(self.notice_until)?
            .encode_bytes(self.message.as_bytes())?;
        Ok(())
    }
}

impl Alert {
    pub fn hash(&self) -> H256 {
        let mut hasher = Blake2bWriter::new();
        let mut serializer = CanonicalSerializer::new(&mut hasher);
        self.serialize(&mut serializer)
            .expect("alert canonical serialize");
        hasher.finalize().into()
    }
}

#[derive(Default)]
pub struct AlertBuilder {
    inner: Alert,
}

impl AlertBuilder {
    pub fn alert(mut self, alert: Alert) -> Self {
        self.inner = alert;
        self
    }

    pub fn id(mut self, id: u32) -> Self {
        self.inner.id = id;
        self
    }

    pub fn cancel(mut self, cancel: u32) -> Self {
        self.inner.cancel = cancel;
        self
    }

    pub fn min_version(mut self, min_version: Option<String>) -> Self {
        self.inner.min_version = min_version;
        self
    }

    pub fn max_version(mut self, max_version: Option<String>) -> Self {
        self.inner.max_version = max_version;
        self
    }

    pub fn priority(mut self, priority: u32) -> Self {
        self.inner.priority = priority;
        self
    }

    pub fn signatures(mut self, signatures: Vec<Bytes>) -> Self {
        self.inner.signatures.extend(signatures);
        self
    }

    pub fn notice_until(mut self, notice_until: u64) -> Self {
        self.inner.notice_until = notice_until;
        self
    }

    pub fn message(mut self, message: String) -> Self {
        self.inner.message = message;
        self
    }

    pub fn build(self) -> Alert {
        self.inner
    }
}
