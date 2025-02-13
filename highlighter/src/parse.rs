use std::mem::take;
use std::time::Duration;

use ropey::RopeSlice;
use tree_sitter::{InactiveQueryCursor, Parser};

use crate::config::LanguageLoader;
use crate::{Error, LayerData, Syntax, TREE_SITTER_MATCH_LIMIT};

impl Syntax {
    pub fn update(
        &mut self,
        source: RopeSlice,
        timeout: Duration,
        edits: &[tree_sitter::InputEdit],
        loader: &impl LanguageLoader,
    ) -> Result<(), Error> {
        // size limit of 512MiB, TS just cannot handle files this big (too
        // slow). Furthermore, TS uses 32 (signed) bit indices so this limit
        // must never be raised above 2GiB
        if source.len_bytes() >= 512 * 1024 * 1024 {
            return Err(Error::ExceededMaximumSize);
        }

        let mut queue = Vec::with_capacity(32);
        let root_flags = &mut self.layer_mut(self.root).flags;
        // The root layer is always considered.
        root_flags.touched = true;
        // If there was an edit then the root layer must've been modified.
        root_flags.modified = true;
        queue.push(self.root);

        let mut parser = Parser::new();
        parser.set_timeout(timeout);
        let mut cursor = InactiveQueryCursor::new();
        // TODO: might need to set cursor range
        cursor.set_byte_range(0..u32::MAX);
        cursor.set_match_limit(TREE_SITTER_MATCH_LIMIT);

        while let Some(layer) = queue.pop() {
            let layer_data = self.layer_mut(layer);
            if let Some(tree) = &mut layer_data.parse_tree {
                if layer_data.flags.moved || layer_data.flags.modified {
                    for edit in edits.iter().rev() {
                        // Apply the edits in reverse.
                        // If we applied them in order then edit 1 would disrupt the positioning
                        // of edit 2.
                        tree.edit(edit);
                    }
                }
                if layer_data.flags.modified {
                    // Re-parse the tree.
                    layer_data.parse(&mut parser, source, loader)?;
                }
            } else {
                // always parse if this layer has never been parsed before
                layer_data.parse(&mut parser, source, loader)?;
            }
            self.run_injection_query(layer, edits, source, loader, |layer| queue.push(layer));
            self.run_local_query(layer, source, loader);
        }

        if self.layer(self.root).parse_tree.is_none() {
            return Err(Error::NoRootConfig);
        }

        self.prune_dead_layers();
        Ok(())
    }

    /// Reset all `LayerUpdateFlags` and remove all untouched layers
    fn prune_dead_layers(&mut self) {
        self.layers
            .retain(|_, layer| take(&mut layer.flags).touched);
    }
}

// /// Compute the ranges that should be included when parsing an injection.
// /// This takes into account three things:
// /// * `parent_ranges` - The ranges must all fall within the *current* layer's ranges.
// /// * `nodes` - Every injection takes place within a set of nodes. The injection ranges
// ///   are the ranges of those nodes.
// /// * `includes_children` - For some injections, the content nodes' children should be
// ///   excluded from the nested document, so that only the content nodes' *own* content
// ///   is reparsed. For other injections, the content nodes' entire ranges should be
// ///   reparsed, including the ranges of their children.
// fn intersect_ranges(
//     parent_ranges: &[Range],
//     nodes: &[Node],
//     included_children: IncludedChildren,
// ) -> Vec<Range> {
//     let mut cursor = nodes[0].walk();
//     let mut result = Vec::new();
//     let mut parent_range_iter = parent_ranges.iter();
//     let mut parent_range = parent_range_iter
//         .next()
//         .expect("Layers should only be constructed with non-empty ranges vectors");
//     for node in nodes.iter() {
//         let mut preceding_range = Range {
//             start_byte: 0,
//             start_point: Point::new(0, 0),
//             end_byte: node.start_byte(),
//             end_point: node.start_position(),
//         };
//         let following_range = Range {
//             start_byte: node.end_byte(),
//             start_point: node.end_position(),
//             end_byte: u32::MAX,
//             end_point: Point::new(usize::MAX, usize::MAX),
//         };

//         for excluded_range in node
//             .children(&mut cursor)
//             .filter_map(|child| match included_children {
//                 IncludedChildren::None => Some(child.range()),
//                 IncludedChildren::All => None,
//                 IncludedChildren::Unnamed => {
//                     if child.is_named() {
//                         Some(child.range())
//                     } else {
//                         None
//                     }
//                 }
//             })
//             .chain([following_range].iter().cloned())
//         {
//             let mut range = Range {
//                 start_byte: preceding_range.end_byte,
//                 start_point: preceding_range.end_point,
//                 end_byte: excluded_range.start_byte,
//                 end_point: excluded_range.start_point,
//             };
//             preceding_range = excluded_range;

//             if range.end_byte < parent_range.start_byte {
//                 continue;
//             }

//             while parent_range.start_byte <= range.end_byte {
//                 if parent_range.end_byte > range.start_byte {
//                     if range.start_byte < parent_range.start_byte {
//                         range.start_byte = parent_range.start_byte;
//                         range.start_point = parent_range.start_point;
//                     }

//                     if parent_range.end_byte < range.end_byte {
//                         if range.start_byte < parent_range.end_byte {
//                             result.push(Range {
//                                 start_byte: range.start_byte,
//                                 start_point: range.start_point,
//                                 end_byte: parent_range.end_byte,
//                                 end_point: parent_range.end_point,
//                             });
//                         }
//                         range.start_byte = parent_range.end_byte;
//                         range.start_point = parent_range.end_point;
//                     } else {
//                         if range.start_byte < range.end_byte {
//                             result.push(range);
//                         }
//                         break;
//                     }
//                 }

//                 if let Some(next_range) = parent_range_iter.next() {
//                     parent_range = next_range;
//                 } else {
//                     return result;
//                 }
//             }
//         }
//     }
//     result
// }

impl LayerData {
    fn parse(
        &mut self,
        parser: &mut Parser,
        source: RopeSlice,
        loader: &impl LanguageLoader,
    ) -> Result<(), Error> {
        let Some(config) = loader.get_config(self.language) else {
            return Ok(());
        };
        parser.set_grammar(config.grammar);
        parser
            .set_included_ranges(&self.ranges)
            .map_err(|_| Error::InvalidRanges)?;
        let tree = parser
            .parse(source, self.parse_tree.as_ref())
            .ok_or(Error::Timeout)?;
        self.parse_tree = Some(tree);
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Default, Clone)]
pub(crate) struct LayerUpdateFlags {
    pub reused: bool,
    pub modified: bool,
    pub moved: bool,
    pub touched: bool,
}
