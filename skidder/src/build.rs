use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{bail, ensure, Context, Result};
use flate2::read::DeflateDecoder;
use sha1::{Digest, Sha1};
use tempfile::TempDir;

use crate::{Metadata, LIB_EXTENSION};

type Checksum = [u8; 20];
fn is_fresh(grammar_dir: &Path, files: &[&str], force: bool) -> Result<(Checksum, bool)> {
    let src_dir = grammar_dir.join("src");
    let cookie = grammar_dir.join(".BUILD_COOKIE");
    let mut hasher = Sha1::new();
    for file in files {
        let path = src_dir.join(file);
        if !path.exists() {
            continue;
        }
        hasher.update(file.as_bytes());
        hasher.update([0, 0, 0, 0]);
        fs::File::open(&path)
            .and_then(|mut file| std::io::copy(&mut file, &mut hasher))
            .with_context(|| format!("failed to read {}", path.display()))?;
        hasher.update([0, 0, 0, 0]);
    }
    let checksum = hasher.finalize();
    if force {
        return Ok((checksum.into(), false));
    }
    let Ok(prev_checksum) = fs::read(cookie) else {
        return Ok((checksum.into(), false));
    };
    Ok((checksum.into(), prev_checksum == checksum[..]))
}

#[cfg(not(windows))]
const SCANNER_OBJECT: &str = "scanner.o";
#[cfg(windows)]
const SCANNER_OBJECT: &str = "scanner.obj";
const BUILD_TARGET: &str = env!("BUILD_TARGET");
static CPP_COMPILER: OnceLock<cc::Tool> = OnceLock::new();
static C_COMPILER: OnceLock<cc::Tool> = OnceLock::new();

enum CompilerCommand {
    Build,
    BuildAndLink { obj_files: Vec<&'static str> },
}
impl CompilerCommand {
    pub fn setup(self, build_dir: &Path, src_dir: &Path, file: &Path, out_file: &str) -> Command {
        let cpp = file.extension().is_some_and(|ext| ext == "cc");
        let compiler = if cpp {
            CPP_COMPILER.get_or_init(|| {
                cc::Build::new()
                    .cpp(true)
                    .opt_level(3)
                    .std("c++14")
                    .debug(false)
                    .cargo_metadata(false)
                    .host(BUILD_TARGET)
                    .target(BUILD_TARGET)
                    .get_compiler()
            })
        } else {
            C_COMPILER.get_or_init(|| {
                cc::Build::new()
                    .cpp(false)
                    .debug(false)
                    .opt_level(3)
                    .std("c11")
                    .cargo_metadata(false)
                    .host(BUILD_TARGET)
                    .target(BUILD_TARGET)
                    .get_compiler()
            })
        };
        let mut cmd = compiler.to_command();
        cmd.current_dir(build_dir);
        if compiler.is_like_msvc() {
            cmd.args(["/nologo", "/LD", "/utf-8", "/I"]).arg(src_dir);
            match self {
                CompilerCommand::Build => {
                    cmd.arg(format!("/Fo{out_file}")).arg("/c").arg(file);
                }
                CompilerCommand::BuildAndLink { obj_files } => {
                    cmd.args(obj_files)
                        .arg(file)
                        .arg("/link")
                        .arg(format!("/out:{out_file}"));
                }
            }
        } else {
            cmd.args(["-shared", "-fPIC", "-fno-exceptions", "-o", out_file, "-I"])
                .arg(src_dir);
            if cfg!(all(
                unix,
                not(any(target_os = "macos", target_os = "illumos"))
            )) {
                cmd.arg("-Wl,-z,relro,-z,now");
            }
            match self {
                CompilerCommand::Build => {
                    cmd.arg("-c");
                }
                CompilerCommand::BuildAndLink { obj_files } => {
                    cmd.args(obj_files);
                }
            }
            cmd.arg(file);
        };
        cmd
    }
}

pub fn build_grammar(grammar_name: &str, grammar_dir: &Path, force: bool) -> Result<()> {
    let src_dir = grammar_dir.join("src");
    let mut parser = src_dir.join("parser.c");
    ensure!(
        parser.exists(),
        "failed to compile {grammar_name}: {} not found!",
        parser.display()
    );
    let (hash, fresh) = is_fresh(grammar_dir, &["parser.c", "scanner.c", "scanner.cc"], force)?;
    if fresh {
        return Ok(());
    }
    let build_dir = TempDir::new().context("failed to create temporary build directory")?;
    let metadata = Metadata::read(&grammar_dir.join("metadata.json"))
        .with_context(|| format!("failed to read metadata for {grammar_name}"))?;
    assert!(metadata.compressed);
    if metadata.compressed {
        let decompressed_parser = build_dir.path().join(format!("{grammar_name}.c"));
        let mut dst = File::create(&decompressed_parser).with_context(|| {
            format!(
                "failed to create parser.c file in temporary build directory {}",
                build_dir.path().display()
            )
        })?;
        File::open(&parser)
            .map(DeflateDecoder::new)
            .and_then(|mut reader| io::copy(&mut reader, &mut dst))
            .with_context(|| {
                format!("failed to decompress parser {}", build_dir.path().display())
            })?;
        parser = decompressed_parser;
    }
    let mut commands = Vec::new();
    let mut obj_files = Vec::new();
    if src_dir.join("scanner.c").exists() {
        let scanner_cmd = CompilerCommand::Build.setup(
            build_dir.path(),
            &src_dir,
            &src_dir.join("scanner.c"),
            SCANNER_OBJECT,
        );
        obj_files.push(SCANNER_OBJECT);
        commands.push(scanner_cmd)
    } else if src_dir.join("scanner.cc").exists() {
        let scanner_cmd = CompilerCommand::Build.setup(
            build_dir.path(),
            &src_dir,
            &src_dir.join("scanner.cc"),
            SCANNER_OBJECT,
        );
        obj_files.push(SCANNER_OBJECT);
        commands.push(scanner_cmd)
    }
    let lib_name = format!("{grammar_name}.{LIB_EXTENSION}");
    let parser_cmd = CompilerCommand::BuildAndLink { obj_files }.setup(
        build_dir.path(),
        &src_dir,
        &parser,
        &lib_name,
    );
    commands.push(parser_cmd);

    for mut cmd in commands {
        let output = cmd.output().context("Failed to execute compiler")?;
        if !output.status.success() {
            bail!(
                "Parser compilation failed.\nStdout: {}\nStderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    fs::copy(
        build_dir.path().join(lib_name),
        grammar_dir.join(grammar_name).with_extension(LIB_EXTENSION),
    )
    .context("failed to create library")?;
    let _ = fs::write(grammar_dir.join(".BUILD_COOKIE"), hash);
    Ok(())
}
