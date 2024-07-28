use ahash::RandomState;
use hashbrown::HashTable;
use ropey::RopeSlice;

use slotmap::{new_key_type, HopSlotMap};

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::str;
use std::sync::Arc;
use tree_sitter::{SyntaxTree, SyntaxTreeNode};

use crate::config::LanguageConfig;
use crate::injections_query::InjectionLanguageMarker;
use crate::parse::LayerUpdateFlags;

pub use crate::config::read_query;
pub use tree_sitter;
// pub use pretty_print::pretty_print_tree;
// pub use tree_cursor::TreeCursor;

mod config;
pub mod highlighter;
mod injections_query;
mod parse;
// mod pretty_print;
pub mod query_iter;
pub mod text_object;
// mod tree_cursor;

/// The Tree siitter syntax tree for a single language.
///
/// This is really multipe (nested) different syntax trees due to tree sitter
/// injections. A single syntax tree/parser is called layer. Each layer
/// is parsed as a single "file" by tree sitter. There can be multiple layers
/// for the same language. A layer corresponds to one of three things:
/// * the root layer
/// * a singular injection limited to a single node in it's parent layer
/// * Multiple injections (multiple disjoint nodes in parent layer) that are
/// parsed as tough they are a single uninterrupted file.
///
/// An injection always refer to a single node into which another layer is
/// injected. As injections only correspond to syntax tree nodes injections in
/// the same layer do not intersect. However, the syntax tree in a an injected
/// layer can have nodes that intersect with nodes from the parent layer. For
/// example:
/// ```
/// layer2: | Sibling A |      Sibling B (layer3)     | Sibling C |
/// layer1: | Sibling A (layer2) | Sibling B | Sibling C (layer2) |
/// ````
/// In this case Sibling B really spans across a "GAP" in layer2. While the syntax
/// node can not be split up by tree sitter directly, we can treat Sibling B as two
/// seperate injections. That is done while parsing/running the query capture. As
/// a result the injections from a tree. Note that such other queries must account for
/// such multi injection nodes.
#[derive(Debug)]
pub struct Syntax {
    layers: HopSlotMap<Layer, LayerData>,
    root: Layer,
}

impl Syntax {
    pub fn new(
        source: RopeSlice,
        config: Arc<LanguageConfig>,
        injection_callback: impl Fn(&InjectionLanguageMarker) -> Option<Arc<LanguageConfig>>,
    ) -> Option<Self> {
        let root_layer = LayerData {
            parse_tree: None,
            config,
            flags: LayerUpdateFlags::empty(),
            ranges: vec![tree_sitter::Range {
                start_byte: 0,
                end_byte: u32::MAX,
                start_point: tree_sitter::Point { row: 0, col: 0 },
                end_point: tree_sitter::Point {
                    row: u32::MAX,
                    col: u32::MAX,
                },
            }]
            .into_boxed_slice(),
            injections: Vec::new(),
            parent: None,
        };

        // track scope_descriptor: a Vec of scopes for item in tree

        let mut layers = HopSlotMap::with_capacity_and_key(32);
        let root = layers.insert(root_layer);

        let mut syntax = Self {
            root,
            layers,
            layer_lut: HashTable::with_capacity(32),
        };

        let res = syntax.update(source, &[], injection_callback);

        if res.is_err() {
            return None;
        }
        Some(syntax)
    }

    pub fn tree(&self) -> &SyntaxTree {
        self.layers[self.root].tree()
    }

    #[inline]
    pub fn tree_for_byte_range(&self, start: usize, end: usize) -> &SyntaxTree {
        let layer = self.layer_for_byte_range(start, end);
        self.layers[layer].tree()
    }

    #[inline]
    pub fn named_descendant_for_byte_range(
        &self,
        start: usize,
        end: usize,
    ) -> Option<SyntaxTreeNode<'_>> {
        self.tree_for_byte_range(start, end)
            .root_node()
            .named_descendant_for_byte_range(start as u32, end as u32)
    }

    #[inline]
    pub fn descendant_for_byte_range(
        &self,
        start: usize,
        end: usize,
    ) -> Option<SyntaxTreeNode<'_>> {
        self.tree_for_byte_range(start, end)
            .root_node()
            .descendant_for_byte_range(start as u32, end as u32)
    }

    pub fn layer_for_byte_range(&self, start: usize, end: usize) -> Layer {
        let mut cursor = self.root;
        loop {
            let layer = &self.layers[cursor];
            let Some(start_injection) = layer.injection_at_byte_idx(start as u32) else {
                break;
            };
            let Some(end_injection) = layer.injection_at_byte_idx(end as u32) else {
                break;
            };
            if start_injection.layer == end_injection.layer {
                cursor = start_injection.layer;
            } else {
                break;
            }
        }
        cursor
    }

    // pub fn walk(&self) -> TreeCursor<'_> {
    //     TreeCursor::new(&self.layers, self.root)
    // }
}

