use std::io::Write;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{self, AtomicUsize};
use std::sync::Mutex;
use std::time::Duration;
use std::{fs, io, thread};

use anyhow::{bail, ensure, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};

#[cfg(not(windows))]
const LIB_EXTENSION: &str = "so";
#[cfg(windows)]
const LIB_EXTENSION: &str = "dll";

mod build;

pub struct Config {
    pub repos: Vec<Repo>,
    pub index: PathBuf,
    pub verbose: bool,
}

impl Config {
    pub fn git(&self, args: &[&str], dir: &Path) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        if self.verbose {
            println!("{}: git {}", dir.display(), args.join(" "))
        }
        let status = if self.verbose {
            cmd.status().context("failed to invoke git")?
        } else {
            let res = cmd.output().context("failed to invoke git")?;
            if !res.status.success() {
                let _ = io::stdout().write_all(&res.stdout);
                let _ = io::stderr().write_all(&res.stderr);
            }
            res.status
        };
        if !status.success() {
            bail!("git returned non-zero exit-code: {status}");
        }
        Ok(())
    }

    pub fn git_exit_with(&self, args: &[&str], dir: &Path, exitcode: i32) -> Result<bool> {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        if self.verbose {
            println!("{}: git {}", dir.display(), args.join(" "))
        }
        if !self.verbose {
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
        }
        let status = cmd.status().context("failed to invoke git")?;
        if status.code() == Some(exitcode) {
            return Ok(true);
        }
        if !status.success() {
            bail!("git returned unexpected exit-code: {status}");
        }
        Ok(false)
    }

    pub fn git_output(&self, args: &[&str], dir: &Path) -> Result<String> {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        if self.verbose {
            println!("{}: git {}", dir.display(), args.join(" "))
        }
        let res = cmd.output().context("failed to invoke git")?;
        if !res.status.success() {
            let _ = io::stdout().write_all(&res.stdout);
            let _ = io::stderr().write_all(&res.stderr);
            bail!("git returned non-zero exit-code: {}", res.status);
        }
        String::from_utf8(res.stdout).context("git returned invalid utf8")
    }
}

pub enum Repo {
    Git {
        name: String,
        remote: String,
        branch: String,
    },
    Local {
        path: PathBuf,
    },
}

impl Repo {
    fn dir(&self, config: &Config) -> PathBuf {
        match self {
            Repo::Git { name, .. } => config.index.join(name),
            Repo::Local { path } => path.clone(),
        }
    }

    pub fn has_grammar(&self, config: &Config, grammar: &str) -> bool {
        self.dir(config)
            .join(grammar)
            .join("metadata.json")
            .exists()
    }

    pub fn read_metadata(&self, config: &Config, grammar: &str) -> Result<Metadata> {
        let path = self.dir(config).join(grammar).join("metadata.json");
        Metadata::read(&path).with_context(|| format!("failed to read metadata for {grammar}"))
    }

    pub fn list_grammars(&self, config: &Config) -> Result<Vec<PathBuf>> {
        fs::read_dir(self.dir(config))
            .context("failed to acces repository")?
            .map(|dent| {
                let dent = dent?;
                if !dent.file_type()?.is_dir() || dent.file_name().to_str().is_none() {
                    return Ok(None);
                }
                let path = dent.path();
                if !path.join("metadata.json").exists() {
                    return Ok(None);
                }
                Ok(Some(dent.path()))
            })
            .filter_map(|res| res.transpose())
            .collect()
    }

