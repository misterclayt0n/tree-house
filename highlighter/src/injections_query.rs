use std::borrow::Cow;
use std::path::Path;

use hashbrown::HashMap;
use once_cell::sync::Lazy;
use regex_cursor::engines::meta::Regex;
use ropey::RopeSlice;

use crate::{byte_range_to_str, Layer, LayerData};
use tree_sitter::query::UserPredicate;
use tree_sitter::{
    query, Capture, Grammar, InactiveQueryCursor, Pattern, Query, QueryMatch, SyntaxTreeNode,
};

const SHEBANG: &str = r"#!\s*(?:\S*[/\\](?:env\s+(?:\-\S+\s+)*)?)?([^\s\.\d]+)";
static SHEBANG_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(SHEBANG).unwrap());

#[derive(Clone, Default, Debug)]
pub struct InjectionProperties<'a> {
    include_children: IncludedChildren,
    language: Option<InjectionLanguageMarker<'a>>,
    combined: bool,
}

#[derive(Debug, Clone)]
pub enum InjectionLanguageMarker<'a> {
    Name(Cow<'a, str>),
    Filename(Cow<'a, Path>),
    Shebang(String),
}

#[derive(Clone, Default, Debug)]
enum IncludedChildren {
    #[default]
    None,
    All,
    Unnamed,
}

#[derive(Debug)]
pub struct InjectionsQuery {
    query: Query,
    injection_properties: HashMap<Pattern, InjectionProperties<'static>>,
    injection_content_capture: Option<Capture>,
    injection_language_capture: Option<Capture>,
    injection_filename_capture: Option<Capture>,
    injection_shebang_capture: Option<Capture>,
}

impl InjectionsQuery {
    pub fn new(
        grammar: Grammar,
        query_text: &str,
        query_path: impl AsRef<Path>,
    ) -> Result<Self, query::ParseError> {
        let mut injection_properties: HashMap<Pattern, InjectionProperties> = HashMap::new();
        let query = Query::new(grammar, query_text, query_path, |pattern, predicate| {
            match predicate {
                // "injection.include-unnamed-children"
                UserPredicate::SetProperty {
                    key: "injection.include-unnamed-children",
                    val: None,
                } => {
                    injection_properties
                        .entry(pattern)
                        .or_default()
                        .include_children = IncludedChildren::Unnamed
                }
                UserPredicate::SetProperty {
                    key: "injection.include-children",
                    val: None,
                } => {
                    injection_properties
                        .entry(pattern)
                        .or_default()
                        .include_children = IncludedChildren::All
                }
                UserPredicate::SetProperty {
                    key: "injection.language",
                    val: Some(lang),
                } => {
                    injection_properties.entry(pattern).or_default().language =
                        Some(InjectionLanguageMarker::Name(lang.to_owned().into()))
                }
                UserPredicate::SetProperty {
                    key: "injection.combined",
                    val: None,
                } => injection_properties.entry(pattern).or_default().combined = true,
                predicate => {
                    return Err(format!("unsupported predicate {predicate}").into());
                }
            }
            Ok(())
        })?;

        Ok(InjectionsQuery {
            injection_properties,
            injection_content_capture: query.get_capture("injection.content"),
            injection_language_capture: query.get_capture("injection.language"),
            injection_filename_capture: query.get_capture("injection.filename"),
            injection_shebang_capture: query.get_capture("injection.shebang"),
            query,
        })
    }

    pub fn injection_for_match<'a>(
        &self,
        query_match: &QueryMatch<'a, 'a>,
        source: RopeSlice<'a>,
    ) -> Option<(InjectionProperties<'a>, SyntaxTreeNode<'a>)> {
        let properties = self
            .injection_properties
            .get(&query_match.pattern())
            .cloned()
            .unwrap_or_default();

        let mut injection_capture = None;
        let mut content_node = None;
        for matched_node in query_match.matched_nodes() {
            let capture = Some(matched_node.capture);
            if capture == self.injection_language_capture {
                let name = byte_range_to_str(matched_node.syntax_node.byte_range(), source);
                injection_capture = Some(InjectionLanguageMarker::Name(name));
            } else if capture == self.injection_filename_capture {
                let name = byte_range_to_str(matched_node.syntax_node.byte_range(), source);
                let path = Path::new(name.as_ref()).to_path_buf();
                injection_capture = Some(InjectionLanguageMarker::Filename(path.into()));
            } else if capture == self.injection_shebang_capture {
                let range = matched_node.syntax_node.byte_range();
                let node_slice = source.byte_slice(range.start as usize..range.end as usize);

                // some languages allow space and newlines before the actual string content
                // so a shebang could be on either the first or second line
                let lines = if let Ok(end) = node_slice.try_line_to_byte(2) {
                    node_slice.byte_slice(..end)
                } else {
                    node_slice
                };

                injection_capture = SHEBANG_REGEX
                    .captures_iter(regex_cursor::Input::new(lines))
                    .map(|cap| {
                        let cap = lines.byte_slice(cap.get_group(1).unwrap().range());
                        InjectionLanguageMarker::Shebang(cap.into())
                    })
                    .next()
            } else if capture == self.injection_content_capture {
                content_node = Some(matched_node.syntax_node.clone());
            }
        }

        Some((
            InjectionProperties {
                language: injection_capture.or(properties.language),
                ..properties
            },
            content_node?,
        ))
    }
}

