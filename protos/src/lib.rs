mod convert;
pub use convert::{CanBuild, DataBuilder};

mod error;
pub use error::{Error, Result};

mod generated;
pub use generated::*;

pub const DEP_TYPE_CELL: u8 = 0;
pub const DEP_TYPE_CELL_WITH_HEADER: u8 = 1;
pub const DEP_TYPE_DEP_GROUP: u8 = 2;
pub const DEP_TYPE_HEADER: u8 = 3;
