use ropey::{Rope, RopeSlice};
use std::fmt::Write;
use std::ops::{Bound, RangeBounds};
use std::time::Duration;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::config::LanguageLoader;
use crate::highlighter::{HighlighEvent, Highlight, Highligther};
use crate::{Language, Range, Syntax};

macro_rules! w {
    ($dst: expr$(, $($args: tt)*)?) => {{
        let _ = write!($dst$(, $($args)*)?);
    }};
}
macro_rules! wln {
    ($dst: expr$(, $($args: tt)*)?) => {{
        let _ = writeln!($dst$(, $($args)*)?);
    }};
}

pub fn roundtrip_fixture<R: RangeBounds<usize>>(
    comment_prefix: &str,
    language: Language,
    loader: &impl LanguageLoader,
    get_highlight_name: impl Fn(Highlight) -> String,
    src: &str,
    range: impl Fn(RopeSlice) -> R,
) -> String {
    let ident = " ".repeat(comment_prefix.width());
    let mut raw = String::new();
    for mut line in src.split_inclusive('\n') {
        if line.starts_with(comment_prefix) {
            continue;
        }
        line = line.strip_prefix(&ident).unwrap_or(line);
        raw.push_str(line);
    }

    let raw = Rope::from_str(&raw);
    let syntax = Syntax::new(raw.slice(..), language, Duration::from_secs(60), loader).unwrap();
    let range = range(raw.slice(..));
    highlighter_fixture(
        comment_prefix,
        loader,
        get_highlight_name,
        &syntax,
        raw.slice(..),
        range,
    )
}

pub fn highlighter_fixture(
    comment_prefix: &str,
    loader: &impl LanguageLoader,
    get_highlight_name: impl Fn(Highlight) -> String,
    syntax: &Syntax,
    src: RopeSlice<'_>,
    range: impl RangeBounds<usize>,
) -> String {
    let start = match range.start_bound() {
        Bound::Included(&i) => i,
        Bound::Excluded(&i) => i + 1,
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Included(&i) => i - 1,
        Bound::Excluded(&i) => i,
        Bound::Unbounded => src.len_bytes(),
    };
    let ident = " ".repeat(comment_prefix.width());
    let mut highlighter = Highligther::new(syntax, src, &loader, start as u32..);
    let mut pos = highlighter.next_event_offset();
    let mut highlight_stack = Vec::new();
    let mut line_idx = src.byte_to_line(pos as usize);
    let mut line_start = src.line_to_byte(line_idx) as u32;
    let mut line_end = src.line_to_byte(line_idx + 1) as u32;
    let mut line_highlights = Vec::new();
    let mut res = String::new();
    for line in src.byte_slice(..line_start as usize).lines() {
        if line.len_bytes() != 0 {
            wln!(res, "{ident}{line}")
        }
    }
    let mut errors = String::new();
    while pos < end as u32 {
        let new_highlights = match highlighter.advance() {
            HighlighEvent::RefreshHiglights(highlights) => {
                highlight_stack.clear();
                highlights
            }
            HighlighEvent::PushHighlights(highlights) => highlights,
        };
        highlight_stack.extend(new_highlights.map(&get_highlight_name));
        let mut start = pos;
        pos = highlighter.next_event_offset();
        if pos == u32::MAX {
            pos = src.len_bytes() as u32
        }
        if pos <= start {
            wln!(
                errors,
                "INVALID HIGHLIGHT RANGE: {start}..{pos} {:?} {:?}",
                src.byte_slice(pos as usize..start as usize),
                highlight_stack
            );
            start = pos;
        }

        while start >= line_end {
            res.push_str(&ident);
            res.extend(
                src.byte_slice(line_start as usize..line_end as usize)
                    .chunks(),
            );
            render_fixture_line(
                comment_prefix,
                src,
                line_start,
                &mut line_highlights,
                &mut res,
            );
            line_highlights.clear();
            line_idx += 1;
            line_start = line_end;
            line_end = src
                .try_line_to_byte(line_idx + 1)
                .unwrap_or(src.len_bytes()) as u32;
        }
        if !highlight_stack.is_empty() {
            let range = start..pos.min(line_end);
            if !range.is_empty() {
                line_highlights.push((range, highlight_stack.clone()))
            }
        }
        while pos > line_end {
            res.push_str(&ident);
            res.extend(
                src.byte_slice(line_start as usize..line_end as usize)
                    .chunks(),
            );
            render_fixture_line(
                comment_prefix,
                src,
                line_start,
                &mut line_highlights,
                &mut res,
            );
            line_highlights.clear();
            line_idx += 1;
            line_start = line_end;
            line_end = src
                .try_line_to_byte(line_idx + 1)
                .unwrap_or(src.len_bytes()) as u32;
            line_highlights.is_empty();
            if pos > line_start && !highlight_stack.is_empty() {
                line_highlights.push((line_start..pos.min(line_end), Vec::new()))
            }
        }
    }
    if !line_highlights.is_empty() {
        res.push_str(&ident);
        res.extend(
            src.byte_slice(line_start as usize..line_end as usize)
                .chunks(),
        );
        if !res.ends_with('\n') {
            res.push('\n');
        }
        render_fixture_line(
            comment_prefix,
            src,
            line_start,
            &mut line_highlights,
            &mut res,
        );
    }
    for line in src.byte_slice(line_end as usize..).lines() {
        if line.len_bytes() != 0 {
            wln!(res, "{comment_prefix}{line}")
        }
    }
    res
}

