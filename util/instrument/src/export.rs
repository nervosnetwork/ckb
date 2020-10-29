use ckb_chain_iter::ChainIterator;
use ckb_jsonrpc_types::BlockView as JsonBlock;
use ckb_shared::shared::Shared;
#[cfg(feature = "progress_bar")]
use indicatif::{ProgressBar, ProgressStyle};
use std::error::Error;
use std::fs;
use std::io;
use std::io::Write;
use std::path::PathBuf;

/// Export block from datbase to specify file.
pub struct Export {
    /// export target path
    pub target: PathBuf,
    /// TODO(doc): @doitian
    pub shared: Shared,
}

impl Export {
    /// TODO(doc): @doitian
    pub fn new(shared: Shared, target: PathBuf) -> Self {
        Export { shared, target }
    }

    /// export file name
    fn file_name(&self) -> String {
        format!("{}.{}", self.shared.consensus().id, "json")
    }

    /// TODO(doc): @doitian
    pub fn execute(self) -> Result<(), Box<dyn Error>> {
        fs::create_dir_all(&self.target)?;
        self.write_to_json()
    }

    #[cfg(not(feature = "progress_bar"))]
    pub fn write_to_json(self) -> Result<(), Box<dyn Error>> {
        let f = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&self.target.join(self.file_name()))?;
        let mut writer = io::BufWriter::new(f);
        let snapshot = self.shared.snapshot();

        for block in ChainIterator::new(snapshot.as_ref()) {
            let block: JsonBlock = block.into();
            let encoded = serde_json::to_vec(&block)?;
            writer.write_all(&encoded)?;
            writer.write_all(b"\n")?;
        }
        Ok(())
    }

    /// TODO(doc): @doitian
    #[cfg(feature = "progress_bar")]
    pub fn write_to_json(self) -> Result<(), Box<dyn Error>> {
        let f = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&self.target.join(self.file_name()))?;

        let mut writer = io::BufWriter::new(f);
        let snapshot = self.shared.snapshot();
        let blocks_iter = ChainIterator::new(snapshot.as_ref());
        let progress_bar = ProgressBar::new(blocks_iter.len());
        progress_bar.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] {bar:50.cyan/blue} {pos:>6}/{len:6} {msg}")
                .progress_chars("##-"),
        );
        for block in blocks_iter {
            let block: JsonBlock = block.into();
            let encoded = serde_json::to_vec(&block)?;
            writer.write_all(&encoded)?;
            writer.write_all(b"\n")?;
            progress_bar.inc(1);
        }
        progress_bar.finish_with_message("done!");
        Ok(())
    }
}
