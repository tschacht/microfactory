use std::{fs, path::Path};

use crate::core::ports::FileSystem;
use crate::core::{Error, Result};

#[derive(Debug, Default)]
pub struct StdFileSystem;

impl StdFileSystem {
    pub fn new() -> Self {
        Self
    }
}

impl FileSystem for StdFileSystem {
    fn read_to_string(&self, path: &Path) -> Result<String> {
        fs::read_to_string(path).map_err(|e| Error::FileSystem(e.to_string()))
    }

    fn write(&self, path: &Path, content: &str) -> Result<()> {
        fs::write(path, content).map_err(|e| Error::FileSystem(e.to_string()))
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path).map_err(|e| Error::FileSystem(e.to_string()))
    }
}
