//! Global data, initialized in the launch phase.

use once_cell::sync::OnceCell;
use std::path::PathBuf;

/// ckb data directory path, located under root/data, initialized during the launch phase
pub static DATA_DIR: OnceCell<PathBuf> = OnceCell::new();
