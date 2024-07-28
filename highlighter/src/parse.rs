use std::collections::VecDeque;
use std::mem::replace;
use std::sync::Arc;
use std::time::Duration;

use bitflags::bitflags;
use ropey::RopeSlice;
use tree_sitter::{InactiveQueryCursor, Parser, Point, Range};

use crate::config::LanguageConfig;
use crate::{
    Error, InjectionLanguageMarker, Layer, LayerData, Syntax, HASHER, TREE_SITTER_MATCH_LIMIT,
};

bitflags! {
    /// Flags that track the status of a layer
    /// in the `Sytaxn::update` function
    #[derive(Debug)]
    pub(crate) struct LayerUpdateFlags : u32{
        const MODIFIED = 0b001;
        const MOVED = 0b010;
        const TOUCHED = 0b100;
    }
}

fn point_add(a: Point, b: Point) -> Point {
    if b.row > 0 {
        Point {
            row: a.row + b.row,
            col: b.col,
        }
    } else {
        Point {
            row: a.row,
            col: a.col + b.col,
        }
    }
}
fn point_sub(a: Point, b: Point) -> Point {
    if a.row > b.row {
        Point {
            row: a.row - b.row,
            col: b.col,
        }
    } else {
        Point {
            row: a.row,
            col: a.col - b.col,
        }
    }
}

impl Syntax {
    pub fn update(
        &mut self,
        source: RopeSlice,
        edits: &[tree_sitter::InputEdit],
        injection_callback: impl Fn(&InjectionLanguageMarker) -> Option<Arc<LanguageConfig>>,
    ) -> Result<(), Error> {
        // size limit of 512MiB, TS just cannot handle files this big (too
        // slow). Furthermore, TS uses 32 (signed) bit indecies so this limit
        // must never be raised above 2GiB
        if source.len_bytes() >= 512 * 1024 * 1024 {
            return Err(Error::ExceededMaximumSize);
        }

        let mut queue = VecDeque::new();
        queue.push_back(self.root);

        // Use the edits to update all layers markers
        if !edits.is_empty() {
            for (layer_id, layer) in self.layers.iter_mut() {
                // The root layer always covers the whole range (0..usize::MAX)
                if layer.parent.is_none() {
                    layer.flags = LayerUpdateFlags::MODIFIED;
                } else {
                    layer.apply_edit(edits)
                }
            }
        }

        let parser = Parser::new();
        parser.set_timeout(Duration::from_millis(500)); // half a second is pretty generours
        let mut cursor = InactiveQueryCursor::new();
        // TODO: might need to set cursor range
        cursor.set_byte_range(0..usize::MAX);
        cursor.set_match_limit(TREE_SITTER_MATCH_LIMIT);

        let source_slice = source.slice(..);

        while let Some(layer_id) = queue.pop_front() {
            let layer = &mut self.layers[layer_id];

            // Mark the layer as touched
            layer.flags |= LayerUpdateFlags::TOUCHED;
            // If a tree already exists, notify it of changes.
            if let Some(tree) = &mut layer.parse_tree {
                if layer
                    .flags
                    .intersects(LayerUpdateFlags::MODIFIED | LayerUpdateFlags::MOVED)
                {
                    for edit in edits.iter().rev() {
                        // Apply the edits in reverse.
                        // If we applied them in order then edit 1 would disrupt the positioning of edit 2.
                        tree.edit(edit);
                    }
                }

                if layer.flags.contains(LayerUpdateFlags::MODIFIED) {
                    // Re-parse the tree.
                    layer.parse(&mut parser, source)?;
                }
            } else {
                // always parse if this layer has never been parsed before
                layer.parse(&mut parser, source)?;
            }

            // Switch to an immutable borrow.
            let layer = &self.layers[layer_id];

            // TODO: can't inline this since matches borrows self.layers
            for (config, ranges) in injections {
                let parent = Some(layer_id);
                let new_layer = LayerData {
                    parse_tree: None,
                    config,
                    ranges: ranges.into_boxed_slice(),
                    flags: LayerUpdateFlags::empty(),
                    parent,
                    injections: Vec::new(),
                };

                // Find an identical existing layer
                let layer = layers_table
                    .get(layers_hasher.hash_one(&new_layer), |&it| {
                        self.layers[it] == new_layer
                    })
                    .copied();

                // ...or insert a new one.
                let layer_id = layer.unwrap_or_else(|| self.layers.insert(new_layer));

                queue.push_back(layer_id);
            }

            // TODO: pre-process local scopes at this time, rather than highlight?
            // would solve problems with locals not working across boundaries
        }

        self.prune_dead_layers();
        Ok(())
    }

