use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, ensure, Context, Result};
use skidder::{decompress, Metadata};
use tempfile::TempDir;

use crate::collect_grammars;
use crate::flags::RegenerateParser;
use crate::import::import_compressed;

impl RegenerateParser {
    pub fn run(self) -> Result<()> {
        let paths = if self.recursive {
            collect_grammars(&self.path)?
        } else {
            vec![self.path.clone()]
        };
        let temp_dir =
            TempDir::new().context("failed to create temporary directory for decompression")?;
        // create dummy file to prevent TS cli from creating a full sceleton
        File::create(temp_dir.path().join("grammar.js"))
            .context("failed to create temporary directory for decompression")?;
        let mut failed = false;
        for grammar_dir in paths {
            let grammar_name = grammar_dir.file_name().unwrap().to_str().unwrap();
            if grammar_name <= "dart" {
                continue;
            }
            println!("checking {grammar_name}");

            let compressed = Metadata::read(&grammar_dir.join("metadata.json"))
                .with_context(|| format!("failed to read metadata for {grammar_name}"))?
                .parser_definition()
                .unwrap()
                .compressed;

            let src_path = grammar_dir.join("src");
            let grammar_path = temp_dir.path().join("grammar.json");
            if compressed {
                let dst = File::create(&grammar_path).with_context(|| {
                    format!(
                        "failed to create grammr.json file in temporary build directory {}",
                        temp_dir.path().display()
                    )
                })?;
                decompress_file(&src_path.join("grammar.json"), dst).with_context(|| {
                    format!("failed to decompress grammar.json for {grammar_name}")
                })?;
            } else {
                fs::copy(src_path.join("grammar.json"), &grammar_path)
                    .with_context(|| format!("failed to copy grammar.json for {grammar_name}"))?;
            }
            println!("running tree-sitter generate {}", grammar_path.display());
            let res = Command::new("tree-sitter")
                .arg("generate")
                .arg("--no-bindings")
                .arg(&grammar_path)
                .current_dir(temp_dir.path())
                .status()
                .with_context(|| {
                    format!(
                        "failed to execute tree-sitter generate {}",
                        grammar_path.display()
                    )
                })?
                .success();
            if !res {
                bail!(
                    "failed to execute tree-sitter generate {}",
                    grammar_path.display()
                )
            }

            let new_parser_path = temp_dir.path().join("src").join("parser.c");
            let old_parser_path = src_path.join("parser.c");
            let mut old_parser = Vec::new();
            decompress_file(&old_parser_path, &mut old_parser)
                .with_context(|| format!("failed to decompress parser for {grammar_name}"))?;
            let old_parser = String::from_utf8_lossy(&old_parser);
            let new_parser = fs::read_to_string(&new_parser_path)
                .context("tree-sitter cli did not generate parser.c")?;
            if old_parser.trim() == new_parser.trim() {
                continue;
            }
            failed = true;
            eprintln!("existing parser.c was outdated updating...");
            if compressed {
                import_compressed(&new_parser_path, &old_parser_path).with_context(|| {
                    format!("failed to compress new parser.c for {grammar_name}")
                })?;
            } else {
                fs::copy(&new_parser_path, &old_parser_path)
                    .with_context(|| format!("failed to opy new parser.c for {grammar_name}"))?;
            }
        }
        ensure!(!failed, "one or more parser.c files is not up to date!");
        Ok(())
    }
}

fn decompress_file(src: &Path, dst: impl Write) -> Result<()> {
    File::open(src)
        .map_err(anyhow::Error::from)
        .and_then(|mut reader| decompress(&mut reader, dst))
        .with_context(|| format!("failed to decompress {}", src.display()))?;
    Ok(())
}
