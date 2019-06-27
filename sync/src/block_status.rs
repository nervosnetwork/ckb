use bitflags::bitflags;

bitflags! {
    pub struct BlockStatus: u32 {
        const UNKNOWN                 =     0;

        const HEADER_VERIFIED         =     1;
        const BLOCK_RECEIVED          =     Self::HEADER_VERIFIED.bits | 2;
        const BLOCK_STORED            =     Self::HEADER_VERIFIED.bits | Self::BLOCK_RECEIVED.bits | 16;

        const BLOCK_INVALID           =    32;
    }
}

#[cfg(test)]
mod tests {
    use crate::block_status::BlockStatus;

    fn all() -> Vec<BlockStatus> {
        vec![
            BlockStatus::UNKNOWN,
            BlockStatus::HEADER_VERIFIED,
            BlockStatus::BLOCK_RECEIVED,
            BlockStatus::BLOCK_STORED,
            BlockStatus::BLOCK_INVALID,
        ]
    }

    fn assert_(includes: Vec<BlockStatus>, target: BlockStatus) {
        let excludes: Vec<BlockStatus> = all()
            .into_iter()
            .filter(|s1| !includes.iter().any(|s2| s2 == s1))
            .collect();
        includes.into_iter().for_each(|status| {
            assert!(
                status.contains(target),
                "{:?} should contains {:?}",
                status,
                target
            )
        });
        excludes.into_iter().for_each(|status| {
            assert!(
                !status.contains(target),
                "{:?} should not contains {:?}",
                status,
                target
            )
        });
    }

    #[test]
    fn test_unknown() {
        assert!(BlockStatus::UNKNOWN.is_empty());
    }

    #[test]
    fn test_header_verified() {
        let target = BlockStatus::HEADER_VERIFIED;
        let includes = vec![
            BlockStatus::HEADER_VERIFIED,
            BlockStatus::BLOCK_RECEIVED,
            BlockStatus::BLOCK_STORED,
        ];
        assert_(includes, target);
    }

    #[test]
    fn test_block_received() {
        let target = BlockStatus::BLOCK_RECEIVED;
        let includes = vec![BlockStatus::BLOCK_RECEIVED, BlockStatus::BLOCK_STORED];
        assert_(includes, target);
    }

    #[test]
    fn test_block_invalid() {
        let target = BlockStatus::BLOCK_INVALID;
        let includes = vec![BlockStatus::BLOCK_INVALID];
        assert_(includes, target);
    }
}
