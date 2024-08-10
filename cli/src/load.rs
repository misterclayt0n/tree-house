use std::ffi::c_void;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use libloading::Symbol;

use crate::flags::LoadGrammar;

impl LoadGrammar {
    pub fn run(self) -> Result<()> {
        let paths = if self.recursive {
            fs::read_dir(&self.path)?
                .filter_map(|dent| {
                    let dent = match dent {
                        Ok(dent) => dent,
                        Err(err) => return Some(Err(err.into())),
                    };
                    if !dent.file_type().is_ok_and(|file| file.is_dir()) {
                        return None;
                    }
                    // this is an internal tool I dont' care about windows
                    let path = dent.path().join(dent.file_name()).with_extension("so");
                    path.exists().then_some(Ok(path))
                })
                .collect::<Result<Vec<PathBuf>>>()
                .with_context(|| format!("failed to read directory {}", self.path.display()))?
        } else {
            vec![self.path.clone()]
        };
        for path in paths {
            let Some(name) = path.file_stem().unwrap().to_str() else {
                continue;
            };
            println!("loading {}", path.display());
            unsafe {
                let lib = libloading::Library::new(&path)
                    .with_context(|| format!("failed to load {}", path.display()))?;
                let language_fn_name = format!("tree_sitter_{}", name.replace('-', "_"));
                let _language_fn: Symbol<unsafe extern "C" fn() -> *mut c_void> = lib
                    .get(language_fn_name.as_bytes())
                    .with_context(|| format!("failed to load {}", path.display()))?;
            }
        }
        Ok(())
    }
}
