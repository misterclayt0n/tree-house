use core::slice;
use std::borrow::Cow;
use std::iter::Peekable;
use std::mem::take;
use std::path::Path;
use std::sync::Arc;

use hashbrown::HashMap;
use once_cell::sync::Lazy;
use regex_cursor::engines::meta::Regex;
use ropey::RopeSlice;

use crate::config::LanguageConfig;
use crate::{byte_range_to_str, injections_query, Injection, Layer, LayerData, Range, Syntax};
use tree_sitter::query::UserPredicate;
use tree_sitter::{
    query, Capture, Grammar, InactiveQueryCursor, MatchedNodeIdx, Pattern, Query, QueryMatch,
    SyntaxTreeNode,
};

const SHEBANG: &str = r"#!\s*(?:\S*[/\\](?:env\s+(?:\-\S+\s+)*)?)?([^\s\.\d]+)";
static SHEBANG_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(SHEBANG).unwrap());

#[derive(Clone, Default, Debug)]
pub struct InjectionProperties {
    include_children: IncludedChildren,
    language: Option<Box<str>>,
    combined: bool,
}

#[derive(Debug, Clone)]
pub enum InjectionLanguageMarker<'a> {
    Name(Cow<'a, str>),
    Filename(Cow<'a, Path>),
    Shebang(String),
}

#[derive(Clone, Debug)]
pub struct InjectionMatchProperties {
    include_children: IncludedChildren,
    language: Arc<LanguageConfig>,
    scope: Option<InjectionScope>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum InjectionScope {
    Match { id: u32 },
    Pattern { pattern: Pattern, grammar: Grammar },
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
    injection_properties: HashMap<Pattern, InjectionProperties>,
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
                } => injection_properties.entry(pattern).or_default().language = Some(lang.into()),
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

    pub fn properties_for_match<'a>(
        &self,
        query_match: &QueryMatch<'a, 'a>,
        source: RopeSlice<'a>,
        injection_callback: impl Fn(&InjectionLanguageMarker) -> Option<Arc<LanguageConfig>>,
    ) -> Option<(InjectionMatchProperties, MatchedNodeIdx)> {
        let properties = self
            .injection_properties
            .get(&query_match.pattern())
            .cloned()
            .unwrap_or_default();

        let mut injection_capture = None;
        let mut last_content_node = 0;
        let mut content_nodes = 0;
        for (i, matched_node) in query_match.matched_nodes().enumerate() {
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
                content_nodes += 1;

                last_content_node = i as u32;
            }
        }
        let language = injection_capture.or(properties
            .language
            .as_deref()
            .map(|name| InjectionLanguageMarker::Name(name.into())))?;
        let language = injection_callback(&language)?;
        let scope = if properties.combined {
            Some(InjectionScope::Pattern {
                pattern: query_match.pattern(),
                grammar: language.grammar,
            })
        } else if content_nodes != 1 {
            Some(InjectionScope::Match {
                id: query_match.id(),
            })
        } else {
            None
        };

        Some((
            InjectionMatchProperties {
                language,
                scope,
                include_children: properties.include_children,
            },
            last_content_node,
        ))
    }
}

struct InjectionBuilder {
    layer: Option<Layer>,
    ranges: Vec<tree_sitter::Range>,
    language: Arc<LanguageConfig>,
    include_children: IncludedChildren,
}

