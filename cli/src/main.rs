use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::exit;

use anyhow::Context;

mod flags;
mod import;

fn wrapped_main() -> anyhow::Result<()> {
    let flags = flags::Skidder::from_env_or_exit();
    match flags.subcommand {
        flags::SkidderCmd::Import(import_cmd) => import_cmd.run(),
        flags::SkidderCmd::Build(build_command) => {
            let repo = build_command
                .repo
                .canonicalize()
                .with_context(|| format!("failed to access {}", build_command.repo.display()))?;
            let config = skidder::Config {
                repos: vec![skidder::Repo::Local { path: repo }],
                index: PathBuf::new(),
                verbose: build_command.verbose,
            };
            if let Some(grammar) = build_command.grammar {
                skidder::build_grammar(&config, &grammar, build_command.force)?;
            } else {
                skidder::build_all_grammars(
                    &config,
                    build_command.force,
                    build_command.threads.and_then(NonZeroUsize::new),
                )?;
            }
            Ok(())
        }
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
