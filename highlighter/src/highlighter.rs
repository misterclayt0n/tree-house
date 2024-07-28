use std::mem::replace;
use std::path::Path;
use std::slice;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::query_iter::{MatchedNode, QueryIter, QueryIterEvent};
use crate::Layer;
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
        query_path: impl AsRef<Path>,
        query_text: &str,
    ) -> Result<Self, query::ParseError> {
        let query = Query::new(grammar, query_text, query_path, |_pattern, predicate| {
            Err(format!("unsupported predicate {predicate}").into())
        })?;
        let highlight_indices =
            ArcSwap::from_pointee(vec![Highlight::NONE; query.num_captures() as usize]);
        Ok(Self {
            query,
            highlight_indices,
        })
    }

    /// Set the list of recognized highlight names.
    ///
    /// Tree-sitter syntax-highlighting queries specify highlights in the form of dot-separated
    /// highlight names like `punctuation.bracket` and `function.method.builtin`. Consumers of
    /// these queries can choose to recognize highlights with different levels of specificity.
    /// For example, the string `function.builtin` will match against `function.builtin.constructor`
    /// but will not match `function.method.builtin` and `function.method`.
    ///
    /// When highlighting, results are returned as `Highlight` values, which contain the index
    /// of the matched highlight this list of highlight names.
    pub fn configure(&self, recognized_names: &[String]) {
        let mut capture_parts = Vec::new();
        let indices: Vec<_> = self
            .query
            .captures()
            .map(move |(_, capture_name)| {
                capture_parts.clear();
                capture_parts.extend(capture_name.split('.'));

                let mut best_index = u32::MAX;
                let mut best_match_len = 0;
                for (i, recognized_name) in recognized_names.iter().enumerate() {
                    let mut len = 0;
                    let mut matches = true;
                    for (i, part) in recognized_name.split('.').enumerate() {
                        match capture_parts.get(i) {
                            Some(capture_part) if *capture_part == part => len += 1,
                            _ => {
                                matches = false;
                                break;
                            }
                        }
                    }
                    if matches && len > best_match_len {
                        best_index = i as u32;
                        best_match_len = len;
                    }
                }
                Highlight(best_index)
            })
            .collect();

        self.highlight_indices.store(Arc::new(indices));
    }
}

/// Indicates which highlight should be applied to a region of source code.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Highlight(pub u32);

impl Highlight {
    pub(crate) const NONE: Highlight = Highlight(u32::MAX);
}

#[derive(Debug)]
struct HighlightedNode {
    end: u32,
    highlight: Highlight,
}

#[derive(Debug, Default)]
struct LayerData {
    parent_highlights: usize,
    dormant_highlights: Vec<HighlightedNode>,
}

struct HighlighterConfig<'a> {
    new_precedance: bool,
    highlight_indices: &'a [Highlight],
}

pub struct Highligther<'a> {
    query: QueryIter<'a, LayerData>,
    next_query_event: Option<QueryIterEvent<LayerData>>,
    active_highlights: Vec<HighlightedNode>,
    next_highlight_end: u32,
    next_highlight_start: u32,
    config: HighlighterConfig<'a>,
}

pub struct HighlightList<'a>(slice::Iter<'a, HighlightedNode>);

impl<'a> Iterator for HighlightList<'a> {
    type Item = Highlight;

    fn next(&mut self) -> Option<Highlight> {
        self.0.next().map(|node| node.highlight)
    }
}

pub enum HighlighEvent<'a> {
    RefreshHiglights(HighlightList<'a>),
    PushHighlights(HighlightList<'a>),
}

impl<'a> Highligther<'a> {
    pub fn active_highlights(&self) -> HighlightList<'_> {
        HighlightList(self.active_highlights.iter())
    }

    pub fn next_event_offset(&self) -> u32 {
        self.next_highlight_start.min(self.next_highlight_end)
    }

    pub fn advance(&mut self) -> HighlighEvent<'_> {
        let mut refresh = false;
        let prev_stack_size = self.active_highlights.len();

        let pos = self.next_event_offset();
        if self.next_highlight_end == pos {
            self.process_injection_ends();
            self.process_higlight_end();
            refresh = true;
        }

        let mut first_highlight = true;
        while self.next_highlight_start == pos {
            let Some(query_event) = self.adance_query_iter() else {
                break;
            };
            match query_event {
                QueryIterEvent::EnterInjection(_) => self.enter_injection(),
                QueryIterEvent::Match(node) => self.start_highlight(node, &mut first_highlight),
                QueryIterEvent::ExitInjection { injection, state } => {
                    // state is returned if the layer is finifhed, if it isn't we have
                    // a combined injection and need to deactive its highlights
                    if state.is_none() {
                        self.deactive_layer(injection.layer);
                        refresh = true;
                    }
                }
            }
        }
        self.next_highlight_end = self
            .active_highlights
            .last()
            .map_or(u32::MAX, |node| node.end);

        if refresh {
            HighlighEvent::RefreshHiglights(HighlightList(self.active_highlights.iter()))
        } else {
            HighlighEvent::PushHighlights(HighlightList(
                self.active_highlights[prev_stack_size..].iter(),
            ))
        }
    }

    fn adance_query_iter(&mut self) -> Option<QueryIterEvent<LayerData>> {
        let event = replace(&mut self.next_query_event, self.query.next());
        self.next_highlight_start = self
            .next_query_event
            .as_ref()
            .map_or(u32::MAX, |event| event.start_byte());
        event
    }

    fn process_higlight_end(&mut self) {
        let i = self
            .active_highlights
            .iter()
            .rposition(|highlight| highlight.end != self.next_highlight_end)
            .unwrap();
        self.active_highlights.truncate(i);
    }

    /// processes injections that end at the same position as highlights first.
    fn process_injection_ends(&mut self) {
        while self.next_highlight_end == self.next_highlight_start {
            match self.next_query_event.as_ref() {
                Some(QueryIterEvent::ExitInjection { injection, state }) => {
                    if state.is_none() {
                        self.deactive_layer(injection.layer);
                    }
                }
                Some(QueryIterEvent::Match(matched_node)) if matched_node.byte_range.is_empty() => {
                }
                _ => break,
            }
        }
    }

    fn enter_injection(&mut self) {
        self.query.current_layer_state().parent_highlights = self.active_highlights.len();
    }

    fn deactive_layer(&mut self, layer: Layer) {
        let LayerData {
            parent_highlights,
            ref mut dormant_highlights,
            ..
        } = *self.query.layer_state(layer);
        let i = self.active_highlights[parent_highlights..]
            .iter()
            .rposition(|highlight| highlight.end != self.next_highlight_end)
            .unwrap();
        self.active_highlights.truncate(parent_highlights + i);
        dormant_highlights.extend(self.active_highlights.drain(parent_highlights..))
    }

    fn start_highlight(&mut self, node: MatchedNode, first_highlight: &mut bool) {
        if node.byte_range.is_empty() {
            return;
        }

        // if there are multiple matches for the exact same node
        // only use one of the (the last with new/nvim precedance)
        if !*first_highlight
            && self
                .active_highlights
                .last()
                .map_or(false, |prev_node| prev_node.end == node.byte_range.end)
        {
            if self.config.new_precedance {
                self.active_highlights.pop();
            } else {
                return;
            }
        }
        let highlight = self.config.highlight_indices[node.capture.idx()];
        if highlight.0 == u32::MAX {
            return;
        }
        self.active_highlights.push(HighlightedNode {
            end: node.byte_range.end,
            highlight,
        });
        *first_highlight = false;
    }
}
