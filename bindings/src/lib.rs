mod grammar;
mod parser;
pub mod query;
mod query_cursor;
mod syntax_tree;
mod syntax_tree_node;
mod tree_cursor;

#[cfg(feature = "ropey")]
mod ropey;
#[cfg(feature = "ropey")]
pub use ropey::RopeTsInput;

use std::cmp::min;
use std::ops;

pub use grammar::Grammar;
pub use parser::{Parser, ParserInputRaw};
pub use query::{Capture, Pattern, Query, QueryStr};
pub use query_cursor::{InactiveQueryCursor, MatchedNode, MatchedNodeIdx, QueryCursor, QueryMatch};
pub use syntax_tree::{InputEdit, SyntaxTree};
pub use syntax_tree_node::SyntaxTreeNode;
pub use tree_cursor::TreeCursor;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub row: u32,
    pub col: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Range {
    pub start_point: Point,
    pub end_point: Point,
    pub start_byte: u32,
    pub end_byte: u32,
}

pub trait TsInput {
    type Cursor: regex_cursor::Cursor;
    fn cursor_at(&mut self, offset: u32) -> &mut Self::Cursor;
    fn eq(&mut self, range1: ops::Range<u32>, range2: ops::Range<u32>) -> bool;
}

impl<T: TsInput> IntoTsInput for T {
    type TsInput = T;

    fn into_ts_input(self) -> T {
        self
    }
}

pub trait IntoTsInput {
    type TsInput: TsInput;
    fn into_ts_input(self) -> Self::TsInput;
}

// workaround for missing features in regex cursor/regex crate
pub(crate) struct CursorSlice<T: regex_cursor::Cursor> {
    cursor: T,
    start: usize,
    end: usize,
}

impl<T: regex_cursor::Cursor> CursorSlice<T> {
    pub fn new(cursor: T, start: usize, end: usize) -> Self {
        debug_assert!(start <= cursor.offset() + cursor.chunk().len());
        debug_assert!(end >= cursor.offset());
        Self { cursor, start, end }
    }
}

impl<T: regex_cursor::Cursor> regex_cursor::Cursor for CursorSlice<T> {
    #[inline]
    fn chunk(&self) -> &[u8] {
        let chunk = self.cursor.chunk();
        let end = min(self.end - self.cursor.offset(), chunk.len());
        &chunk[..end]
    }

    #[inline]
    fn advance(&mut self) -> bool {
        if self.end <= self.cursor.offset() + self.cursor.chunk().len() {
            return false;
        }
        self.cursor.advance()
    }

    #[inline]
    fn backtrack(&mut self) -> bool {
        if self.start >= self.cursor.offset() {
            return false;
        }
        self.cursor.backtrack()
    }

    #[inline]
    fn total_bytes(&self) -> Option<usize> {
        Some(self.end - self.start)
    }

    #[inline]
    fn offset(&self) -> usize {
        self.cursor.offset() - self.start
    }
}