fn render_fixture_line(
    comment_prefix: &str,
    src: RopeSlice<'_>,
    line_start: u32,
    highlights: &mut Vec<(Range, Vec<String>)>,
    dst: &mut String,
) {
    if highlights.is_empty() {
        return;
    }
    highlights.dedup_by(|(src_range, src_scopes), (dst_range, dst_scopes)| {
        if dst_scopes == src_scopes && dst_range.end == src_range.start {
            dst_range.end = src_range.end;
            true
        } else {
            false
        }
    });
    w!(dst, "{comment_prefix}");
    let mut prev_pos = line_start;
    let mut offsets = Vec::with_capacity(highlights.len());
    for (i, (range, scopes)) in highlights.iter().enumerate() {
        let offset = src
            .byte_slice(prev_pos as usize..range.start as usize)
            .chars()
            .map(|c| c.width().unwrap_or(0))
            .sum();
        let mut width: usize = src
            .byte_slice(range.start as usize..range.end as usize)
            .chars()
            .map(|c| c.width().unwrap_or(0))
            .sum();
        width = width.saturating_sub(1);
        offsets.push((offset, width));
        let first_char = if scopes.is_empty() {
            "━"
        } else if i == highlights.len() - 1 {
            "╰"
        } else if width == 0 {
            "╿"
        } else {
            "┡"
        };
        let last_char = if i == highlights.len() - 1 {
            "┹"
        } else {
            "┛"
        };
        if width == 0 {
            w!(dst, "{0:^offset$}{first_char}", "");
        } else {
            width -= 1;
            w!(dst, "{0:^offset$}{first_char}{0:━^width$}{last_char}", "");
        }
        prev_pos = range.end;
    }
    let Some(i) = highlights.iter().position(|(_, scopes)| !scopes.is_empty()) else {
        wln!(dst);
        return;
    };
    let highlights = &highlights[i..];
    let offset: usize = offsets
        .drain(..i)
        .map(|(offset, width)| offset + width + 1)
        .sum();
    offsets[0].0 += offset;
    w!(dst, "─");
    for highlight in &highlights.last().unwrap().1 {
        w!(dst, " {highlight}")
    }
    wln!(dst);
    for depth in (0..highlights.len().saturating_sub(1)).rev() {
        w!(dst, "{comment_prefix}");
        for &(offset, width) in &offsets[..depth] {
            w!(dst, "{0:^offset$}│{0:^width$}", "");
        }
        let offset = offsets[depth].0;
        w!(dst, "{:>offset$}╰─", "");
        for highlight in &highlights[depth].1 {
            w!(dst, " {highlight}")
        }
        wln!(dst);
    }
}
