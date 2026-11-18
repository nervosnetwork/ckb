use crate::payload::buffer;
use ckb_util::Mutex;
use lru::LruCache;
use std::{io, sync::Arc};

const CAPACITY: usize = 32 * 1024 * 1024;
const ENTRIES: usize = 4096;

/// Reuse decoding only when newly read bytes equal a previously verified input.
/// Files and metadata are still read on every request, including cache hits.
/// One-time reads retain only a checksum, avoiding payload copies during scans.
pub(crate) struct DecodeCache {
    entries: Mutex<Entries>,
}

struct Entries {
    // None is an admission hint, never evidence that bytes have been verified.
    values: LruCache<[u8; 32], Option<Arc<Decoded>>>,
    bytes: usize,
}

struct Decoded {
    encoded: Vec<u8>,
    decoded: Vec<u8>,
}

impl Decoded {
    fn size(&self) -> usize {
        self.encoded.len() + self.decoded.len()
    }
}

impl DecodeCache {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(Entries {
                values: LruCache::new(ENTRIES),
                bytes: 0,
            }),
        }
    }

    /// The callback must verify the checksum and decoded length before success.
    /// Neither verification nor byte comparisons/copies hold the cache lock.
    pub fn decode_into(
        &self,
        encoded: &[u8],
        hash: &[u8; 32],
        output: &mut [u8],
        verify_and_decode: impl FnOnce(&mut [u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        let cached = {
            let mut entries = self.entries.lock();
            let cached = entries.values.get(hash).cloned();
            if cached.is_none()
                && let Some((_, Some(old))) = entries.values.push(*hash, None)
            {
                entries.bytes -= old.size();
            }
            cached
        };
        if let Some(Some(value)) = &cached
            && value.decoded.len() == output.len()
            && value.encoded == encoded
        {
            output.copy_from_slice(&value.decoded);
            return Ok(());
        }
        verify_and_decode(output)?;
        let size = encoded.len().saturating_add(output.len());
        if cached.is_none() || size > CAPACITY {
            return Ok(());
        }
        // Cache allocation is optional; a verified read must still succeed
        // if its extra retained copy cannot be allocated.
        let (Ok(mut input), Ok(mut decoded)) = (buffer(encoded.len()), buffer(output.len())) else {
            return Ok(());
        };
        input.copy_from_slice(encoded);
        decoded.copy_from_slice(output);
        let value = Arc::new(Decoded {
            encoded: input,
            decoded,
        });
        let mut entries = self.entries.lock();
        // Another reader may have admitted this input while we decoded it.
        if matches!(entries.values.peek(hash), Some(Some(_))) {
            return Ok(());
        }
        entries.bytes += value.size();
        if let Some((_, Some(old))) = entries.values.push(*hash, Some(value)) {
            entries.bytes -= old.size();
        }
        while entries.bytes > CAPACITY {
            if let Some((_, Some(old))) = entries.values.pop_lru() {
                entries.bytes -= old.size();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{format::checksum, payload};
    use std::cell::Cell;

    #[test]
    fn reuse_requires_equal_bytes_and_length_and_never_caches_an_error() {
        let cache = DecodeCache::new();
        let raw = b"verified archive bytes";
        let encoded = lz4_flex::block::compress(raw);
        let hash = checksum(&encoded);
        let calls = Cell::new(0);
        let decode = |data: &[u8], output: &mut [u8]| {
            cache.decode_into(data, &hash, output, |output| {
                calls.set(calls.get() + 1);
                payload::decode_into(data, &hash, output)
            })
        };
        let mut output = [0; 22];
        let mut corrupted = encoded.clone();
        corrupted[0] ^= 1;
        assert!(decode(&corrupted, &mut output).is_err());
        assert!(decode(&corrupted, &mut output).is_err());
        assert_eq!(cache.entries.lock().bytes, 0);
        for _ in 0..3 {
            decode(&encoded, &mut output).unwrap();
            assert_eq!(&output, raw);
        }
        assert_eq!(calls.get(), 3); // Two failures, one verified admission, then reuse.
        assert!(decode(&corrupted, &mut output).is_err());
        assert!(decode(&encoded, &mut output[..21]).is_err());
        assert!(decode(&encoded, &mut [0; 23]).is_err());
        decode(&encoded, &mut output).unwrap();
        assert_eq!(&output, raw);
        assert_eq!(calls.get(), 6);
    }

    #[test]
    fn scans_only_retain_bounded_history_and_repeated_reads_respect_the_byte_budget() {
        let cache = DecodeCache::new();
        for _ in 0..2 {
            for number in 0..ENTRIES + 1 {
                let raw = number.to_le_bytes();
                let encoded = lz4_flex::block::compress(&raw);
                let hash = checksum(&encoded);
                let mut output = [0; std::mem::size_of::<usize>()];
                cache
                    .decode_into(&encoded, &hash, &mut output, |output| {
                        payload::decode_into(&encoded, &hash, output)
                    })
                    .unwrap();
                assert_eq!(output, raw);
            }
            let entries = cache.entries.lock();
            assert_eq!(entries.bytes, 0);
            assert!(entries.values.len() <= ENTRIES);
        }
        for number in 0..10 {
            let raw = vec![number; CAPACITY / 8];
            let encoded = lz4_flex::block::compress(&raw);
            let hash = checksum(&encoded);
            for _ in 0..3 {
                let mut output = vec![0; raw.len()];
                cache
                    .decode_into(&encoded, &hash, &mut output, |output| {
                        payload::decode_into(&encoded, &hash, output)
                    })
                    .unwrap();
                assert_eq!(output, raw);
            }
            let entries = cache.entries.lock();
            assert!(entries.bytes <= CAPACITY);
            assert!(entries.values.len() <= ENTRIES);
        }
    }
}
