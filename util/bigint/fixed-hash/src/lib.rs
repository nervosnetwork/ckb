// Copyright 2015-2017 Parity Technologies
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#[doc(hidden)]
pub extern crate libc;

#[cfg(feature = "heapsizeof")]
#[doc(hidden)]
pub extern crate heapsize;

#[cfg(feature = "std")]
#[doc(hidden)]
pub extern crate core;

#[cfg(feature = "std")]
#[doc(hidden)]
pub extern crate rustc_hex;

#[cfg(feature = "std")]
#[doc(hidden)]
pub extern crate rand;

mod hash;
pub use hash::*;
