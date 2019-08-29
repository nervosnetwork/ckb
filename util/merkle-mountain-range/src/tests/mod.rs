mod test_accumulate_headers;
mod test_mmr;

use blake2b_rs::{Blake2b, Blake2bBuilder};

fn new_blake2b() -> Blake2b {
    Blake2bBuilder::new(32).build()
}
