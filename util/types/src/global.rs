//! Global data, initialized in the launch phase.

use std::path::PathBuf;
use std::sync::OnceLock;

/// ckb data directory path, located under root/data, initialized during the launch phase
pub static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
