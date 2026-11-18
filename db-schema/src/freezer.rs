//! On-disk names for the replaceable payload columns.
use super::*;

/// Logical columns reclaimed together.
pub const PAYLOAD_COLUMNS: [Col; 5] = [
    COLUMN_BLOCK_BODY,
    COLUMN_BLOCK_UNCLE,
    COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_NUMBER_HASH,
    COLUMN_BLOCK_EXTENSION,
];
/// Published generation in the default column family; absence means generation zero.
pub const GENERATION_KEY: &[u8] = b"freezer/cf-generation";
const PREFIX: &str = "freezer.";

/// Decode the generation selected by this database view.
pub fn generation(marker: Option<&[u8]>) -> Result<u64, std::array::TryFromSliceError> {
    marker
        .map(|raw| raw.try_into().map(u64::from_le_bytes))
        .transpose()
        .map(Option::unwrap_or_default)
}

/// Physical name of a rotating payload column.
pub fn physical_name(generation: u64, col: Col) -> String {
    if generation == 0 {
        col.to_owned()
    } else {
        format!("{PREFIX}{generation}.{col}")
    }
}

/// Recognize a payload column, including its generation-zero name.
pub fn logical_name(name: &str) -> Option<Col> {
    let logical = if let Some(suffix) = name.strip_prefix(PREFIX) {
        let (generation, col) = suffix.split_once('.')?;
        let id = generation.parse::<u64>().ok()?;
        if id == 0 || generation != id.to_string() {
            return None;
        }
        col
    } else {
        name
    };
    PAYLOAD_COLUMNS.into_iter().find(|col| *col == logical)
}

/// Versioned separately from the E4 generation. An E4 opener rejects the reserved
/// freezer.epoch.* column namespace instead of silently using an older hot view.
pub const SEGMENTS_KEY: &[u8] = b"freezer/epoch-segments/v1";
const EPOCH_PREFIX: &str = "freezer.epoch.";
const DEFAULT_WIDTH: u64 = 16;

/// Physical CF for block metadata and individually keyed transactions.
pub fn segment_name(start: u64) -> String {
    format!("{EPOCH_PREFIX}{start}")
}

/// Parse the name independently of the manifest. Opening validates its alignment
/// against the width in the same database.
pub fn parse_segment(name: &str) -> Option<u64> {
    let digits = name.strip_prefix(EPOCH_PREFIX)?;
    let start = digits.parse::<u64>().ok()?;
    (start.is_multiple_of(8) && digits == start.to_string()).then_some(start)
}

/// A view's epoch layout. Empty windows select fixed columns. The width is
/// bound on the first publication and travels with the handles on every reopen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segments {
    width: u64,
    starts: Vec<u64>,
}

impl Default for Segments {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            starts: Vec::new(),
        }
    }
}

impl Segments {
    /// Number of epochs per window.
    pub fn width(&self) -> u64 {
        self.width
    }

    /// First epoch of the window containing this epoch.
    pub fn start(&self, epoch: u64) -> u64 {
        epoch - epoch % self.width
    }

    /// All live windows, in publication order.
    pub fn windows(&self) -> &[u64] {
        &self.starts
    }

    /// The last window stays writable. Earlier windows can never gain members.
    pub fn active(&self) -> Option<u64> {
        self.starts.last().copied()
    }

    /// Sealed windows entirely below the exclusive retention boundary.
    pub fn eligible(&self, before_epoch: u64) -> &[u64] {
        let Some((_, sealed)) = self.starts.split_last() else {
            return &[];
        };
        &sealed[..sealed.partition_point(|&start| {
            start
                .checked_add(self.width)
                .is_some_and(|end| end <= before_epoch)
        })]
    }

    /// Add a later window. Skipped windows do not need empty CFs.
    pub fn open(&self, start: u64) -> Option<Self> {
        if self.start(start) != start || self.active().is_some_and(|last| start <= last) {
            return None;
        }
        let mut next = self.clone();
        next.starts.push(start);
        Some(next)
    }

    /// Retire a sealed window, never the current write destination.
    pub fn retire(&self, start: u64) -> Option<Self> {
        let index = self.starts.binary_search(&start).ok()?;
        if index + 1 == self.starts.len() {
            return None;
        }
        let mut next = self.clone();
        next.starts.remove(index);
        Some(next)
    }

