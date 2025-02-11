use std::mem::replace;
use std::ops::RangeBounds;
use std::path::Path;
use std::slice;
use std::sync::Arc;

use crate::config::{LanguageConfig, LanguageLoader};
use crate::query_iter::{MatchedNode, QueryIter, QueryIterEvent, QueryLoader};
use crate::{Language, Layer, Syntax};
use arc_swap::ArcSwap;
use ropey::RopeSlice;
use tree_sitter::query::Query;
use tree_sitter::{query, Grammar};

/// Contains the data needed to highlight code written in a particular language.
///
/// This struct is immutable and can be shared between threads.
#[derive(Debug)]
pub struct HighlightQuery {
    pub query: Query,
    pub(crate) highlight_indices: ArcSwap<Vec<Highlight>>,
}

impl HighlightQuery {
    pub fn new(
        grammar: Grammar,
        query_text: &str,
        query_path: impl AsRef<Path>,
    ) -> Result<Self, query::ParseError> {
        let query = Query::new(grammar, query_text, query_path, |_pattern, predicate| {
            Err(format!("unsupported predicate {predicate}").into())
        })?;
        Ok(Self {
            highlight_indices: ArcSwap::from_pointee(vec![
                Highlight::NONE;
                query.num_captures() as usize
            ]),
            query,
        })
    }

    /// Configures the list of recognized highlight names.
    ///
    /// Tree-sitter syntax-highlighting queries specify highlights in the form of dot-separated
    /// highlight names like `punctuation.bracket` and `function.method.builtin`. Consumers of
    /// these queries can choose to recognize highlights with different levels of specificity.
    /// For example, the string `function.builtin` will match against `function.builtin.constructor`
    /// but will not match `function.method.builtin` and `function.method`.
    ///
    /// The closure provided to this function should therefore try to first lookup the full
    /// name. If no highlight was found for that name it should [`rsplit_once('.')`](str::rsplit_once)
    /// and retry until a highlight has been found. If none of the parent scopes are defined
    /// then `Highlight::NONE` should be returned.
    ///
    /// When highlighting, results are returned as `Highlight` values, configured by this function.
    /// The meaning of these indices is up to the user of the implementation. The highlighter
    /// treats the indices as entirely opaque.
    pub fn configure(&self, mut f: impl FnMut(&str) -> Highlight) {
        let highlight_indices = self
            .query
            .captures()
            .map(move |(_, capture_name)| f(capture_name))
            .collect();
        self.highlight_indices.store(Arc::new(highlight_indices));
    }
}

/// Indicates which highlight should be applied to a region of source code.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Highlight(pub u32);

impl Highlight {
    pub const NONE: Highlight = Highlight(u32::MAX);
}

#[derive(Debug)]
struct HighlightedNode {
    end: u32,
    highlight: Highlight,
}

#[derive(Debug, Default)]
pub struct LayerData {
    parent_highlights: usize,
    dormant_highlights: Vec<HighlightedNode>,
}

pub struct Highlighter<'a, 'tree, Loader: LanguageLoader> {
    query: QueryIter<'a, 'tree, HighlightQueryLoader<&'a Loader>, LayerData>,
    next_query_event: Option<QueryIterEvent<'tree, LayerData>>,
    active_highlights: Vec<HighlightedNode>,
    next_highlight_end: u32,
    next_highlight_start: u32,
    active_config: Option<&'a LanguageConfig>,
}

pub struct HighlightList<'a>(slice::Iter<'a, HighlightedNode>);

impl Iterator for HighlightList<'_> {
    type Item = Highlight;

    fn next(&mut self) -> Option<Highlight> {
        self.0.next().map(|node| node.highlight)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl DoubleEndedIterator for HighlightList<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(|node| node.highlight)
    }
}

