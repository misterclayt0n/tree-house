use std::io::Write;
use std::path::Path;

use anyhow::Context;

use crate::flags::InitRepo;

impl InitRepo {
    pub fn run(self) -> anyhow::Result<()> {
        append(&self.repo.join(".gitignore"), "*/*.so\n*/.BUILD_COOKIE\n")?;
        append(
            &self.repo.join(".gitattributes"),
            "*/src/parser.c binary\n*/src/grammar.json binary\n",
        )?;
        Ok(())
    }
}

fn append(path: &Path, contents: &str) -> anyhow::Result<()> {
    std::fs::File::options()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(contents.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))
}