    pub fn fetch(&self, config: &Config, update: bool) -> Result<()> {
        let Repo::Git { remote, branch, .. } = self else {
            return Ok(());
        };
        let dir = self.dir(config);
        if dir.join(".git").exists() {
            let current_branch = config.git_output(&["rev-parse", "--abbrev-ref", "HEAD"], &dir)?;
            let switch_branch = current_branch != *branch;
            if !update && !switch_branch {
                return Ok(());
            }
            if switch_branch {
                config.git(&["reset", "--hard"], &dir)?;
                config.git(&["checkout", branch], &dir)?;
            }
            config.git(&["fetch", "origin", branch], &dir)?;
            config.git(&["reset", "--hard", &format!("origin/{}", branch)], &dir)?;
            return Ok(());
        }
        let _ = fs::create_dir_all(&dir);
        ensure!(dir.exists(), "failed to create directory {}", dir.display());
        let Some(dir_str) = dir.as_os_str().to_str() else {
            bail!(
                "could not convert the directory name to a string: {}",
                dir.display()
            )
        };
        // intentionally not doing a shallow clone since that makes
        // incremental updates more exensive, however partial clones are a great
        // fit since that avoids fetching old parsers (which are not very useful)
        config.git(
            &[
                "clone",
                "--single-branch",
                "--filter=blob:none",
                "--branch",
                branch,
                remote,
                &dir_str,
            ],
            &dir,
        )
    }
}

pub fn fetch(config: &Config, update_existing_grammar: bool) -> Result<()> {
    for repo in &config.repos {
        repo.fetch(config, update_existing_grammar)?
    }
    Ok(())
}

pub fn build_grammar(config: &Config, grammar: &str, force_rebuild: bool) -> Result<PathBuf> {
    for repo in &config.repos {
        if repo.has_grammar(config, grammar) {
            build::build_grammar(grammar, &repo.dir(config).join(grammar), force_rebuild)?;
            return Ok(repo
                .dir(config)
                .join(grammar)
                .join(grammar)
                .with_extension(LIB_EXTENSION));
        }
    }
    bail!("grammar not found in any configured repository")
}

pub fn list_grammars(config: &Config) -> Result<Vec<PathBuf>> {
    let mut res = Vec::new();
    for repo in &config.repos {
        res.append(&mut repo.list_grammars(config)?)
    }
    res.sort_by(|path1, path2| path1.file_name().cmp(&path2.file_name()));
    res.dedup_by(|path1, path2| path1.file_name() == path2.file_name());
    Ok(res)
}

pub fn build_all_grammars(
    config: &Config,
    force_rebuild: bool,
    concurrency: Option<NonZeroUsize>,
) -> Result<usize> {
    let grammars = list_grammars(config)?;
    let bar = ProgressBar::new(grammars.len() as u64).with_style(
        ProgressStyle::with_template("{spinner} {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}")
            .unwrap(),
    );
    bar.set_message("Compiling");
    bar.enable_steady_tick(Duration::from_millis(100));
    let i = AtomicUsize::new(0);
    let concurrency = concurrency
        .or_else(|| thread::available_parallelism().ok())
        .map_or(4, usize::from);
    let failed = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..concurrency {
            scope.spawn(|| loop {
                let Some(grammar) = grammars.get(i.fetch_add(1, atomic::Ordering::Relaxed)) else {
                    break;
                };
                let name = grammar.file_name().unwrap().to_str().unwrap();
                if let Err(err) = build::build_grammar(name, grammar, force_rebuild) {
                    for err in err.chain() {
                        bar.println(format!("error: {err}"))
                    }
                    failed.lock().unwrap().push(name.to_owned())
                }
                bar.inc(1);
            });
        }
    });
    let failed = failed.into_inner().unwrap();
    if !failed.is_empty() {
        bail!("failed to build grammars {failed:?}")
    }
    Ok(grammars.len())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Metadata {
    /// The git remote of the query upstreama
    pub repo: String,
    /// The git remote of the query
    pub rev: String,
    /// The SPDX license identifier
    #[serde(default)]
    pub license: String,
    /// Wether to use the new query precedence
    /// where later matches take priority.
    #[serde(default)]
    pub new_precedence: bool,
    #[serde(default)]
    pub compressed: bool,
}

impl Metadata {
    pub fn read(path: &Path) -> Result<Metadata> {
        let json = fs::read_to_string(path)
            .with_context(|| format!("couldn't read {}", path.display()))?;
        serde_json::from_str(&json)
            .with_context(|| format!("invalid metadata.json file at {}", path.display()))
    }
    pub fn write(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(&self).unwrap();
        fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
    }
}
