use std::ffi::c_void;

use anyhow::{Context, Result};
use libloading::Symbol;

use crate::collect_grammars;
use crate::flags::LoadGrammar;

impl LoadGrammar {
    pub fn run(self) -> Result<()> {
        let paths = if self.recursive {
            collect_grammars(&self.path)?
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