    /// Reset all `LayerUpdateFlags` and remove all untouched layers
    fn prune_dead_layers(&mut self) {
        // TODO: is it faster to iterate the hashmap or the slotmap? slotmap
        // has higher utilzation (except for edgecases) so one may assume it
        // iterates faster. The layer size is much larger than a layer idx so
        // the density argument may not be as relevant (altough CPUs are pretty
        // good at strided memory access). The other container always does
        // random access/earsure, and the random access into the slot map should
        // be faster than into the hashmap. That's why I went with this order for
        // now
        self.layers.retain(|_, layer| {
            let retain = layer.flags.contains(LayerUpdateFlags::TOUCHED);
            layer.flags = LayerUpdateFlags::empty();
            retain
        });
    }
}

/// Compute the ranges that should be included when parsing an injection.
/// This takes into account three things:
/// * `parent_ranges` - The ranges must all fall within the *current* layer's ranges.
/// * `nodes` - Every injection takes place within a set of nodes. The injection ranges
///   are the ranges of those nodes.
/// * `includes_children` - For some injections, the content nodes' children should be
///   excluded from the nested document, so that only the content nodes' *own* content
///   is reparsed. For other injections, the content nodes' entire ranges should be
///   reparsed, including the ranges of their children.
fn intersect_ranges(
    parent_ranges: &[Range],
    nodes: &[Node],
    included_children: IncludedChildren,
) -> Vec<Range> {
    let mut cursor = nodes[0].walk();
    let mut result = Vec::new();
    let mut parent_range_iter = parent_ranges.iter();
    let mut parent_range = parent_range_iter
        .next()
        .expect("Layers should only be constructed with non-empty ranges vectors");
    for node in nodes.iter() {
        let mut preceding_range = Range {
            start_byte: 0,
            start_point: Point::new(0, 0),
            end_byte: node.start_byte(),
            end_point: node.start_position(),
        };
        let following_range = Range {
            start_byte: node.end_byte(),
            start_point: node.end_position(),
            end_byte: usize::MAX,
            end_point: Point::new(usize::MAX, usize::MAX),
        };

        for excluded_range in node
            .children(&mut cursor)
            .filter_map(|child| match included_children {
                IncludedChildren::None => Some(child.range()),
                IncludedChildren::All => None,
                IncludedChildren::Unnamed => {
                    if child.is_named() {
                        Some(child.range())
                    } else {
                        None
                    }
                }
            })
            .chain([following_range].iter().cloned())
        {
            let mut range = Range {
                start_byte: preceding_range.end_byte,
                start_point: preceding_range.end_point,
                end_byte: excluded_range.start_byte,
                end_point: excluded_range.start_point,
            };
            preceding_range = excluded_range;

            if range.end_byte < parent_range.start_byte {
                continue;
            }

            while parent_range.start_byte <= range.end_byte {
                if parent_range.end_byte > range.start_byte {
                    if range.start_byte < parent_range.start_byte {
                        range.start_byte = parent_range.start_byte;
                        range.start_point = parent_range.start_point;
                    }

                    if parent_range.end_byte < range.end_byte {
                        if range.start_byte < parent_range.end_byte {
                            result.push(Range {
                                start_byte: range.start_byte,
                                start_point: range.start_point,
                                end_byte: parent_range.end_byte,
                                end_point: parent_range.end_point,
                            });
                        }
                        range.start_byte = parent_range.end_byte;
                        range.start_point = parent_range.end_point;
                    } else {
                        if range.start_byte < range.end_byte {
                            result.push(range);
                        }
                        break;
                    }
                }

                if let Some(next_range) = parent_range_iter.next() {
                    parent_range = next_range;
                } else {
                    return result;
                }
            }
        }
    }
    result
}