    /// The fixed layout has no marker. ROW2 stores a width and at least one start.
    pub fn encode(&self) -> Option<Vec<u8>> {
        self.active()?;
        let mut raw = Vec::with_capacity(12 + 8 * self.starts.len());
        raw.extend_from_slice(b"ROW2");
        raw.extend_from_slice(&self.width.to_le_bytes());
        raw.extend(self.starts.iter().flat_map(|start| start.to_le_bytes()));
        Some(raw)
    }

    /// Use the requested width for first activation, or validate it against an
    /// existing layout. ROW1 did not record its width and cannot be inferred.
    pub fn decode(raw: Option<&[u8]>, requested: Option<u64>) -> Result<Self, &'static str> {
        if requested.is_some_and(|width| !matches!(width, 8 | 16)) {
            return Err("epoch segment width must be 8 or 16");
        }
        let Some(raw) = raw else {
            return Ok(Self {
                width: requested.unwrap_or(DEFAULT_WIDTH),
                starts: Vec::new(),
            });
        };
        if raw.starts_with(b"ROW1") {
            return Err(
                "ROW1 has no recorded width; use its matching binary or a verified offline import",
            );
        }
        let (width, starts) = raw
            .strip_prefix(b"ROW2")
            .and_then(|bytes| bytes.split_first_chunk::<8>())
            .ok_or("invalid or unsupported epoch layout")?;
        let width = u64::from_le_bytes(*width);
        if !matches!(width, 8 | 16) || requested.is_some_and(|w| w != width) {
            return Err("unsupported or conflicting persisted epoch segment width");
        }
        let (chunks, rest) = starts.as_chunks::<8>();
        if chunks.is_empty() || !rest.is_empty() {
            return Err("invalid epoch segment manifest length");
        }
        let starts: Vec<_> = chunks.iter().copied().map(u64::from_le_bytes).collect();
        if starts.iter().any(|s| !s.is_multiple_of(width))
            || starts.windows(2).any(|p| p[0] >= p[1])
        {
            return Err("invalid epoch segment order or boundary");
        }
        Ok(Self { width, starts })
    }
}

/// A nine-byte hot route cannot be mistaken for an archive record (8 + 32n)
/// or the eight-byte fixed-column override. The route is atomic with the payload.
pub fn segment_route(start: u64) -> [u8; 9] {
    let mut raw = [0; 9];
    raw[0] = b'E';
    raw[1..].copy_from_slice(&start.to_le_bytes());
    raw
}

/// Decode a hot route from the exact-hash index. All other formats go through the archive decoder.
pub fn routed_segment(raw: &[u8]) -> Option<u64> {
    let (tag, bytes) = raw.split_first()?;
    let start = u64::from_le_bytes(bytes.try_into().ok()?);
    (*tag == b'E').then_some(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_is_bound_even_when_the_same_starts_fit_both_widths() {
        for width in [8, 16] {
            let layout = Segments::decode(None, Some(width))
                .unwrap()
                .open(0)
                .unwrap()
                .open(16)
                .unwrap();
            let mut expected = b"ROW2".to_vec();
            expected.extend([width, 0, 16].into_iter().flat_map(u64::to_le_bytes));
            assert_eq!(layout.encode().unwrap(), expected);
            assert_eq!(Segments::decode(Some(&expected), None).unwrap(), layout);
            assert!(Segments::decode(Some(&expected), Some(24 - width)).is_err());
            assert_eq!(layout.eligible(width - 1), []);
            assert_eq!(layout.eligible(width), [0]);
        }
        assert!(Segments::default().encode().is_none());
        for bad in [
            b"ROW1".as_slice(),
            b"ROW3",
            b"ROW2",
            b"ROW2\x08\0\0\0\0\0\0\0",
        ] {
            assert!(Segments::decode(Some(bad), None).is_err());
        }
        for fields in [vec![7, 0], vec![16, 8], vec![8, 8, 8], vec![16, 32, 16]] {
            let mut bytes = b"ROW2".to_vec();
            bytes.extend(fields.into_iter().flat_map(u64::to_le_bytes));
            assert!(Segments::decode(Some(&bytes), None).is_err());
        }
    }
}
