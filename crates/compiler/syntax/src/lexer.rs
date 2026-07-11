use pop_diagnostics::lexing;
use pop_foundation::{Diagnostic, SourceSpan, TextRange, TextSize};
use pop_source::SourceFile;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Whitespace,
    Newline,
    LineComment,
    DocumentationComment,
    Identifier,
    Number,
    String,
    Namespace,
    Using,
    Public,
    Internal,
    Private,
    Export,
    Function,
    Local,
    Return,
    End,
    Const,
    Record,
    Union,
    Class,
    Interface,
    Enum,
    Attribute,
    Type,
    Open,
    Implements,
    If,
    Then,
    Else,
    While,
    For,
    Do,
    Match,
    When,
    With,
    Nil,
    True,
    False,
    And,
    Or,
    Not,
    Dot,
    Comma,
    Colon,
    Equal,
    EqualEqual,
    TildeEqual,
    LeftParenthesis,
    RightParenthesis,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    LessThan,
    GreaterThan,
    At,
    Question,
    Pipe,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Unknown,
}

impl TokenKind {
    #[must_use]
    pub const fn is_trivia(self) -> bool {
        matches!(
            self,
            Self::Whitespace | Self::Newline | Self::LineComment | Self::DocumentationComment
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Token {
    kind: TokenKind,
    range: TextRange,
}

impl Token {
    #[must_use]
    pub const fn kind(self) -> TokenKind {
        self.kind
    }

    #[must_use]
    pub const fn range(self) -> TextRange {
        self.range
    }

    #[must_use]
    pub fn text(self, source: &SourceFile) -> &str {
        source
            .text()
            .get(self.range.start().to_usize()..self.range.end().to_usize())
            .unwrap_or("")
    }
}

#[derive(Clone, Debug)]
pub struct LexResult {
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl LexResult {
    #[must_use]
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn reconstruct(&self, source: &SourceFile) -> String {
        let mut text = String::with_capacity(source.text().len());
        for token in &self.tokens {
            text.push_str(token.text(source));
        }
        text
    }
}

#[must_use]
pub fn lex(source: &SourceFile) -> LexResult {
    Lexer {
        source,
        cursor: 0,
        tokens: Vec::new(),
        diagnostics: Vec::new(),
    }
    .run()
}

struct Lexer<'source> {
    source: &'source SourceFile,
    cursor: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl Lexer<'_> {
    fn run(mut self) -> LexResult {
        while self.cursor < self.source.text().len() {
            self.scan_token();
        }
        LexResult {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
        }
    }

    fn scan_token(&mut self) {
        let start = self.cursor;
        let remaining = &self.source.text()[start..];
        let byte = remaining.as_bytes()[0];

        let Some(first_character) = remaining.chars().next() else {
            return;
        };
        if first_character.is_alphabetic() || first_character == '_' {
            while let Some(character) = self.source.text()[self.cursor..].chars().next() {
                if !(character.is_alphanumeric() || character == '_') {
                    break;
                }
                self.cursor += character.len_utf8();
            }
            self.tokens.push(Token {
                kind: keyword(&self.source.text()[start..self.cursor]),
                range: Self::range(start, self.cursor),
            });
            return;
        }

        let kind = match byte {
            b' ' | b'\t' | b'\r' => {
                self.consume_while(|next| matches!(next, b' ' | b'\t' | b'\r'));
                TokenKind::Whitespace
            }
            b'\n' => {
                self.cursor += 1;
                TokenKind::Newline
            }
            b'-' if remaining.starts_with("---") => {
                self.consume_line();
                TokenKind::DocumentationComment
            }
            b'-' if remaining.starts_with("--") => {
                self.consume_line();
                TokenKind::LineComment
            }
            b'0'..=b'9' => {
                self.consume_while(|next| next.is_ascii_digit() || next == b'_');
                TokenKind::Number
            }
            b'\'' | b'"' => self.scan_string(start, byte),
            b'=' if remaining.starts_with("==") => {
                self.cursor += 2;
                TokenKind::EqualEqual
            }
            b'~' if remaining.starts_with("~=") => {
                self.cursor += 2;
                TokenKind::TildeEqual
            }
            _ => {
                self.cursor += char::from(byte).len_utf8();
                punctuation(byte).unwrap_or_else(|| {
                    let character = self.source.text()[start..]
                        .chars()
                        .next()
                        .expect("cursor points at a character");
                    self.cursor = start + character.len_utf8();
                    let range = Self::range(start, self.cursor);
                    self.diagnostics.push(lexing::invalid_character(
                        SourceSpan::new(self.source.id(), range),
                        character,
                    ));
                    TokenKind::Unknown
                })
            }
        };
        self.tokens.push(Token {
            kind,
            range: Self::range(start, self.cursor),
        });
    }

    fn scan_string(&mut self, start: usize, quote: u8) -> TokenKind {
        self.cursor += 1;
        let bytes = self.source.text().as_bytes();
        let mut escaped = false;
        while self.cursor < bytes.len() {
            let byte = bytes[self.cursor];
            if byte == b'\n' || byte == b'\r' {
                break;
            }
            self.cursor += 1;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == quote {
                return TokenKind::String;
            }
        }
        let range = Self::range(start, self.cursor);
        self.diagnostics
            .push(lexing::unterminated_string(SourceSpan::new(
                self.source.id(),
                range,
            )));
        TokenKind::String
    }

    fn consume_while(&mut self, predicate: impl Fn(u8) -> bool) {
        let bytes = self.source.text().as_bytes();
        while self.cursor < bytes.len() && predicate(bytes[self.cursor]) {
            self.cursor += 1;
        }
    }

    fn consume_line(&mut self) {
        let bytes = self.source.text().as_bytes();
        while self.cursor < bytes.len() && bytes[self.cursor] != b'\n' {
            self.cursor += 1;
        }
    }

    fn range(start: usize, end: usize) -> TextRange {
        TextRange::new(
            TextSize::try_from_usize(start).expect("validated source offset"),
            TextSize::try_from_usize(end).expect("validated source offset"),
        )
        .unwrap_or_else(|| TextRange::empty(TextSize::from_u32(0)))
    }
}

fn keyword(text: &str) -> TokenKind {
    match text {
        "namespace" => TokenKind::Namespace,
        "using" => TokenKind::Using,
        "public" => TokenKind::Public,
        "internal" => TokenKind::Internal,
        "private" => TokenKind::Private,
        "export" => TokenKind::Export,
        "function" => TokenKind::Function,
        "local" => TokenKind::Local,
        "return" => TokenKind::Return,
        "end" => TokenKind::End,
        "const" => TokenKind::Const,
        "record" => TokenKind::Record,
        "union" => TokenKind::Union,
        "class" => TokenKind::Class,
        "interface" => TokenKind::Interface,
        "enum" => TokenKind::Enum,
        "attribute" => TokenKind::Attribute,
        "type" => TokenKind::Type,
        "open" => TokenKind::Open,
        "implements" => TokenKind::Implements,
        "if" => TokenKind::If,
        "then" => TokenKind::Then,
        "else" => TokenKind::Else,
        "while" => TokenKind::While,
        "for" => TokenKind::For,
        "do" => TokenKind::Do,
        "match" => TokenKind::Match,
        "when" => TokenKind::When,
        "with" => TokenKind::With,
        "nil" => TokenKind::Nil,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "and" => TokenKind::And,
        "or" => TokenKind::Or,
        "not" => TokenKind::Not,
        _ => TokenKind::Identifier,
    }
}

fn punctuation(byte: u8) -> Option<TokenKind> {
    Some(match byte {
        b'.' => TokenKind::Dot,
        b',' => TokenKind::Comma,
        b':' => TokenKind::Colon,
        b'=' => TokenKind::Equal,
        b'(' => TokenKind::LeftParenthesis,
        b')' => TokenKind::RightParenthesis,
        b'{' => TokenKind::LeftBrace,
        b'}' => TokenKind::RightBrace,
        b'[' => TokenKind::LeftBracket,
        b']' => TokenKind::RightBracket,
        b'<' => TokenKind::LessThan,
        b'>' => TokenKind::GreaterThan,
        b'@' => TokenKind::At,
        b'?' => TokenKind::Question,
        b'|' => TokenKind::Pipe,
        b'+' => TokenKind::Plus,
        b'-' => TokenKind::Minus,
        b'*' => TokenKind::Star,
        b'/' => TokenKind::Slash,
        b'%' => TokenKind::Percent,
        _ => return None,
    })
}
