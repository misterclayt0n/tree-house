use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use indexmap::{IndexMap, IndexSet};
use once_cell::sync::Lazy;
use once_cell::unsync::OnceCell;
use skidder::Repo;
use tree_sitter::Grammar;

use crate::config::{LanguageConfig, LanguageLoader};
use crate::fixtures::{check_highlighter_fixture, check_injection_fixture};
use crate::highlighter::{Highlight, HighlightQuery};
use crate::injections_query::{InjectionLanguageMarker, InjectionsQuery};
use crate::Language;

static GRAMMARS: Lazy<Vec<PathBuf>> = Lazy::new(|| {
    fs::create_dir_all("../test-grammars").unwrap();
    let skidder_config = skidder_config();
    skidder::fetch(&skidder_config, false).unwrap();
    skidder::build_all_grammars(&skidder_config, false, None).unwrap();
    let grammars = skidder::list_grammars(&skidder_config).unwrap();
    assert!(!grammars.is_empty());
    grammars
});

fn skidder_config() -> skidder::Config {
    skidder::Config {
        repos: vec![Repo::Git {
            name: "helix-language-support".to_owned(),
            remote: "git@github.com:helix-editor/tree-sitter-grammars.git".into(),
            branch: "reversed".into(),
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
    let parser_path = skidder::build_grammar(&skidder_config, grammar, false).unwrap();
    let grammar = unsafe { Grammar::new(grammar, &parser_path).unwrap() };
    let highlights_query_path = grammar_dir.join("highlights.scm");
    let highlight_query = HighlightQuery::new(
        grammar,
        &highlights_query_path,
        &overwrites.highlights.clone().unwrap_or_else(|| {
            fs::read_to_string(&highlights_query_path)
                .map_err(|err| {
                    format!(
                        "failed to read highlights in {}: {err}",
                        highlights_query_path.display()
                    )
                })
                .unwrap()
        }),
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
        highlight_query,
        injections_query,
    }
}

#[derive(Debug)]
struct TestLanguageLoader {
    languages: IndexMap<String, Language>,
    lang_config: Box<[OnceCell<LanguageConfig>]>,
    overwrites: Box<[Overwrites]>,
    test_theme: RefCell<IndexSet<String>>,
}

impl TestLanguageLoader {
    fn new() -> Self {
        let grammars = &GRAMMARS;

        Self {
            lang_config: (0..grammars.len()).map(|_| OnceCell::new()).collect(),
            overwrites: vec![Overwrites::default(); grammars.len()].into_boxed_slice(),
            test_theme: RefCell::default(),
            languages: grammars
                .iter()
                .enumerate()
                .map(|(i, grammar)| {
                    (
                        grammar.file_name().unwrap().to_str().unwrap().to_owned(),
                        Language::new(i as u32),
                    )
                })
                .collect(),
        }
    }

    fn get(&self, name: &str) -> Language {
        self.languages[name]
    }

    // TODO: remove on first use.
    #[allow(dead_code)]
    fn overwrite_injections(&mut self, lang: &str, content: String) {
        let lang = self.get(lang);
        self.overwrites[lang.idx()].injections = Some(content);
        self.lang_config[lang.idx()] = OnceCell::new();
    }

    // TODO: remove on first use.
    #[allow(dead_code)]
    fn overwrite_highlights(&mut self, lang: &str, content: String) {
        let lang = self.get(lang);
        self.overwrites[lang.idx()].highlights = Some(content);
        self.lang_config[lang.idx()] = OnceCell::new();
    }

    fn shadow_injections(&mut self, lang: &str, content: &str) {
        let lang = self.get(lang);
        let skidder_config = skidder_config();
        let grammar = self.languages.get_index(lang.idx()).unwrap().0;
        let grammar_dir = skidder_config.grammar_dir(grammar).unwrap();
        let mut injections =
            fs::read_to_string(grammar_dir.join("injections.scm")).unwrap_or_default();
        injections.push('\n');
        injections.push_str(content);
        self.overwrites[lang.idx()].injections = Some(injections);
        self.lang_config[lang.idx()] = OnceCell::new();
    }

    // TODO: remove on first use.
    #[allow(dead_code)]
    fn shadow_highlights(&mut self, lang: &str, content: &str) {
        let lang = self.get(lang);
        let skidder_config = skidder_config();
        let grammar = self.languages.get_index(lang.idx()).unwrap().0;
        let grammar_dir = skidder_config.grammar_dir(grammar).unwrap();
        let mut highlights = fs::read_to_string(grammar_dir.join("highlights.scm")).unwrap();
        highlights.push('\n');
        highlights.push_str(content);
        self.overwrites[lang.idx()].highlights = Some(highlights);
        self.lang_config[lang.idx()] = OnceCell::new();
    }
}

impl LanguageLoader for TestLanguageLoader {
    fn language_for_marker(&self, marker: &InjectionLanguageMarker) -> Option<Language> {
        let InjectionLanguageMarker::Name(name) = marker else {
            return None;
        };
        self.languages.get(*name).copied()
    }

    fn get_config(&self, lang: Language) -> &LanguageConfig {
        self.lang_config[lang.idx()].get_or_init(|| {
            let config = get_grammar(
                self.languages.get_index(lang.idx()).unwrap().0,
                &self.overwrites[lang.idx()],
            );
            let mut theme = self.test_theme.borrow_mut();
            config
                .highlight_query
                .configure(|scope| Highlight(theme.insert_full(scope.to_owned()).0 as u32));
            config
        })
    }
}

fn highlight_fixture(loader: &TestLanguageLoader, fixture: impl AsRef<Path>) {
    let path = Path::new("../fixtures").join(fixture);
    let lang = match path
        .extension()
        .and_then(|it| it.to_str())
        .unwrap_or_default()
    {
        "rs" => loader.get("rust"),
        extension => unreachable!("unknown file type .{extension}"),
    };
    check_highlighter_fixture(
        path,
        "// ",
        lang,
        loader,
        |highlight| loader.test_theme.borrow()[highlight.0 as usize].clone(),
        |_| ..,
    )
}

fn injection_fixture(loader: &TestLanguageLoader, fixture: impl AsRef<Path>) {
    let path = Path::new("../fixtures").join(fixture);
    let lang = match path
        .extension()
        .and_then(|it| it.to_str())
        .unwrap_or_default()
    {
        "rs" => loader.get("rust"),
        extension => unreachable!("unknown file type .{extension}"),
    };
    check_injection_fixture(
        path,
        "// ",
        lang,
        loader,
        |lang| loader.languages.get_index(lang.idx()).unwrap().0.clone(),
        |_| ..,
    )
}

#[test]
fn highlight() {
    let loader = TestLanguageLoader::new();
    highlight_fixture(&loader, "highlighter/hello_world.rs");
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
    highlight_fixture(&loader, "highlighter/rust_doc_comment.rs");
}

#[test]
fn injection_in_child() {
    let mut loader = TestLanguageLoader::new();
    // here doc_comment is a child of line_comment which has higher precedence
    // however since it doesn't include children the doc_comment injection is
    // still active here. This could probably use a more real world use case (maybe nix?)
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
    highlight_fixture(&loader, "highlighter/rust_doc_comment.rs");
    injection_fixture(&loader, "injections/rust_doc_comment.rs");
}

#[test]
fn injection_precedence() {
    let mut loader = TestLanguageLoader::new();
    loader.shadow_injections(
        "rust",
        r#"
([(line_comment) (block_comment)] @injection.content
 (#set! injection.language "comment")
 (#set! injection.include-children))

([(line_comment (doc_comment) @injection.content) (block_comment (doc_comment) @injection.content)]
 (#set! injection.language "markdown")
 (#set! injection.combined))
 "#,
    );
    highlight_fixture(&loader, "highlighter/rust_doc_comment.rs");
    loader.shadow_injections(
        "rust",
        r#"
([(line_comment (doc_comment) @injection.content) (block_comment (doc_comment) @injection.content)]
 (#set! injection.language "markdown")
 (#set! injection.combined))

([(line_comment) (block_comment)] @injection.content
 (#set! injection.language "comment")
 (#set! injection.include-children))
 "#,
    );
    highlight_fixture(&loader, "highlighter/rust_no_doc_comment.rs");
    injection_fixture(&loader, "injections/rust_no_doc_comment.rs");
}
