use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::time::Duration;

use expect_test::expect;
use indexmap::{IndexMap, IndexSet};
use once_cell::unsync::OnceCell;
use ropey::{Rope, RopeSlice};
use skidder::Repo;
use tree_sitter::Grammar;

use crate::config::{LanguageConfig, LanguageLoader};
use crate::highlighter::{HighlighEvent, Highlight, HighlightQuery, Highligther};
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

fn collect_highlights(
    loader: &TestLanguageLoader,
    syntax: &Syntax,
    src: RopeSlice<'_>,
    start: usize,
) -> Vec<(String, Vec<String>)> {
    let mut highlighter = Highligther::new(syntax, src, &loader, start as u32..);
    let mut res = Vec::new();
    let mut pos = highlighter.next_event_offset();
    let mut highlight_stack = Vec::new();
    while pos < src.len_bytes() as u32 {
        let new_highlights = match highlighter.advance() {
            HighlighEvent::RefreshHiglights(highlights) => {
                highlight_stack.clear();
                highlights
            }
            HighlighEvent::PushHighlights(highlights) => highlights,
        };
        highlight_stack.extend(
            new_highlights
                .map(|highlight| loader.test_theme.borrow()[highlight.0 as usize].clone()),
        );
        let start = pos;
        pos = highlighter.next_event_offset();
        if pos == u32::MAX {
            pos = src.len_bytes() as u32
        }
        if highlight_stack.is_empty() {
            continue;
        }
        let src = src.byte_slice(start as usize..pos as usize).to_string();
        println!("{src:?} = {highlight_stack:?}");
        res.push((src, highlight_stack.clone()))
    }
    res
}

macro_rules! fixture {
    (
        $loader: expr,
        $syntax: expr,
        $src: expr,
        $start: expr,
        $fixture: expr
    ) => {{
        use std::fmt::Write;
        let scopes = collect_highlights($loader, $syntax, $src, $start);
        let mut res = String::new();
        for scope in scopes {
            writeln!(res, "{:?} {:?}", scope.0, scope.1).unwrap();
        }
        $fixture.assert_eq(&res);
    }};
}

#[test]
fn highlight() {
    let mut loader = TestLanguageLoader::new();
    let lang = loader.get("rust");
    let source = Rope::from_str(
        r#"
        fn main() {
            println!("hello world")
        }
    "#,
    );
    let syntax = Syntax::new(source.slice(..), lang, Duration::from_secs(1), &loader).unwrap();
    fixture!(
        &loader,
        &syntax,
        source.slice(..),
        0,
        expect![[r#"
        "fn" ["keyword.function"]
        "main" ["function"]
        "(" ["punctuation.bracket"]
        ")" ["punctuation.bracket"]
        "{" ["punctuation.bracket"]
        "println" ["function.macro"]
        "!" ["function.macro"]
        "(" ["punctuation.bracket"]
        "\"hello world\"" ["string"]
        ")" ["punctuation.bracket"]
        "}" ["punctuation.bracket"]
    "#]]
    );
}

#[test]
fn combined_injection() {
    let mut loader = TestLanguageLoader::new();
    let lang = loader.get("rust");
    let source = Rope::from_str(
        r#"
        /// **hello-world** 
        /// **foo
        fn foo() {
            println!("hello world")
        }
        /// bar**
        fn bar() {
            println!("hello world")
        }
    "#,
    );
    let syntax = Syntax::new(source.slice(..), lang, Duration::from_secs(1), &loader).unwrap();
    fixture!(&loader, &syntax, source.slice(..), 0, expect![[r#"
        "///" ["comment"]
        " " ["comment"]
        "*" ["comment", "markup.bold", "punctuation.bracket"]
        "*" ["comment", "markup.bold", "punctuation.bracket"]
        "hello-world" ["comment", "markup.bold"]
        "*" ["comment", "markup.bold", "punctuation.bracket"]
        "*" ["comment", "markup.bold", "punctuation.bracket"]
        " \n" ["comment"]
        "///" ["comment"]
        " " ["comment"]
        "*" ["comment", "markup.bold", "punctuation.bracket"]
        "*" ["comment", "markup.bold", "punctuation.bracket"]
        "foo\n" ["comment", "markup.bold"]
        "" ["comment"]
        "fn" ["keyword.function"]
        "foo" ["function"]
        "(" ["punctuation.bracket"]
        ")" ["punctuation.bracket"]
        "{" ["punctuation.bracket"]
        "println" ["function.macro"]
        "!" ["function.macro"]
        "(" ["punctuation.bracket"]
        "\"hello world\"" ["string"]
        ")" ["punctuation.bracket"]
        "}" ["punctuation.bracket"]
        "///" ["comment"]
        " bar" ["comment", "markup.bold"]
        "*" ["comment", "markup.bold", "punctuation.bracket"]
        "*" ["comment", "markup.bold", "punctuation.bracket"]
        "\n" ["comment"]
        "fn" ["keyword.function"]
        "bar" ["function"]
        "(" ["punctuation.bracket"]
        ")" ["punctuation.bracket"]
        "{" ["punctuation.bracket"]
        "println" ["function.macro"]
        "!" ["function.macro"]
        "(" ["punctuation.bracket"]
        "\"hello world\"" ["string"]
        ")" ["punctuation.bracket"]
        "}" ["punctuation.bracket"]
    "#]]);
}