impl Syntax {
    pub(crate) fn run_injection_query(
        &mut self,
        layer: Layer,
        mut offset: Option<i32>,
        edits: &[tree_sitter::InputEdit],
        cursor: InactiveQueryCursor,
        source: RopeSlice<'_>,
        injection_callback: impl Fn(&InjectionLanguageMarker) -> Option<Arc<LanguageConfig>>,
    ) -> InactiveQueryCursor {
        self.map_injections(layer, offset, edits);
        let layer = &mut self.layers[layer];
        let injections_query = &layer.config.injections_query;
        let Some(injection_content_capture) = injections_query.injection_content_capture else {
            return cursor;
        };
        let query_cursor =
            cursor.execute_query(&injections_query.query, &self.tree().root_node(), source);

        let mut old_injections = layer.injections.iter().peekable();

        let mut last_injection_end = 0;
        let mut injections = Vec::with_capacity(layer.injections.len());
        let mut combined_injections: HashMap<InjectionScope, InjectionBuilder> =
            HashMap::with_capacity(32);
        while let Some((query_match, node_idx)) = query_cursor.next_matched_node() {
            let node = query_match.matched_node(node_idx);
            if query_match.matched_node(node_idx).capture != injection_content_capture {
                continue;
            }
            let Some((properties, last_conten_node_idx)) =
                injections_query.properties_for_match(&query_match, source, &injection_callback)
            else {
                query_match.remove();
                continue;
            };
            if last_conten_node_idx == node_idx {
                query_match.remove();
            }
            let range = query_match.matched_node(node_idx).syntax_node.byte_range();

            let grammar = properties.language.grammar;
            let injection_builder = properties
                .scope
                .and_then(|scope| combined_injections.remove(&scope))
                .unwrap_or_else(|| InjectionBuilder {
                    layer: None,
                    ranges: Vec::new(),
                    language: properties.language,
                    include_children: properties.include_children,
                });
            if injection_builder.layer.is_none() {
                injection_builder.layer = self.reuse_injection(grammar, range, &mut old_injections)
            }
            // TODO: intersect ranges

            if let Some(scope) = properties.scope {
                if !matches!(scope, InjectionScope::Pattern { .. })
                    || last_conten_node_idx != node_idx
                {
                    combined_injections.insert(scope, injection_builder);
                }
            }
            // TODO: prioritization
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

    /// maps the layers injection ranges trough edits to enable incremental reparse
    fn map_injections(
        &mut self,
        layer: Layer,
        offset: Option<i32>,
        mut edits: &[tree_sitter::InputEdit],
    ) {
        let layer_data = &mut self.layers[layer];
        let mut injections = take(&mut layer_data.injections);
        if injections.is_empty() {
            return;
        }
        let mut offset = if let Some(offset) = offset {
            let first_relevant_edit = edits.partition_point(|edit| {
                (edit.old_end_byte as i32) < (layer_data.ranges[0].end_byte as i32 - offset)
            });
            edits = &edits[first_relevant_edit..];
            offset
        } else {
            0
        };
        // injections and edits are non-overlapping and sorted so we can
        // apply edits in O(M+N) instead of O(NM)
        let mut edits = edits.iter().peekable();
        for injection in &mut injections {
            let range = &mut injection.range;
            let flags = &mut self.layers[injection.layer].flags;

            while let Some(edit) = edits.next_if(|edit| edit.old_end_byte < range.start) {
                offset += edit.offset();
            }
            flags.moved = offset != 0;
            let mut mapped_start = (range.start as i32 + offset) as u32;
            if let Some(edit) = edits.next_if(|edit| edit.old_end_byte <= range.end) {
                if edit.start_byte < range.start {
                    flags.moved = true;
                    mapped_start = (edit.new_end_byte as i32 + offset) as u32;
                } else {
                    flags.modified = true;
                }
                offset += edit.offset();
                while let Some(edit) = edits.next_if(|edit| edit.old_end_byte <= range.end) {
                    offset += edit.offset();
                }
            }
            let mut mapped_end = (range.end as i32 + offset) as u32;
            if let Some(edit) = edits.peek().filter(|edit| edit.start_byte <= range.end) {
                flags.modified = true;

                if edit.start_byte < range.start {
                    mapped_start = (edit.new_end_byte as i32 + offset) as u32;
                    mapped_end = mapped_start;
                }
            }
            *range = mapped_start..mapped_end;
        }
        self.layers[layer].injections = injections;
    }

    fn reuse_injection<'a>(
        &mut self,
        grammar: Grammar,
        new_range: Range,
        injections: &mut Peekable<impl Iterator<Item = &'a Injection>>,
    ) -> Option<Layer> {
        loop {
            let skip = injections.next_if(|injection| injection.range.end <= new_range.start);
            if skip.is_none() {
                break;
            }
        }
        let injection = injections.next_if(|injection| {
            (injection.range.start < new_range.end)
                && self.layers[injection.layer].config.grammar == grammar
                && !self.layers[injection.layer].flags.reused
        })?;
        self.layers[injection.layer].flags.reused = true;
        Some(injection.layer)
    }
}
