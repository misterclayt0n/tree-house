use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{bail, ensure, Context, Result};
use sha1::{Digest, Sha1};
use tempfile::TempDir;

use crate::LIB_EXTENSION;

type Checksum = [u8; 20];
fn is_fresh(grammar_dir: &Path, files: &[&str], force: bool) -> Result<(Checksum, bool)> {
    let cookie = grammar_dir.join(".BUILD_COOKIE");
    let mut hasher = Sha1::new();
    for file in files {
        let path = grammar_dir.join(file);
        if !path.exists() {
            continue;
        }
        hasher.update(file.as_bytes());
        let file = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        hasher.update(file);
        // paddding bytes
        hasher.update(&[0, 0, 0, 0]);
    }
    let checksum = hasher.finalize();
    if force {
        return Ok((checksum.into(), false));
    }
    let Ok(prev_checksum) = fs::read(cookie) else {
        return Ok((checksum.into(), false));
    };
    return Ok((checksum.into(), prev_checksum == checksum[..]));
}

const BUILD_TARGET: &str = env!("BUILD_TARGET");
static CPP_COMPILER: OnceLock<cc::Tool> = OnceLock::new();
static C_COMPILER: OnceLock<cc::Tool> = OnceLock::new();

fn compiler_command(build_dir: &Path, src_dir: &Path, files: &[&str]) -> (Command, PathBuf) {
    let files: Vec<_> = files
        .iter()
        .map(|file| src_dir.join(file))
        .filter(|path| path.exists())
        .collect();
    let cpp = files.iter().any(|file| file.ends_with(".cc"));
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
    let out_file = if compiler.is_like_msvc() {
        cmd.args(["/nologo", "/LD", "/I"])
            .arg(src_dir)
            .arg("/utf-8");
        build_dir.join("parser.dll")
    } else {
        cmd.args(["-shared", "-fPIC", "-fno-exceptions", "-I"])
            .arg(src_dir)
            .arg("-o")
            .arg("parser.so");
        if cfg!(all(
            unix,
            not(any(target_os = "macos", target_os = "illumos"))
        )) {
            cmd.arg("-Wl,-z,relro,-z,now");
        }
        build_dir.join("parser.so")
    };
    cmd.args(files);
    (cmd, out_file)
}

pub fn build_grammar(grammar_name: &str, grammar_dir: &Path, force: bool) -> Result<()> {
    let (hash, fresh) = is_fresh(grammar_dir, &["parser.c", "scanner.c", "scanner.cc"], force)?;
    if fresh {
        return Ok(());
    }
    ensure!(
        grammar_dir.join("parser.c").exists(),
        "fialed to compile {grammar_name}: parser.c not found!"
    );
    let build_dir = TempDir::new().context("fialed to create temporary build dierctory")?;
    let (mut cmd, output_path) = compiler_command(
        build_dir.path(),
        grammar_dir,
        &["parser.c", "scanner.c", "scanner.cc"],
    );

    let output = cmd.output().context("Failed to execute compiler")?;
    if !output.status.success() {
        bail!(
            "Parser compilation failed.\nStdout: {}\nStderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::copy(
        output_path,
        grammar_dir.join(grammar_name).with_extension(LIB_EXTENSION),
    )
    .context("failed to create library")?;
    let _ = fs::write(grammar_dir.join(".BUILD_COOKIE"), &hash);
    Ok(())
}