pub enum HighlightEvent<'a> {
    RefreshHighlights(HighlightList<'a>),
    PushHighlights(HighlightList<'a>),
}

impl<'a, 'tree: 'a, Loader: LanguageLoader> Highlighter<'a, 'tree, Loader> {
    pub fn new(
        syntax: &'tree Syntax,
        src: RopeSlice<'a>,
        loader: &'a Loader,
        range: impl RangeBounds<u32>,
    ) -> Self {
        let mut query = QueryIter::new(syntax, src, HighlightQueryLoader(loader), range);
        let active_language = query.current_language();
        let mut res = Highlighter {
            active_config: query.loader().0.get_config(active_language),
            next_query_event: None,
            active_highlights: Vec::new(),
            next_highlight_end: u32::MAX,
            next_highlight_start: 0,
            query,
        };
        res.advance_query_iter();
        res
    }

    pub fn active_highlights(&self) -> HighlightList<'_> {
        HighlightList(self.active_highlights.iter())
    }

    pub fn next_event_offset(&self) -> u32 {
        self.next_highlight_start.min(self.next_highlight_end)
    }

    pub fn advance(&mut self) -> HighlightEvent<'_> {
        let mut refresh = false;
        let prev_stack_size = self.active_highlights.len();

        let pos = self.next_event_offset();
        if self.next_highlight_end == pos {
            // self.process_injection_ends();
            self.process_highlight_end(pos);
            refresh = true;
        }

        let mut first_highlight = true;
        while self.next_highlight_start == pos {
            let Some(query_event) = self.advance_query_iter() else {
                break;
            };
            match query_event {
                QueryIterEvent::EnterInjection(_) => self.enter_injection(),
                QueryIterEvent::Match(node) => self.start_highlight(node, &mut first_highlight),
                QueryIterEvent::ExitInjection { injection, state } => {
                    // state is returned if the layer is finished, if it isn't we have
                    // a combined injection and need to deactivate its highlights
                    if state.is_none() {
                        self.deactivate_layer(injection.layer);
                        refresh = true;
                    }
                    let active_language = self.query.current_language();
                    self.active_config = self.query.loader().0.get_config(active_language);
                }
            }
        }
        self.next_highlight_end = self
            .active_highlights
            .last()
            .map_or(u32::MAX, |node| node.end);

        if refresh {
            HighlightEvent::RefreshHighlights(HighlightList(self.active_highlights.iter()))
        } else {
            HighlightEvent::PushHighlights(HighlightList(
                self.active_highlights[prev_stack_size..].iter(),
            ))
        }
    }

    fn advance_query_iter(&mut self) -> Option<QueryIterEvent<'tree, LayerData>> {
        let event = replace(&mut self.next_query_event, self.query.next());
        self.next_highlight_start = self
            .next_query_event
            .as_ref()
            .map_or(u32::MAX, |event| event.start_byte());
        event
    }

    fn process_highlight_end(&mut self, pos: u32) {
        let i = self
            .active_highlights
            .iter()
            .rposition(|highlight| highlight.end != pos)
            .map_or(0, |i| i + 1);
        self.active_highlights.truncate(i);
    }

    fn enter_injection(&mut self) {
        let active_language = self.query.current_language();
        self.active_config = self.query.loader().0.get_config(active_language);
        let data = self.query.current_injection().1;
        data.parent_highlights = self.active_highlights.len();
        self.active_highlights.append(&mut data.dormant_highlights);
    }

    fn deactivate_layer(&mut self, layer: Layer) {
        let LayerData {
            mut parent_highlights,
            ref mut dormant_highlights,
            ..
        } = *self.query.layer_state(layer);
        parent_highlights = parent_highlights.min(self.active_highlights.len());
        dormant_highlights.extend(self.active_highlights.drain(parent_highlights..));
        self.process_highlight_end(self.next_highlight_start);
    }

    fn start_highlight(&mut self, node: MatchedNode, first_highlight: &mut bool) {
        let range = node.syntax_node.byte_range();
        if range.is_empty() {
            return;
        }

        // If multiple patterns match this exact node, prefer the last one which matched.
        // This matches the precedence of Neovim, Zed, and tree-sitter-cli.
        if !*first_highlight
            && self
                .active_highlights
                .last()
                .is_some_and(|prev_node| prev_node.end == range.end)
        {
            self.active_highlights.pop();
        }
        let highlight = self.active_config.map_or(Highlight::NONE, |config| {
            config.highlight_query.highlight_indices.load()[node.capture.idx()]
        });
        if highlight != Highlight::NONE {
            self.active_highlights.push(HighlightedNode {
                end: range.end,
                highlight,
            });
            *first_highlight = false;
        }
    }
}

pub(crate) struct HighlightQueryLoader<T>(T);

impl<'a, T: LanguageLoader> QueryLoader<'a> for HighlightQueryLoader<&'a T> {
    fn get_query(&mut self, lang: Language) -> Option<&'a Query> {
        self.0
            .get_config(lang)
            .map(|config| &config.highlight_query.query)
    }
}
