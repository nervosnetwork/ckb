use crate::base::ExtraHashView;

impl ::std::fmt::Display for ExtraHashView {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        if let Some((ref extension_hash, ref extra_hash)) = self.extension_hash_and_extra_hash {
            write!(
                f,
                "uncles_hash: {}, extension_hash: {}, extra_hash: {}",
                self.uncles_hash, extension_hash, extra_hash
            )
        } else {
            write!(
                f,
                "uncles_hash: {}, extension_hash: None, extra_hash: uncles_hash",
                self.uncles_hash
            )
        }
    }
}
