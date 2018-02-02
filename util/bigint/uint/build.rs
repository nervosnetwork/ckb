// Copyright 2015-2017 Parity Technologies
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

extern crate rustc_version;

use rustc_version::{version_meta, Channel};

fn main() {
    if cfg!(feature = "use_asm") {
        if let Channel::Nightly = version_meta().unwrap().channel {
            println!("cargo:rustc-cfg=asm_available");
        }
    }
}
