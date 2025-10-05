use locals::Locals;
use ropey::RopeSlice;

use slab::Slab;

use std::fmt;
use std::hash::{Hash, Hasher};
use std::time::Duration;
use tree_sitter::{IncompatibleGrammarError, Node, Tree};

pub use crate::config::{read_query, LanguageConfig, LanguageLoader};
pub use crate::injections_query::{InjectionLanguageMarker, InjectionsQuery};
use crate::parse::LayerUpdateFlags;
pub use crate::tree_cursor::TreeCursor;
pub use tree_sitter;
// pub use pretty_print::pretty_print_tree;
// pub use tree_cursor::TreeCursor;

mod config;
pub mod highlighter;
mod injections_query;
mod parse;
#[cfg(all(test, feature = "fixtures"))]
mod tests;
// mod pretty_print;
#[cfg(feature = "fixtures")]
pub mod fixtures;
pub mod locals;
pub mod query_iter;
pub mod text_object;
mod tree_cursor;

/// A layer represents a single a single syntax tree that represents (part of)
/// a file parsed with a tree-sitter grammar. See [`Syntax`].
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct Layer(u32);

impl Layer {
    fn idx(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Language(pub u32);

impl Language {
    pub fn new(idx: u32) -> Language {
        Language(idx)
    }

    pub fn idx(self) -> usize {
        self.0 as usize
    }
}

/// The Tree sitter syntax tree for a single language.
///
/// This is really multiple (nested) different syntax trees due to tree sitter
/// injections. A single syntax tree/parser is called layer. Each layer
/// is parsed as a single "file" by tree sitter. There can be multiple layers
/// for the same language. A layer corresponds to one of three things:
/// * the root layer
/// * a singular injection limited to a single node in its parent layer
/// * Multiple injections (multiple disjoint nodes in parent layer) that are
///   parsed as though they are a single uninterrupted file.
///
/// An injection always refer to a single node into which another layer is
/// injected. As injections only correspond to syntax tree nodes injections in
/// the same layer do not intersect. However, the syntax tree in a an injected
/// layer can have nodes that intersect with nodes from the parent layer. For
/// example:
///
/// ``` no-compile
/// layer2: | Sibling A |      Sibling B (layer3)     | Sibling C |
/// layer1: | Sibling A (layer2) | Sibling B | Sibling C (layer2) |
/// ````
///
/// In this case Sibling B really spans across a "GAP" in layer2. While the syntax
/// node can not be split up by tree sitter directly, we can treat Sibling B as two
/// separate injections. That is done while parsing/running the query capture. As
/// a result the injections form a tree. Note that such other queries must account for
/// such multi injection nodes.
#[derive(Debug)]
pub struct Syntax {
    layers: Slab<LayerData>,
    root: Layer,
}

impl Syntax {
    pub fn new(
        source: RopeSlice,
        language: Language,
        timeout: Duration,
        loader: &impl LanguageLoader,
    ) -> Result<Self, Error> {
        let root_layer = LayerData {
            parse_tree: None,
            language,
            flags: LayerUpdateFlags::default(),
            ranges: vec![tree_sitter::Range {
                start_byte: 0,
                end_byte: u32::MAX,
                start_point: tree_sitter::Point::ZERO,
                end_point: tree_sitter::Point::MAX,
            }],
            injections: Vec::new(),
            parent: None,
            locals: Locals::default(),
        };
        let mut layers = Slab::with_capacity(32);
        let root = layers.insert(root_layer);
        let mut syntax = Self {
            root: Layer(root as u32),
            layers,
        };

        syntax.update(source, timeout, &[], loader).map(|_| syntax)
    }

    /// Create a new Syntax with an old tree for incremental parsing
    pub fn new_with_old_tree(
        source: RopeSlice,
        language: Language,
        timeout: Duration,
        loader: &impl LanguageLoader,
        old_tree: Option<&Tree>,
        edits: &[tree_sitter::InputEdit],
    ) -> Result<Self, Error> {
        let root_layer = LayerData {
            parse_tree: old_tree.cloned(),
            language,
            flags: LayerUpdateFlags::default(),
            ranges: vec![tree_sitter::Range {
                start_byte: 0,
                end_byte: u32::MAX,
                start_point: tree_sitter::Point::ZERO,
                end_point: tree_sitter::Point::MAX,
            }],
            injections: Vec::new(),
            parent: None,
            locals: Locals::default(),
        };
        let mut layers = Slab::with_capacity(32);
        let root = layers.insert(root_layer);
        let mut syntax = Self {
            root: Layer(root as u32),
            layers,
        };

        syntax
            .update(source, timeout, edits, loader)
            .map(|_| syntax)
    }

    pub fn layer(&self, layer: Layer) -> &LayerData {
        &self.layers[layer.idx()]
    }

    fn layer_mut(&mut self, layer: Layer) -> &mut LayerData {
        &mut self.layers[layer.idx()]
    }

    pub fn root(&self) -> Layer {
        self.root
    }

    pub fn tree(&self) -> &Tree {
        self.layer(self.root)
            .tree()
            .expect("`Syntax::new` would err if the root layer's tree could not be parsed")
    }

    #[inline]
    pub fn tree_for_byte_range(&self, start: u32, end: u32) -> &Tree {
        self.layer_and_tree_for_byte_range(start, end).1
    }

    /// Finds the smallest layer which has a parse tree and covers the given range.
    pub(crate) fn layer_and_tree_for_byte_range(&self, start: u32, end: u32) -> (Layer, &Tree) {
        let mut layer = self.layer_for_byte_range(start, end);
        loop {
            // NOTE: this loop is guaranteed to terminate because the root layer always has a
            // tree.
            if let Some(tree) = self.layer(layer).tree() {
                return (layer, tree);
            }
            if let Some(parent) = self.layer(layer).parent {
                layer = parent;
            }
        }
    }

    #[inline]
    pub fn named_descendant_for_byte_range(&self, start: u32, end: u32) -> Option<Node<'_>> {
        self.tree_for_byte_range(start, end)
            .root_node()
            .named_descendant_for_byte_range(start, end)
    }

    #[inline]
    pub fn descendant_for_byte_range(&self, start: u32, end: u32) -> Option<Node<'_>> {
        self.tree_for_byte_range(start, end)
            .root_node()
            .descendant_for_byte_range(start, end)
    }

