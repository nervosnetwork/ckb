use crate::format::Format;
use crate::iter::ChainIterator;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
#[cfg(feature = "progress_bar")]
use indicatif::{ProgressBar, ProgressStyle};
use serde_json;
use std::error::Error;
use std::fs;
use std::io;
use std::io::Write;
use std::path::PathBuf;

/// Export block from datbase to specify file.
pub struct Export<CS> {
    /// export target path
    pub target: PathBuf,
    pub shared: Shared<CS>,
    /// which format be used to export
    pub format: Format,
}

impl<CS: ChainStore> Export<CS> {
    pub fn new(shared: Shared<CS>, format: Format, target: PathBuf) -> Self {
        Export {
            shared,
            format,
            target,
        }
    }

    /// Returning ChainIterator dealing with blocks iterate.
    pub fn iter(&self) -> ChainIterator<CS> {
        ChainIterator::new(self.shared.clone())
    }

    /// export file name
    fn file_name(&self) -> String {
        format!("{}.{}", self.shared.consensus().id, self.format)
    }

    pub fn execute(self) -> Result<(), Box<Error>> {
        fs::create_dir_all(&self.target)?;
        match self.format {
            Format::Json => self.write_to_json(),
            _ => Ok(()),
        }
    }

    #[cfg(not(feature = "progress_bar"))]
    pub fn write_to_json(self) -> Result<(), Box<Error>> {
        let f = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&self.target.join(self.file_name()))?;
        let mut writer = io::BufWriter::new(f);

        for block in self.iter() {
            let block: Block = block.into();
            let encoded = serde_json::to_vec(&block)?;
            writer.write_all(&encoded)?;
            writer.write_all(b"\n")?;
        }
        Ok(())
    }

    #[cfg(feature = "progress_bar")]
    pub fn write_to_json(self) -> Result<(), Box<Error>> {
        let f = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&self.target.join(self.file_name()))?;
        let mut writer = io::BufWriter::new(f);

        let blocks_iter = self.iter();
        let progress_bar = ProgressBar::new(blocks_iter.len());
        progress_bar.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] {bar:50.cyan/blue} {pos:>6}/{len:6} {msg}")
                .progress_chars("##-"),
        );
        for block in blocks_iter {
            let encoded = serde_json::to_vec(&block)?;
            writer.write_all(&encoded)?;
            writer.write_all(b"\n")?;
            progress_bar.inc(1);
        }
        progress_bar.finish_with_message("done!");
        Ok(())
    }
}
