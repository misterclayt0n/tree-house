use std::num::NonZeroUsize;
use std::path::PathBuf;

use anyhow::Context;

use crate::flags;

impl flags::Build {
    pub fn run(self) -> anyhow::Result<()> {
        let repo = self
            .repo
            .canonicalize()
            .with_context(|| format!("failed to access {}", self.repo.display()))?;
        let config = skidder::Config {
            repos: vec![skidder::Repo::Local { path: repo }],
            index: PathBuf::new(),
            verbose: self.verbose,
        };
        if let Some(grammar) = self.grammar {
            skidder::build_grammar(&config, &grammar, self.force)?;
        } else {
            skidder::build_all_grammars(
                &config,
                self.force,
                self.threads.and_then(NonZeroUsize::new),
            )?;
        }
        Ok(())
    }
}
