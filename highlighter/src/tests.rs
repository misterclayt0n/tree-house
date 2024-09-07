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

#[derive(Debug, Clone, Default)]
struct Overwrites {
    highlights: Option<String>,
    injections: Option<String>,
}

fn get_grammar(grammar: &str, overwrites: &Overwrites) -> LanguageConfig {
    let skidder_config = skidder_config();
    let grammar_dir = skidder_config.grammar_dir(grammar).unwrap();
    let new_precedance = skidder::use_new_precedance(&skidder_config, grammar).unwrap();
    let parser_path = skidder::build_grammar(&skidder_config, grammar, false).unwrap();
    let grammar = unsafe { Grammar::new(grammar, &parser_path).unwrap() };
    let highlights_query_path = grammar_dir.join("highlights.scm");
    let highight_query = HighlightQuery::new(
        grammar,
        &highlights_query_path,
        &overwrites
            .highlights
            .clone()
            .unwrap_or_else(|| fs::read_to_string(&highlights_query_path).unwrap()),
    )
    .unwrap();
    let injections_query_path = grammar_dir.join("injections.scm");
    if !injections_query_path.exists() {
        println!("skipping {injections_query_path:?}");
    }
    let injections_query = InjectionsQuery::new(
        grammar,
        &injections_query_path,
        &overwrites
            .injections
            .clone()
            .unwrap_or_else(|| fs::read_to_string(&injections_query_path).unwrap_or_default()),
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
    overwrites: Box<[Overwrites]>,
    test_theme: RefCell<IndexSet<String>>,
}

impl TestLanguageLoader {
    fn new() -> Self {
        let skidder_config = skidder_config();
        // skidder::fetch(&skidder_config, false).unwrap();
        let grammars = skidder::list_grammars(&skidder_config).unwrap();
        let mut loader = TestLanguageLoader {
            lang_config: (0..grammars.len()).map(|_| OnceCell::new()).collect(),
            overwrites: vec![Overwrites::default(); grammars.len()].into_boxed_slice(),
            test_theme: RefCell::default(),
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
        };
        loader.languages.insert(
            "markdown.inline".into(),
            loader.languages["markdown-inline"],
        );
        loader
    }

    fn get(&self, name: &str) -> Language {
        self.languages[name]
    }

    fn overwrite_injections(&mut self, lang: &str, content: String) {
        let lang = self.get(lang);
        self.overwrites[lang.idx()].injections = Some(content);
        self.lang_config[lang.idx()] = OnceCell::new();
    }

    fn overwrite_highlights(&mut self, lang: &str, content: String) {
        let lang = self.get(lang);
        self.overwrites[lang.idx()].highlights = Some(content);
        self.lang_config[lang.idx()] = OnceCell::new();
    }
    fn shadow_injections(&mut self, lang: &str, content: &str) {
        let lang = self.get(lang);
        let skidder_config = skidder_config();
        let grammar = self.languages.get_index(lang.idx()).unwrap().0;
        let new_precedance = skidder::use_new_precedance(&skidder_config, grammar).unwrap();
        let grammar_dir = skidder_config.grammar_dir(grammar).unwrap();
        let mut injections =
            fs::read_to_string(grammar_dir.join("injections.scm")).unwrap_or_default();
        if new_precedance {
            injections.push('\n');
            injections.push_str(content)
        } else {
            let mut content = content.to_owned();
            content.push('\n');
            content.push_str(&injections);
            injections = content;
        }
        self.overwrites[lang.idx()].injections = Some(injections);
        self.lang_config[lang.idx()] = OnceCell::new();
    }

    fn shadow_highlights(&mut self, lang: &str, content: &str) {
        let lang = self.get(lang);
        let skidder_config = skidder_config();
        let grammar = self.languages.get_index(lang.idx()).unwrap().0;
        let new_precedance = skidder::use_new_precedance(&skidder_config, grammar).unwrap();
        let grammar_dir = skidder_config.grammar_dir(grammar).unwrap();
        let mut highlights = fs::read_to_string(grammar_dir.join("highlights.scm")).unwrap();
        if new_precedance {
            highlights.push('\n');
            highlights.push_str(content)
        } else {
            let mut content = content.to_owned();
            content.push('\n');
            content.push_str(&highlights);
            highlights = content;
        }
        self.overwrites[lang.idx()].highlights = Some(highlights);
        self.lang_config[lang.idx()] = OnceCell::new();
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
            let mut config = get_grammar(
                self.languages.get_index(lang.idx()).unwrap().0,
                &self.overwrites[lang.idx()],
            );
            let mut theme = self.test_theme.borrow_mut();
            config
                .highight_query
                .configure(|scope| Highlight(theme.insert_full(scope.to_owned()).0 as u32));
            config
        })
    }
}

fn fixture(loader: &TestLanguageLoader, fixture: impl AsRef<Path>) {
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
    let loader = TestLanguageLoader::new();
    fixture(&loader, "highlighter/hellow_world.rs");
}

#[test]
fn combined_injection() {
    let mut loader = TestLanguageLoader::new();
    loader.shadow_injections(
        "rust",
        r#"
((doc_comment) @injection.content
 (#set! injection.language "markdown")
 (#set! injection.combined))"#,
    );
    fixture(&loader, "highlighter/rust_doc_comment.rs");
}

#[test]
fn injection_in_child() {
    let mut loader = TestLanguageLoader::new();
    // here doc_comment is a child of line_comment which has higher precedance
    // however since it doesn't include children the doc_comment injection is
    // still active here. This could probalby use a more realworld usecase (maybe nix?)
    loader.shadow_injections(
        "rust",
        r#"
([(line_comment) (block_comment)] @injection.content
 (#set! injection.language "comment"))

([(line_comment (doc_comment) @injection.content) (block_comment (doc_comment) @injection.content)]
 (#set! injection.language "markdown")
 (#set! injection.combined))
"#,
    );
    fixture(&loader, "highlighter/rust_doc_comment.rs");
}

#[test]
fn injection_precedance() {
    let mut loader = TestLanguageLoader::new();
    loader.shadow_injections(
        "rust",
        r#"
([(line_comment (doc_comment) @injection.content) (block_comment (doc_comment) @injection.content)]
 (#set! injection.language "markdown")
 (#set! injection.combined))

([(line_comment) (block_comment)] @injection.content
 (#set! injection.language "comment")
 (#set! injection.include-children))"#,
    );
    fixture(&loader, "highlighter/rust_doc_comment.rs");
    loader.shadow_injections(
        "rust",
        r#"
([(line_comment) (block_comment)] @injection.content
 (#set! injection.language "comment")
 (#set! injection.include-children))

([(line_comment (doc_comment) @injection.content) (block_comment (doc_comment) @injection.content)]
 (#set! injection.language "markdown")
 (#set! injection.combined))"#,
    );
    fixture(&loader, "highlighter/rust_no_doc_comment.rs");
}
