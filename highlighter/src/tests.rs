use std::borrow::Cow;
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
use crate::highlighter::Highlight;
use crate::injections_query::InjectionLanguageMarker;
use crate::Language;

static GRAMMARS: Lazy<Vec<PathBuf>> = Lazy::new(|| {
    let skidder_config = skidder_config();
    skidder::fetch(&skidder_config, false).unwrap();
    skidder::build_all_grammars(&skidder_config, false, None).unwrap();
    let grammars = skidder::list_grammars(&skidder_config).unwrap();
    assert!(!grammars.is_empty());
    grammars
});

fn skidder_config() -> skidder::Config {
    skidder::Config {
        repos: vec![Repo::Local {
            // `./test-grammars` in the root of the repo.
            path: Path::new("../test-grammars").canonicalize().unwrap(),
        }],
        index: PathBuf::new(),
        verbose: true,
    }
}

#[derive(Debug, Clone, Default)]
struct Overwrites {
    highlights: Option<String>,
    locals: Option<String>,
    injections: Option<String>,
}

fn get_grammar(grammar: &str, overwrites: &Overwrites) -> LanguageConfig {
    let skidder_config = skidder_config();
    let grammar_dir = skidder_config.grammar_dir(grammar).unwrap();
    let parser_path = skidder::build_grammar(&skidder_config, grammar, false).unwrap();
    let grammar = unsafe { Grammar::new(grammar, &parser_path).unwrap() };
    let highlights_query_path = grammar_dir.join("highlights.scm");
    let injections_query_path = grammar_dir.join("injections.scm");
    if !injections_query_path.exists() {
        println!("skipping {injections_query_path:?}");
    }
    let locals_query_path = grammar_dir.join("locals.scm");
    if !locals_query_path.exists() {
        println!("skipping {locals_query_path:?}");
    }
    LanguageConfig::new(
        grammar,
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
        &overwrites
            .injections
            .clone()
            .unwrap_or_else(|| fs::read_to_string(&injections_query_path).unwrap_or_default()),
        &overwrites
            .locals
            .clone()
            .unwrap_or_else(|| fs::read_to_string(&locals_query_path).unwrap_or_default()),
    )
    .unwrap()
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
    fn language_for_marker(&self, marker: InjectionLanguageMarker) -> Option<Language> {
        match marker {
            InjectionLanguageMarker::Name(name) => self.languages.get(name).copied(),
            InjectionLanguageMarker::Match(text) => {
                let name: Cow<str> = text.into();
                self.languages.get(name.as_ref()).copied()
            }
            _ => unimplemented!(),
        }
    }

    fn get_config(&self, lang: Language) -> Option<&LanguageConfig> {
        let config = self.lang_config[lang.idx()].get_or_init(|| {
            let config = get_grammar(
                self.languages.get_index(lang.idx()).unwrap().0,
                &self.overwrites[lang.idx()],
            );
            let mut theme = self.test_theme.borrow_mut();
            config.configure(|scope| {
                Some(Highlight::new(theme.insert_full(scope.to_owned()).0 as u32))
            });
            config
        });
        Some(config)
    }
}

fn lang_for_path(path: &Path, loader: &TestLanguageLoader) -> Language {
    match path
        .extension()
        .and_then(|it| it.to_str())
        .unwrap_or_default()
    {
        "rs" => loader.get("rust"),
        "html" => loader.get("html"),
        "erl" => loader.get("erlang"),
        extension => unreachable!("unknown file type .{extension}"),
    }
}

fn highlight_fixture(loader: &TestLanguageLoader, fixture: impl AsRef<Path>) {
    let path = Path::new("../fixtures").join(fixture);
    let lang = lang_for_path(&path, loader);
    check_highlighter_fixture(
        path,
        "// ",
        lang,
        loader,
        |highlight| loader.test_theme.borrow()[highlight.idx()].clone(),
        |_| ..,
    )
}

fn injection_fixture(loader: &TestLanguageLoader, fixture: impl AsRef<Path>) {
    let path = Path::new("../fixtures").join(fixture);
    let lang = lang_for_path(&path, loader);
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
fn highlight_overlaps_with_injection() {
    let loader = TestLanguageLoader::new();
    // The comment node is highlighted both by the comment capture and as an injection for the
    // comment grammar.
    highlight_fixture(&loader, "highlighter/comment.html");
}

#[test]
fn rust_parameter_locals() {
    let loader = TestLanguageLoader::new();
    highlight_fixture(&loader, "highlighter/rust_parameter_locals.rs");
}

#[test]
fn parameters_within_injections_within_injections() {
    let loader = TestLanguageLoader::new();
    // The root language is Rust. Then markdown is injected in a doc comment. Then within that
    // we have a code fence which is Rust again. Within that block we check that locals are
    // highlighted as expected.
    highlight_fixture(&loader, "highlighter/injectionception.rs");
    injection_fixture(&loader, "injections/injectionception.rs");
}

#[test]
fn html_in_edoc_in_erlang() {
    let loader = TestLanguageLoader::new();
    // This fixture exhibited a bug (which has been fixed) where a combined injection became
    // dormant at the same time as a new highlight started, causing a total reset of all
    // highlights (incorrectly).
    highlight_fixture(&loader, "highlighter/html_in_edoc_in_erlang.erl");
    injection_fixture(&loader, "injections/html_in_edoc_in_erlang.erl");
}

#[test]
fn non_local_pattern() {
    let mut loader = TestLanguageLoader::new();
    // Pretend that `this` is a builtin like `self`, but only when it is not a parameter.
    loader.shadow_highlights(
        "rust",
        r#"
((identifier) @variable.builtin
 (#eq? @variable.builtin "this")
 (#is-not? local))
"#,
    );
    highlight_fixture(&loader, "highlighter/non_local.rs");
}

#[test]
fn reference_highlight_starts_after_definition_ends() {
    let loader = TestLanguageLoader::new();
    // In this example the function name matches one of the parameters. The function name can be
    // a reference but since the definition occurs after the function name it, the function name
    // should not be highlighted as a parameter.
    highlight_fixture(
        &loader,
        "highlighter/reference_highlight_starts_after_definition_ends.rs",
    );
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