struct CombinedInjection<'a> {
    reuse_layer: Option<Layer>,
    ranges: Vec<tree_sitter::Range>,
    include_children: IncludedChildren,
    language: Option<InjectionLanguageMarker<'a>>,
    capture: Option<SyntaxTreeNode<'a>>,
}

impl LayerData {
    pub fn find_injections(
        &mut self,
        edits: &[tree_sitter::InputEdit],
        cursor: InactiveQueryCursor,
        source: RopeSlice<'_>,
    ) -> InactiveQueryCursor {
        let injection_query = &self.config.injections_query;
        // Process injections.
        let query_cursor =
            cursor.execute_query(&injection_query.query, &self.tree().root_node(), source);

        let mut ranges = self.ranges.iter().peekable();
        let mut old_injections = self.injections.iter().peekable();

        let mut last_injection_end = 0;
        let mut injections = Vec::with_capacity(self.ranges.len());
        let mut combined_injections: HashMap<Pattern, CombinedInjection> =
            HashMap::with_capacity(32);
        while let Some(query_match) = query_cursor.next_match() {
            let (properties, content_node) =
                injection_query.injection_for_match(&query_match, source);

            // in case this is a combined injection save it for more processing later
            if properties.combined {
                match combined_injections.entry(query_match.pattern()) {}
                let entry = &mut combined_injections[combined_injection_idx];
                if injection_capture.is_some() {
                    entry.0 = injection_capture;
                }
                if let Some(content_node) = content_node {
                    if content_node.start_byte() >= last_injection_end {
                        entry.1.push(content_node);
                        last_injection_end = content_node.end_byte();
                    }
                }
                entry.2 = included_children;
                continue;
            }

            // Explicitly remove this match so that none of its other captures will remain
            // in the stream of captures.
            mat.remove();

            // If a language is found with the given name, then add a new language layer
            // to the highlighted document.
            if let (Some(injection_capture), Some(content_node)) = (injection_capture, content_node)
            {
                if let Some(config) = (injection_callback)(&injection_capture) {
                    let ranges =
                        intersect_ranges(&layer.ranges, &[content_node], included_children);

                    if !ranges.is_empty() {
                        if content_node.start_byte() < last_injection_end {
                            continue;
                        }
                        last_injection_end = content_node.end_byte();
                        injections.push((config, ranges));
                    }
                }
            }
        }

        for (lang_name, content_nodes, included_children) in combined_injections {
            if let (Some(lang_name), false) = (lang_name, content_nodes.is_empty()) {
                if let Some(config) = (injection_callback)(&lang_name) {
                    let ranges = intersect_ranges(&layer.ranges, &content_nodes, included_children);
                    if !ranges.is_empty() {
                        injections.push((config, ranges));
                    }
                }
            }
        }
    }
}