    /// Finds the smallest injection layer that fully includes the range `start..=end`.
    pub fn layer_for_byte_range(&self, start: u32, end: u32) -> Layer {
        self.layers_for_byte_range(start, end)
            .last()
            .expect("always includes the root layer")
    }

    /// Returns an iterator of layers which **fully include** the byte range `start..=end`,
    /// in decreasing order based on the size of each layer.
    ///
    /// The first layer is always the `root` layer.
    pub fn layers_for_byte_range(&self, start: u32, end: u32) -> impl Iterator<Item = Layer> + '_ {
        let mut parent_injection_layer = self.root;

        std::iter::once(self.root).chain(std::iter::from_fn(move || {
            let layer = &self.layers[parent_injection_layer.idx()];

            let injection_at_start = layer.injection_at_byte_idx(start)?;

            // +1 because the end is exclusive.
            let injection_at_end = layer.injection_at_byte_idx(end + 1)?;

            (injection_at_start.layer == injection_at_end.layer).then(|| {
                parent_injection_layer = injection_at_start.layer;

                injection_at_start.layer
            })
        }))
    }

    pub fn walk(&self) -> TreeCursor<'_> {
        TreeCursor::new(self)
    }
}

#[derive(Debug, Clone)]
pub struct Injection {
    pub range: Range,
    pub layer: Layer,
    matched_node_range: Range,
}

#[derive(Debug)]
pub struct LayerData {
    pub language: Language,
    parse_tree: Option<Tree>,
    ranges: Vec<tree_sitter::Range>,
    /// a list of **sorted** non-overlapping injection ranges. Note that
    /// injection ranges are not relative to the start of this layer but the
    /// start of the root layer
    injections: Vec<Injection>,
    /// internal flags used during parsing to track incremental invalidation
    flags: LayerUpdateFlags,
    parent: Option<Layer>,
    locals: Locals,
}

/// This PartialEq implementation only checks if that
/// two layers are theoretically identical (meaning they highlight the same text range with the same language).
/// It does not check whether the layers have the same internal tree-sitter
/// state.
impl PartialEq for LayerData {
    fn eq(&self, other: &Self) -> bool {
        self.parent == other.parent
            && self.language == other.language
            && self.ranges == other.ranges
    }
}

/// Hash implementation belongs to PartialEq implementation above.
/// See its documentation for details.
impl Hash for LayerData {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.parent.hash(state);
        self.language.hash(state);
        self.ranges.hash(state);
    }
}

impl LayerData {
    /// Returns the parsed `Tree` for this layer.
    ///
    /// This `Option` will always be `Some` when the `LanguageLoader` passed to `Syntax::new`
    /// returns `Some` when passed the layer's language in `LanguageLoader::get_config`.
    pub fn tree(&self) -> Option<&Tree> {
        self.parse_tree.as_ref()
    }

    /// Returns the injection range **within this layers** that contains `idx`.
    /// This function will not descend into nested injections
    pub fn injection_at_byte_idx(&self, idx: u32) -> Option<&Injection> {
        self.injections_at_byte_idx(idx)
            .next()
            .filter(|injection| injection.range.start <= idx)
    }

    /// Returns the injection ranges **within this layers** that contain
    /// `idx` or start after idx. This function will not descend into nested
    /// injections.
    pub fn injections_at_byte_idx(&self, idx: u32) -> impl Iterator<Item = &Injection> {
        let i = self
            .injections
            .partition_point(|range| range.range.end < idx);
        self.injections[i..].iter()
    }
}

/// Represents the reason why syntax highlighting failed.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Timeout,
    ExceededMaximumSize,
    InvalidRanges,
    Unknown,
    NoRootConfig,
    IncompatibleGrammar(Language, IncompatibleGrammarError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("configured timeout was exceeded"),
            Self::ExceededMaximumSize => f.write_str("input text exceeds the maximum allowed size"),
            Self::InvalidRanges => f.write_str("invalid ranges"),
            Self::Unknown => f.write_str("an unknown error occurred"),
            Self::NoRootConfig => f.write_str(
                "`LanguageLoader::get_config` for the root layer language returned `None`",
            ),
            Self::IncompatibleGrammar(language, IncompatibleGrammarError { abi_version }) => {
                write!(
                    f,
                    "failed to load grammar for language {language:?} with ABI version {abi_version}"
                )
            }
        }
    }
}

/// The maximum number of in-progress matches a TS cursor can consider at once.
/// This is set to a constant in order to avoid performance problems for medium to large files. Set with `set_match_limit`.
/// Using such a limit means that we lose valid captures, so there is fundamentally a tradeoff here.
///
///
/// Old tree sitter versions used a limit of 32 by default until this limit was removed in version `0.19.5` (must now be set manually).
/// However, this causes performance issues for medium to large files.
/// In Helix, this problem caused tree-sitter motions to take multiple seconds to complete in medium-sized rust files (3k loc).
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

// use 32 bit ranges since TS doesn't support files larger than 2GiB anyway
// and it allows us to save a lot memory/improve cache efficiency
type Range = std::ops::Range<u32>;
