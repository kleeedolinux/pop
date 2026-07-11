//! Source files, UTF-aware line maps, and the source database.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use pop_foundation::{FileId, TextSize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineColumn {
    line: u32,
    column: u32,
}

impl LineColumn {
    #[must_use]
    pub const fn new(line: u32, column: u32) -> Self {
        Self { line, column }
    }

    #[must_use]
    pub const fn line(self) -> u32 {
        self.line
    }

    #[must_use]
    pub const fn column(self) -> u32 {
        self.column
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceError {
    FileTooLarge,
    TooManyFiles,
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileTooLarge => {
                formatter.write_str("source file exceeds the supported byte range")
            }
            Self::TooManyFiles => formatter.write_str("source database exhausted file identifiers"),
        }
    }
}

impl Error for SourceError {}

#[derive(Clone, Debug)]
pub struct SourceFile {
    id: FileId,
    path: Arc<str>,
    text: Arc<str>,
    length: TextSize,
    line_starts: Vec<TextSize>,
}

impl SourceFile {
    /// Creates one immutable source file and its line map.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::FileTooLarge`] when the UTF-8 text cannot be
    /// addressed by the compiler's 32-bit source offsets.
    pub fn new(
        id: FileId,
        path: impl Into<Arc<str>>,
        text: impl Into<Arc<str>>,
    ) -> Result<Self, SourceError> {
        let text = text.into();
        let length = TextSize::try_from_usize(text.len()).ok_or(SourceError::FileTooLarge)?;
        let mut line_starts = vec![TextSize::from_u32(0)];
        for (offset, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                let start = offset.checked_add(1).ok_or(SourceError::FileTooLarge)?;
                line_starts.push(TextSize::try_from_usize(start).ok_or(SourceError::FileTooLarge)?);
            }
        }

        Ok(Self {
            id,
            path: path.into(),
            text,
            length,
            line_starts,
        })
    }

    #[must_use]
    pub const fn id(&self) -> FileId {
        self.id
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub const fn len(&self) -> TextSize {
        self.length
    }

    #[must_use]
    pub fn line_column(&self, offset: TextSize) -> Option<LineColumn> {
        let offset = offset.to_usize();
        if offset > self.text.len() || !self.text.is_char_boundary(offset) {
            return None;
        }
        let line = self
            .line_starts
            .partition_point(|start| start.to_usize() <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts[line].to_usize();
        let column = self.text[line_start..offset].chars().count();
        Some(LineColumn::new(
            u32::try_from(line).ok()?,
            u32::try_from(column).ok()?,
        ))
    }

    #[must_use]
    pub fn offset(&self, position: LineColumn) -> Option<TextSize> {
        let line_start = *self.line_starts.get(position.line as usize)?;
        let suffix = &self.text[line_start.to_usize()..];
        let line_text = suffix.split_once('\n').map_or(suffix, |(line, _)| line);
        let byte_in_line = if position.column == 0 {
            0
        } else {
            line_text
                .char_indices()
                .map(|(offset, character)| offset + character.len_utf8())
                .nth(position.column as usize - 1)?
        };
        TextSize::try_from_usize(line_start.to_usize() + byte_in_line)
    }
}

#[derive(Debug, Default)]
pub struct SourceDatabase {
    files: BTreeMap<FileId, SourceFile>,
    next_file: u32,
}

impl SourceDatabase {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            next_file: 0,
        }
    }

    /// Adds a source file with a deterministic session-local [`FileId`].
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::FileTooLarge`] for an unsupported source length,
    /// or [`SourceError::TooManyFiles`] if the session exhausts file IDs.
    pub fn add(
        &mut self,
        path: impl Into<Arc<str>>,
        text: impl Into<Arc<str>>,
    ) -> Result<FileId, SourceError> {
        let id = FileId::from_raw(self.next_file);
        self.next_file = self
            .next_file
            .checked_add(1)
            .ok_or(SourceError::TooManyFiles)?;
        let file = SourceFile::new(id, path, text)?;
        self.files.insert(id, file);
        Ok(id)
    }

    #[must_use]
    pub fn file(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(&id)
    }
}