impl LayerData {
    fn parse(&mut self, parser: &mut Parser, source: RopeSlice) -> Result<(), Error> {
        parser
            .set_included_ranges(&self.ranges)
            .map_err(|_| Error::InvalidRanges)?;

        parser
            .set_language(&self.config.language)
            .map_err(|_| Error::InvalidLanguage)?;

        // unsafe { syntax.parser.set_cancellation_flag(cancellation_flag) };
        let tree = parser
            .parse_with(
                &mut |byte, _| {
                    if byte <= source.len_bytes() {
                        let (chunk, start_byte, _, _) = source.chunk_at_byte(byte);
                        &chunk.as_bytes()[byte - start_byte..]
                    } else {
                        // out of range
                        &[]
                    }
                },
                self.parse_tree.as_ref(),
            )
            .ok_or(Error::Timeout)?;
        // unsafe { ts_parser.parser.set_cancellation_flag(None) };
        self.parse_tree = Some(tree);
        Ok(())
    }

    fn apply_edit(&mut self, edits: &[tree_sitter::InputEdit]) {
        for range in &mut *self.ranges {
            // Roughly based on https://github.com/tree-sitter/tree-sitter/blob/ddeaa0c7f534268b35b4f6cb39b52df082754413/lib/src/subtree.c#L691-L720
            for edit in edits.iter().rev() {
                let is_pure_insertion = edit.old_end_byte == edit.start_byte;

                // if edit is after range, skip
                if edit.start_byte > range.end_byte {
                    // TODO: || (is_noop && edit.start_byte == range.end_byte)
                    continue;
                }

                // if edit is before range, shift entire range by len
                if edit.old_end_byte < range.start_byte {
                    range.start_byte = edit.new_end_byte + (range.start_byte - edit.old_end_byte);
                    range.start_point = point_add(
                        edit.new_end_point,
                        point_sub(range.start_point, edit.old_end_point),
                    );

                    range.end_byte = edit
                        .new_end_byte
                        .saturating_add(range.end_byte - edit.old_end_byte);
                    range.end_point = point_add(
                        edit.new_end_point,
                        point_sub(range.end_point, edit.old_end_point),
                    );

                    self.flags |= LayerUpdateFlags::MOVED;
                }
                // if the edit starts in the space before and extends into the range
                else if edit.start_byte < range.start_byte {
                    range.start_byte = edit.new_end_byte;
                    range.start_point = edit.new_end_point;

                    range.end_byte = range
                        .end_byte
                        .saturating_sub(edit.old_end_byte)
                        .saturating_add(edit.new_end_byte);
                    range.end_point = point_add(
                        edit.new_end_point,
                        point_sub(range.end_point, edit.old_end_point),
                    );
                    self.flags = LayerUpdateFlags::MODIFIED;
                }
                // If the edit is an insertion at the start of the tree, shift
                else if edit.start_byte == range.start_byte && is_pure_insertion {
                    range.start_byte = edit.new_end_byte;
                    range.start_point = edit.new_end_point;
                    self.flags |= LayerUpdateFlags::MOVED;
                } else {
                    range.end_byte = range
                        .end_byte
                        .saturating_sub(edit.old_end_byte)
                        .saturating_add(edit.new_end_byte);
                    range.end_point = point_add(
                        edit.new_end_point,
                        point_sub(range.end_point, edit.old_end_point),
                    );
                    self.flags = LayerUpdateFlags::MODIFIED;
                }
            }
        }
    }
}
