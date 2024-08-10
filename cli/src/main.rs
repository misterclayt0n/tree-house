use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::exit;

use anyhow::Context;

mod build;
mod flags;
mod import;
mod init;

fn wrapped_main() -> anyhow::Result<()> {
    let flags = flags::Skidder::from_env_or_exit();
    match flags.subcommand {
        flags::SkidderCmd::Import(import_cmd) => import_cmd.run(),
        flags::SkidderCmd::Build(build_cmd) => build_cmd.run(),
        flags::SkidderCmd::InitRepo(init_cmd) => init_cmd.run(),
    }
}

pub fn main() {
    if let Err(err) = wrapped_main() {
        for error in err.chain() {
            eprintln!("error: {error}")
        }
        exit(1)
    }
}
