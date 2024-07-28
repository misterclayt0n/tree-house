use once_cell::sync::Lazy;
use regex::Regex;
use tree_sitter::Grammar;

use crate::highlighter::HighlightQuery;
use crate::injections_query::InjectionsQuery;

use std::fmt::Write;

#[derive(Debug)]
pub struct LanguageConfig {
    pub grammar: Grammar,
    pub highight_query: HighlightQuery,
    pub injections_query: InjectionsQuery,
}

static INHERITS_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r";+\s*inherits\s*:?\s*([a-z_,()-]+)\s*").unwrap());

/// reads a query by invoking `read_query_text`, handeles any `inherits` directives
pub fn read_query(
    language: &str,
    filename: &str,
    mut read_query_text: impl FnMut(&str, &str) -> String,
) -> String {
    fn read_query_impl(
        language: &str,
        filename: &str,
        read_query_text: &mut impl FnMut(&str, &str) -> String,
    ) -> String {
        let query = read_query_text(language, filename);

        // replaces all "; inherits <language>(,<language>)*" with the queries of the given language(s)
        INHERITS_REGEX
            .replace_all(&query, |captures: &regex::Captures| {
                captures[1]
                    .split(',')
                    .fold(String::new(), |mut output, language| {
                        // `write!` to a String cannot fail.
                        write!(
                            output,
                            "\n{}\n",
                            read_query(language, filename, &mut *read_query_text)
                        )
                        .unwrap();
                        output
                    })
            })
            .into_owned()
    }
    read_query_impl(language, filename, &mut read_query_text)
}
