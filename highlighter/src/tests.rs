use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use indexmap::{IndexMap, IndexSet};
use indoc::indoc;
use once_cell::unsync::OnceCell;
use pretty_assertions::StrComparison;
use ropey::Rope;
use skidder::Repo;
use tree_sitter::Grammar;

use crate::config::{LanguageConfig, LanguageLoader};
use crate::fixtures::roundtrip_fixture;
use crate::highlighter::{Highlight, HighlightQuery};
use crate::injections_query::{InjectionLanguageMarker, InjectionsQuery};
use crate::{Language, Syntax};

fn skidder_config() -> skidder::Config {
    skidder::Config {
        repos: vec![Repo::Git {
            name: "helix-language-support".to_owned(),
            remote: "git@github.com:helix-editor/tree-sitter-grammars.git".into(),
            branch: "main".into(),
        }],
        index: Path::new("../test-grammars").canonicalize().unwrap(),
        verbose: true,
    }
}

fn get_grammar(grammar: &str) -> LanguageConfig {
    let skidder_config = skidder_config();
    let grammar_dir = skidder_config.grammar_dir(grammar).unwrap();
    let new_precedance = skidder::use_new_precedance(&skidder_config, grammar).unwrap();
    let parser_path = skidder::build_grammar(&skidder_config, grammar, false).unwrap();
    let grammar = unsafe { Grammar::new(grammar, &parser_path).unwrap() };
    let highlights_query_path = grammar_dir.join("highlights.scm");
    let highight_query = HighlightQuery::new(
        grammar,
        &highlights_query_path,
        &fs::read_to_string(&highlights_query_path).unwrap(),
    )
    .unwrap();
    let injections_query_path = grammar_dir.join("injections.scm");
    if !injections_query_path.exists() {
        println!("skipping {injections_query_path:?}");
    }
    let injections_query = InjectionsQuery::new(
        grammar,
        &injections_query_path,
        &fs::read_to_string(&injections_query_path).unwrap_or_default(),
    )
    .unwrap();
    LanguageConfig {
        grammar,
        highight_query,
        injections_query,
        new_precedance,
    }
}

#[derive(Debug)]
struct TestLanguageLoader {
    // this would be done with something like IndexMap normally but I don't want to pull that in for a test
    languages: IndexMap<String, Language>,
    lang_config: Box<[OnceCell<LanguageConfig>]>,
    test_theme: RefCell<IndexSet<String>>,
}

impl TestLanguageLoader {
    fn new() -> Self {
        let skidder_config = skidder_config();
        // skidder::fetch(&skidder_config, false).unwrap();
        let grammars = skidder::list_grammars(&skidder_config).unwrap();
        let mut loader = TestLanguageLoader {
            lang_config: (0..grammars.len()).map(|_| OnceCell::new()).collect(),
            languages: grammars
                .into_iter()
                .enumerate()
                .map(|(i, grammar)| {
                    (
                        grammar.file_name().unwrap().to_str().unwrap().to_owned(),
                        Language::new(i as u32),
                    )
                })
                .collect(),
            test_theme: RefCell::default(),
        };
        loader.languages.insert(
            "markdown.inline".into(),
            loader.languages["markdown-inline"],
        );
        loader
    }

    fn get(&mut self, name: &str) -> Language {
        self.languages[name]
    }
}

impl LanguageLoader for TestLanguageLoader {
    fn load_language(&self, marker: &InjectionLanguageMarker) -> Option<Language> {
        let InjectionLanguageMarker::Name(name) = marker else {
            return None;
        };
        self.languages.get(&**name).copied()
    }

    fn get_config(&self, lang: Language) -> &LanguageConfig {
        self.lang_config[lang.idx()].get_or_init(|| {
            let mut config = get_grammar(self.languages.get_index(lang.idx()).unwrap().0);
            let mut theme = self.test_theme.borrow_mut();
            config
                .highight_query
                .configure(|scope| Highlight(theme.insert_full(scope.to_owned()).0 as u32));
            config
        })
    }
}

fn fixture(loader: &mut TestLanguageLoader, fixture: impl AsRef<Path>) {
    let path = Path::new("../fixtures").join(fixture);
    let snapshot = fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    let snapshot = snapshot.trim_end();
    let lang = match path
        .extension()
        .and_then(|it| it.to_str())
        .unwrap_or_default()
    {
        "rs" => loader.get("rust"),
        extension => unreachable!("unkown file type .{extension}"),
    };
    let roundtrip = roundtrip_fixture(
        "// ",
        lang,
        loader,
        |highlight| loader.test_theme.borrow()[highlight.0 as usize].clone(),
        snapshot,
        |_| ..,
    );
    if snapshot != roundtrip.trim_end() {
        if std::env::var_os("UPDATE_EXPECT").is_some_and(|it| it == "1") {
            println!("\x1b[1m\x1b[92mupdating\x1b[0m: {}", path.display());
            fs::write(path, roundtrip).unwrap();
        } else {
            println!(
                "\n
{}

\x1b[1m\x1b[91merror\x1b[97m: fixture test failed\x1b[0m
   \x1b[1m\x1b[34m-->\x1b[0m {}

You can update all fixtures by running:

    env UPDATE_EXPECT=1 cargo test
",
                StrComparison::new(snapshot, &roundtrip.trim_end()),
                path.display(),
            );
        }

        std::panic::resume_unwind(Box::new(()));
    }
    pretty_assertions::assert_str_eq!(
        snapshot,
        roundtrip.trim_end(),
        "fixture {} out of date, set UPDATE_EXPECT=1",
        path.display()
    );
}

#[test]
fn highlight() {
    let mut loader = TestLanguageLoader::new();
    fixture(&mut loader, "highlighter/hellow_world.rs");
}

#[test]
fn combined_injection() {
    let mut loader = TestLanguageLoader::new();
    fixture(&mut loader, "highlighter/combined_injections.rs");
}