#[derive(Debug, Clone)]
pub struct Injection {
    pub byte_range: Range,
    pub layer: Layer,
}

#[derive(Debug)]
pub struct LayerData {
    pub config: Arc<LanguageConfig>,
    parse_tree: Option<SyntaxTree>,
    ranges: Box<[tree_sitter::Range]>,
    /// a list of **sorted** non-overlapping injection ranges. Note that
    /// injection ranges are not relative to the start of this layer but the
    /// start of the root layer
    injections: Vec<Injection>,
    /// internal flags used during parsing to track incremental invalidation
    flags: LayerUpdateFlags,
    parent: Option<Layer>,
}

/// This PartialEq implementation only checks if that
/// two layers are theoretically identical (meaning they highlight the same text range with the same language).
/// It does not check whether the layers have the same internal treesitter
/// state.
impl PartialEq for LayerData {
    fn eq(&self, other: &Self) -> bool {
        self.parent == other.parent
            && self.config.grammar == other.config.grammar
            && self.ranges == other.ranges
    }
}

/// Hash implementation belongs to PartialEq implementation above.
/// See its documentation for details.
impl Hash for LayerData {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.parent.hash(state);
        self.config.grammar.hash(state);
        self.ranges.hash(state);
    }
}

impl LayerData {
    pub fn tree(&self) -> &SyntaxTree {
        // TODO: no unwrap
        self.parse_tree.as_ref().unwrap()
    }

    /// Returns the injection range **within this layers** that contains `idx`.
    /// This function will not descend into nested injections
    pub fn injection_at_byte_idx(&self, idx: u32) -> Option<&Injection> {
        let i = self
            .injections
            .partition_point(|range| range.byte_range.start < idx);
        self.injections
            .get(i)
            .filter(|injection| injection.byte_range.end > idx)
    }
}

/// Represents the reason why syntax highlighting failed.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Timeout,
    ExceededMaximumSize,
    InvalidLanguage,
    InvalidRanges,
    Unknown,
}

fn byte_range_to_str(range: Range, source: RopeSlice) -> Cow<str> {
    Cow::from(source.byte_slice(range.start as usize..range.end as usize))
}

new_key_type! {
    /// A layer represent a single a single syntax tree that reprsents (part of)
    /// a file parsed with a tree-sitter grammar. See [`Syntax`]
    pub struct Layer;
}

/// The maximum number of in-progress matches a TS cursor can consider at once.
/// This is set to a constant in order to avoid performance problems for medium to large files. Set with `set_match_limit`.
/// Using such a limit means that we lose valid captures, so there is fundamentally a tradeoff here.
///
///
/// Old tree sitter versions used a limit of 32 by default until this limit was removed in version `0.19.5` (must now be set manually).
/// However, this causes performance issues for medium to large files.
/// In helix, this problem caused treesitter motions to take multiple seconds to complete in medium-sized rust files (3k loc).
///
///
/// Neovim also encountered this problem and reintroduced this limit after it was removed upstream
/// (see <https://github.com/neovim/neovim/issues/14897> and <https://github.com/neovim/neovim/pull/14915>).
/// The number used here is fundamentally a tradeoff between breaking some obscure edge cases and performance.
///
///
/// Neovim chose 64 for this value somewhat arbitrarily (<https://github.com/neovim/neovim/pull/18397>).
/// 64 is too low for some languages though. In particular, it breaks some highlighting for record fields in Erlang record definitions.
/// This number can be increased if new syntax highlight breakages are found, as long as the performance penalty is not too high.
pub const TREE_SITTER_MATCH_LIMIT: u32 = 256;

// we don't care about dosh resistance and using a const hasher is easier here
static HASHER: RandomState = RandomState::with_seeds(
    0x8a2e_03a0_08f3_08fe,
    0x7644_ec4e_263f_6a88,
    0x38f2_299f_85a3_f3e9,
    0xfa98_6c81_a409_31d0,
);

// use 32 bit ranges since TS doesn't support files larger than 2GiB anyway
// and it allows us to save a lot memory/improve cache efficency
type Range = std::ops::Range<u32>;
