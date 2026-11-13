//! Pure retained-text algorithms for search and copy-mode motion.
//!
//! Operates on captured screen rows, without terminal locks, PTY writes or rendering.
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TerminalTextPoint {
    pub row: u32,
    pub col: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalTextMatch {
    pub start: TerminalTextPoint,
    pub end: TerminalTextPoint,
    pub source_fingerprint: u64,
    pub scan_cols: u16,
    pub scan_screen: crate::ghostty::ActiveScreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalSearchDirection {
    Forward,
    Backward,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct TerminalSearchWindow {
    pub matches: Vec<TerminalTextMatch>,
    pub current: Option<usize>,
    pub current_global: Option<usize>,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalWordMotion {
    NextStart,
    PreviousStart,
    NextEnd,
    NextBigStart,
    PreviousBigStart,
    NextBigEnd,
}

const COPY_MODE_WORD_SEPARATORS: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^`{|}~";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextClass {
    Whitespace,
    Separator,
    Word,
}

#[derive(Debug)]
struct TextAtom {
    point: Option<TerminalTextPoint>,
    end_col: u16,
    class: TextClass,
}

#[derive(Debug)]
struct TextSpan {
    byte_start: usize,
    byte_end: usize,
    start: TerminalTextPoint,
    end: TerminalTextPoint,
}

#[derive(Debug, Default)]
struct LogicalTextLine {
    text: String,
    spans: Vec<TextSpan>,
}

struct TextMatchCandidate<'a> {
    start: TerminalTextPoint,
    end: TerminalTextPoint,
    text: &'a str,
}

#[derive(Debug)]
pub(super) struct RetainedTextBuffer {
    cols: u16,
    lines: Vec<LogicalTextLine>,
    atoms: Vec<TextAtom>,
}

enum TextBuildMode {
    Search,
    Words,
    #[cfg(test)]
    Both,
}

impl RetainedTextBuffer {
    #[cfg(test)]
    pub(super) fn new(cols: u16, rows: Vec<crate::ghostty::ScreenTextRow>) -> Self {
        Self::build(cols, rows, 0, TextBuildMode::Both)
    }

    pub(super) fn new_search(
        cols: u16,
        rows: Vec<crate::ghostty::ScreenTextRow>,
        row_offset: u32,
    ) -> Self {
        Self::build(cols, rows, row_offset, TextBuildMode::Search)
    }

    pub(super) fn new_words(
        cols: u16,
        rows: Vec<crate::ghostty::ScreenTextRow>,
        row_offset: u32,
    ) -> Self {
        Self::build(cols, rows, row_offset, TextBuildMode::Words)
    }

    fn build(
        cols: u16,
        rows: Vec<crate::ghostty::ScreenTextRow>,
        row_offset: u32,
        mode: TextBuildMode,
    ) -> Self {
        // Build only the representation needed by this operation.
        let (build_lines, build_atoms) = match mode {
            TextBuildMode::Search => (true, false),
            TextBuildMode::Words => (false, true),
            #[cfg(test)]
            TextBuildMode::Both => (true, true),
        };
        let mut lines = Vec::new();
        let mut line = LogicalTextLine::default();
        let mut atoms: Vec<TextAtom> = Vec::new();

        for (row_idx, row) in rows.into_iter().enumerate() {
            let Some(row_idx) = u32::try_from(row_idx).ok() else {
                break;
            };
            let row_idx = row_offset.saturating_add(row_idx);
            for (col, cell) in row.cells.into_iter().enumerate() {
                let Ok(col) = u16::try_from(col) else {
                    break;
                };
                if cell.wide == crate::ghostty::CellWide::SpacerTail {
                    continue;
                }
                if cell.wide == crate::ghostty::CellWide::SpacerHead {
                    if build_atoms {
                        atoms.push(TextAtom {
                            point: Some(TerminalTextPoint { row: row_idx, col }),
                            end_col: col,
                            class: atoms
                                .last()
                                .map_or(TextClass::Whitespace, |atom| atom.class),
                        });
                    }
                    continue;
                }
                let width = if cell.wide == crate::ghostty::CellWide::Wide {
                    2
                } else {
                    1
                };
                let text = terminal_cell_text(&cell.graphemes);
                let start = TerminalTextPoint { row: row_idx, col };
                let end = TerminalTextPoint {
                    row: row_idx,
                    col: col.saturating_add(width - 1),
                };
                if build_lines {
                    let byte_start = line.text.len();
                    line.text.push_str(&text);
                    let byte_end = line.text.len();
                    line.spans.push(TextSpan {
                        byte_start,
                        byte_end,
                        start,
                        end,
                    });
                }
                if build_atoms {
                    atoms.push(TextAtom {
                        point: Some(start),
                        end_col: end.col,
                        class: text_class(&text),
                    });
                }
            }

            if row.soft_wrapped {
                continue;
            }

            if build_lines {
                let trimmed_len = line.text.trim_end().len();
                while line
                    .spans
                    .last()
                    .is_some_and(|span| span.byte_start >= trimmed_len)
                {
                    line.spans.pop();
                }
                line.text.truncate(trimmed_len);
                lines.push(std::mem::take(&mut line));
            }
            if build_atoms {
                atoms.push(TextAtom {
                    point: None,
                    end_col: 0,
                    class: TextClass::Whitespace,
                });
            }
        }

        if build_lines && (!line.text.is_empty() || !line.spans.is_empty()) {
            lines.push(line);
        }

        Self { cols, lines, atoms }
    }

    pub(super) fn search_window(
        &self,
        query: &str,
        case_sensitive: bool,
        active_screen: crate::ghostty::ActiveScreen,
        direction: TerminalSearchDirection,
        cursor: TerminalTextPoint,
        previous: Option<(TerminalTextPoint, TerminalTextPoint)>,
        limit: usize,
    ) -> TerminalSearchWindow {
        if query.is_empty() || limit == 0 {
            return TerminalSearchWindow::default();
        }
        let Ok(regex) = regex::RegexBuilder::new(&regex::escape(query))
            .case_insensitive(!case_sensitive)
            .build()
        else {
            return TerminalSearchWindow::default();
        };

        let origin = match direction {
            TerminalSearchDirection::Forward => previous.map_or(cursor, |(_, end)| end),
            TerminalSearchDirection::Backward => previous.map_or(cursor, |(start, _)| start),
        };
        let mut total = 0usize;
        let mut target = None;
        for found in self.search_matches(&regex) {
            match direction {
                TerminalSearchDirection::Forward if target.is_none() && found.start > origin => {
                    target = Some(total);
                }
                TerminalSearchDirection::Backward if found.end < origin => {
                    target = Some(total);
                }
                _ => {}
            }
            total = total.saturating_add(1);
        }
        if total == 0 {
            return TerminalSearchWindow::default();
        }
        let target = target.unwrap_or(match direction {
            TerminalSearchDirection::Forward => 0,
            TerminalSearchDirection::Backward => total - 1,
        });
        let retained = limit.min(total);
        let start = target
            .saturating_sub(retained / 2)
            .min(total.saturating_sub(retained));

        // Keep two scans rather than retaining every match. Only the bounded
        // output window needs fingerprints and complete match records.
        let mut matches = Vec::with_capacity(retained);
        matches.extend(
            self.search_matches(&regex)
                .skip(start)
                .take(retained)
                .map(|found| TerminalTextMatch {
                    start: found.start,
                    end: found.end,
                    source_fingerprint: text_fingerprint(found.text),
                    scan_cols: self.cols,
                    scan_screen: active_screen,
                }),
        );
        TerminalSearchWindow {
            matches,
            current: Some(target - start),
            current_global: Some(target),
            total,
        }
    }

    fn search_matches<'a>(
        &'a self,
        regex: &'a regex::Regex,
    ) -> impl Iterator<Item = TextMatchCandidate<'a>> + 'a {
        self.lines.iter().flat_map(move |line| {
            regex.find_iter(&line.text).filter_map(move |found| {
                // A valid match must cover complete terminal graphemes.
                let start_index = line
                    .spans
                    .binary_search_by_key(&found.start(), |span| span.byte_start)
                    .ok()?;
                let end_index = line
                    .spans
                    .binary_search_by_key(&found.end(), |span| span.byte_end)
                    .ok()?;
                Some(TextMatchCandidate {
                    start: line.spans[start_index].start,
                    end: line.spans[end_index].end,
                    text: found.as_str(),
                })
            })
        })
    }

    pub(super) fn word_motion(
        &self,
        row: u32,
        col: u16,
        motion: TerminalWordMotion,
    ) -> Option<TerminalTextPoint> {
        let current = self.atoms.iter().position(|atom| {
            atom.point
                .is_some_and(|point| point.row == row && col >= point.col && col <= atom.end_col)
        })?;
        match motion {
            TerminalWordMotion::NextStart => self.next_word_start(current),
            TerminalWordMotion::PreviousStart => self.previous_word_start(current),
            TerminalWordMotion::NextEnd => self.next_word_end(current),
            TerminalWordMotion::NextBigStart => self.next_big_word_start(current),
            TerminalWordMotion::PreviousBigStart => self.previous_big_word_start(current),
            TerminalWordMotion::NextBigEnd => self.next_big_word_end(current),
        }
    }

    fn next_word_start(&self, current: usize) -> Option<TerminalTextPoint> {
        let current_class = self.atoms.get(current)?.class;
        let mut next = current.saturating_add(1);
        if current_class != TextClass::Whitespace {
            while self
                .atoms
                .get(next)
                .is_some_and(|atom| atom.class == current_class)
            {
                next += 1;
            }
        }
        while self
            .atoms
            .get(next)
            .is_some_and(|atom| atom.class == TextClass::Whitespace)
        {
            next += 1;
        }
        self.next_point(next)
    }

    fn previous_word_start(&self, current: usize) -> Option<TerminalTextPoint> {
        let mut previous = current.checked_sub(1)?;
        while self
            .atoms
            .get(previous)
            .is_some_and(|atom| atom.class == TextClass::Whitespace)
        {
            previous = previous.checked_sub(1)?;
        }
        let class = self.atoms.get(previous)?.class;
        while previous > 0
            && self
                .atoms
                .get(previous - 1)
                .is_some_and(|atom| atom.class == class)
        {
            previous -= 1;
        }
        self.previous_point(previous)
    }

    fn next_word_end(&self, current: usize) -> Option<TerminalTextPoint> {
        let mut next = current.saturating_add(1);
        while self
            .atoms
            .get(next)
            .is_some_and(|atom| atom.class == TextClass::Whitespace)
        {
            next += 1;
        }
        let class = self.atoms.get(next)?.class;
        while self
            .atoms
            .get(next + 1)
            .is_some_and(|atom| atom.class == class)
        {
            next += 1;
        }
        self.previous_point(next)
    }

    fn next_big_word_start(&self, current: usize) -> Option<TerminalTextPoint> {
        let mut next = current.saturating_add(1);
        if self
            .atoms
            .get(current)
            .is_some_and(|atom| atom.class != TextClass::Whitespace)
        {
            while self
                .atoms
                .get(next)
                .is_some_and(|atom| atom.class != TextClass::Whitespace)
            {
                next += 1;
            }
        }
        while self
            .atoms
            .get(next)
            .is_some_and(|atom| atom.class == TextClass::Whitespace)
        {
            next += 1;
        }
        self.next_point(next)
    }

    fn previous_big_word_start(&self, current: usize) -> Option<TerminalTextPoint> {
        let mut previous = current.checked_sub(1)?;
        while self
            .atoms
            .get(previous)
            .is_some_and(|atom| atom.class == TextClass::Whitespace)
        {
            previous = previous.checked_sub(1)?;
        }
        while previous > 0
            && self
                .atoms
                .get(previous - 1)
                .is_some_and(|atom| atom.class != TextClass::Whitespace)
        {
            previous -= 1;
        }
        self.previous_point(previous)
    }

    fn next_big_word_end(&self, current: usize) -> Option<TerminalTextPoint> {
        let mut next = current.saturating_add(1);
        while self
            .atoms
            .get(next)
            .is_some_and(|atom| atom.class == TextClass::Whitespace)
        {
            next += 1;
        }
        self.atoms.get(next)?;
        while self
            .atoms
            .get(next + 1)
            .is_some_and(|atom| atom.class != TextClass::Whitespace)
        {
            next += 1;
        }
        self.previous_point(next)
    }

    fn next_point(&self, mut index: usize) -> Option<TerminalTextPoint> {
        while let Some(atom) = self.atoms.get(index) {
            if let Some(point) = atom.point {
                return Some(point);
            }
            index += 1;
        }
        None
    }

    fn previous_point(&self, mut index: usize) -> Option<TerminalTextPoint> {
        loop {
            if let Some(point) = self.atoms.get(index)?.point {
                return Some(point);
            }
            index = index.checked_sub(1)?;
        }
    }

    pub(super) fn point_is_final_atom(&self, point: TerminalTextPoint) -> bool {
        // Word motion targets are atom start points, so compare against the
        // final atom's start point. Comparing against `end_col` would never
        // match a wide glyph, whose end column is one past its start.
        self.atoms
            .iter()
            .rev()
            .find(|atom| atom.point.is_some())
            .is_some_and(|atom| atom.point == Some(point))
    }
}

pub(super) fn terminal_cell_text(graphemes: &[u32]) -> String {
    if graphemes.is_empty()
        || graphemes.first().copied() == Some(crate::ghostty::KITTY_UNICODE_PLACEHOLDER)
    {
        return " ".to_string();
    }
    graphemes
        .iter()
        .map(|codepoint| char::from_u32(*codepoint).unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn text_class(text: &str) -> TextClass {
    let Some(ch) = text.chars().next() else {
        return TextClass::Whitespace;
    };
    if ch.is_whitespace() {
        TextClass::Whitespace
    } else if ch.is_ascii() && COPY_MODE_WORD_SEPARATORS.contains(ch) {
        TextClass::Separator
    } else {
        TextClass::Word
    }
}

fn text_fingerprint(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}
