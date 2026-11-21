//! JavaScript tokenizer for display. It never evaluates the script; it only
//! splits the text into tokens whose concatenation is the original text.
//!
//! A `/` is a regular expression or a division depending on the previous
//! token. The lexer decides with the usual rule (a value before `/` makes it
//! a division) and tracks open brackets so that `)` after `if (…)` and `}`
//! after a block allow a regular expression. Input that would only make
//! sense under the other reading, such as a string broken by a line end or a
//! mismatched bracket, is rejected so the caller can show the original text.

/// Lexical class of a [`Token`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Kind {
    /// Whitespace and line terminators.
    Space,
    LineComment,
    BlockComment,
    String,
    Template,
    Regex,
    Number,
    /// Identifiers, keywords, and private names.
    Word,
    /// Operators, brackets, and other punctuators.
    Punct,
    /// A character that cannot start a token, kept as it is.
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Token<'a> {
    pub(super) kind: Kind,
    pub(super) text: &'a str,
}

/// Tokens of a script and, if the text ends inside a token or with open
/// brackets, what was left unfinished.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct Lexed<'a> {
    pub(super) tokens: Vec<Token<'a>>,
    pub(super) unfinished: Option<&'static str>,
}

/// Keywords after which a `/` starts a regular expression.
const REGEX_KEYWORDS: [&str; 15] = [
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
    "extends",
];
/// Keywords whose parenthesized condition may be followed by a statement
/// that starts with a regular expression.
const CONDITION_KEYWORDS: [&str; 4] = ["if", "while", "for", "with"];
/// Keywords directly followed by a block.
const BLOCK_KEYWORDS: [&str; 4] = ["else", "do", "try", "finally"];
/// Punctuators, longest first so the longest match wins.
const PUNCTUATORS: [&str; 52] = [
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=",
    "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
    "**", "<<", ">>", "{", "}", "(", ")", "[", "]", ";", ",", "<", ">", "+", "-", "*", "/", "%",
    "&", "|", "^", "!",
];
const SINGLE_PUNCTUATORS: [char; 6] = ['~', '?', ':', '=', '.', '@'];

/// An open bracket and whether a `/` right after its closing bracket starts
/// a regular expression.
#[derive(Clone, Copy, Debug)]
struct Open {
    bracket: char,
    regex_after_close: bool,
}

/// How a scan of a delimited token ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum End {
    /// Closed; the token is this many bytes long.
    Closed(usize),
    /// The text ended first.
    Eof,
    /// A line terminator ended the line first, after this many bytes.
    Newline(usize),
}

/// Splits `text` into tokens. Fails, with a byte offset, when the text
/// cannot be read as JavaScript without guessing.
pub(super) fn lex(text: &str) -> Result<Lexed<'_>, String> {
    Lexer::default().run(text)
}

/// What the last significant token means for the next one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Previous {
    /// Start of the script or of a statement: `/` starts a regular
    /// expression and `{` opens a block.
    #[default]
    Statement,
    /// An operator or a keyword that takes an operand: `/` starts a
    /// regular expression and `{` opens an object.
    Operator,
    /// An identifier or a closed condition: `/` divides and `{` opens a
    /// block (a class or function body).
    Name,
    /// A literal or other value: `/` divides and `{` cannot follow.
    Value,
    /// `if`, `while`, `for`, or `with`, whose condition comes next.
    Condition,
    /// `.` or `?.`: the next word is a property name, not a keyword.
    Dot,
}

impl Previous {
    const fn regex_allowed(self) -> bool {
        matches!(self, Self::Statement | Self::Operator)
    }

    const fn block_allowed(self) -> bool {
        matches!(self, Self::Statement | Self::Name)
    }

    fn after_word(self, word: &str) -> Self {
        if self == Self::Dot {
            Self::Name
        } else if CONDITION_KEYWORDS.contains(&word) {
            Self::Condition
        } else if BLOCK_KEYWORDS.contains(&word) {
            Self::Statement
        } else if REGEX_KEYWORDS.contains(&word) {
            Self::Operator
        } else {
            Self::Name
        }
    }

    /// The state after the closing bracket of `open`.
    const fn after_close(open: Open) -> Self {
        match (open.bracket, open.regex_after_close) {
            (_, true) => Self::Statement,
            ('(', false) => Self::Name,
            _ => Self::Value,
        }
    }
}

#[derive(Default)]
struct Lexer {
    opens: Vec<Open>,
    previous: Previous,
}

impl Lexer {
    fn run(mut self, text: &str) -> Result<Lexed<'_>, String> {
        let mut tokens = Vec::new();
        let mut unfinished = None;
        let mut rest = text;
        if rest.starts_with("#!") {
            let (line, after) = split_line(rest);
            tokens.push(Token {
                kind: Kind::LineComment,
                text: line,
            });
            rest = after;
        }
        while !rest.is_empty() {
            let offset = text.len() - rest.len();
            let (kind, len, cut) = self.next(rest, offset)?;
            let (piece, after) = rest.split_at(len);
            if cut.is_some() {
                unfinished = cut;
            }
            self.advance(kind, piece, offset)?;
            tokens.push(Token { kind, text: piece });
            rest = after;
        }
        if unfinished.is_none() {
            unfinished = self.opens.last().map(|open| match open.bracket {
                '{' => "block or object",
                '(' => "parenthesis",
                _ => "array or index",
            });
        }
        Ok(Lexed { tokens, unfinished })
    }

    /// Classifies the token at the start of `rest` and returns its length.
    fn next(
        &self,
        rest: &str,
        offset: usize,
    ) -> Result<(Kind, usize, Option<&'static str>), String> {
        let mut chars = rest.chars();
        let first = chars.next().unwrap_or_default();
        let second = chars.next();
        let token = match (first, second) {
            (c, _) if is_space(c) => (Kind::Space, prefix_len(rest, is_space), None),
            ('/', Some('/')) => (Kind::LineComment, split_line(rest).0.len(), None),
            ('/', Some('*')) => rest
                .get(2..)
                .and_then(|body| body.find("*/"))
                .map_or((Kind::BlockComment, rest.len(), Some("comment")), |end| {
                    (Kind::BlockComment, end + 4, None)
                }),
            ('/', _) if self.previous.regex_allowed() => match regex_len(rest) {
                End::Closed(len) => (Kind::Regex, len, None),
                End::Eof => (Kind::Regex, rest.len(), Some("regular expression")),
                End::Newline(_) => {
                    return Err(format!("unterminated regular expression at byte {offset}"));
                }
            },
            ('"' | '\'', _) => match string_len(rest) {
                End::Closed(len) => (Kind::String, len, None),
                End::Eof => (Kind::String, rest.len(), Some("string")),
                End::Newline(_) => return Err(format!("unterminated string at byte {offset}")),
            },
            ('`', _) => match template_len(rest) {
                End::Closed(len) => (Kind::Template, len, None),
                _ => (Kind::Template, rest.len(), Some("template literal")),
            },
            ('0'..='9', _) | ('.', Some('0'..='9')) => (Kind::Number, number_len(rest), None),
            (c, _) if is_word_start(c) => (Kind::Word, prefix_len(rest, is_word_part), None),
            _ => punctuator_len(rest).map_or_else(
                || (Kind::Other, first.len_utf8(), None),
                |len| (Kind::Punct, len, None),
            ),
        };
        Ok(token)
    }

    /// Updates bracket and `/` state after a token.
    fn advance(&mut self, kind: Kind, piece: &str, offset: usize) -> Result<(), String> {
        self.previous = match kind {
            Kind::Space | Kind::LineComment | Kind::BlockComment => return Ok(()),
            Kind::Word => self.previous.after_word(piece),
            Kind::Punct => self.punct(piece, offset)?,
            Kind::String | Kind::Template | Kind::Regex | Kind::Number | Kind::Other => {
                Previous::Value
            }
        };
        Ok(())
    }

    fn punct(&mut self, piece: &str, offset: usize) -> Result<Previous, String> {
        let previous = match piece {
            "(" | "[" | "{" => {
                let regex_after_close = match piece {
                    "(" => self.previous == Previous::Condition,
                    "{" => self.previous.block_allowed(),
                    _ => false,
                };
                self.opens.push(Open {
                    bracket: piece.chars().next().unwrap_or_default(),
                    regex_after_close,
                });
                if piece == "{" {
                    Previous::Statement
                } else {
                    Previous::Operator
                }
            }
            ")" | "]" | "}" => {
                let expected = match piece {
                    ")" => '(',
                    "]" => '[',
                    _ => '{',
                };
                match self.opens.pop() {
                    Some(open) if open.bracket == expected => Previous::after_close(open),
                    _ => return Err(format!("unbalanced {piece:?} at byte {offset}")),
                }
            }
            "++" | "--" => Previous::Value,
            ";" | "=>" => Previous::Statement,
            "." | "?." => Previous::Dot,
            _ => Previous::Operator,
        };
        Ok(previous)
    }
}

const fn is_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

const fn is_line_end(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || matches!(c, '_' | '$' | '\\' | '#')
}

fn is_word_part(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '$' | '\\' | '\u{200c}' | '\u{200d}')
}

fn prefix_len(text: &str, accept: impl Fn(char) -> bool) -> usize {
    text.char_indices()
        .find(|(index, c)| *index > 0 && !accept(*c))
        .map_or(text.len(), |(index, _)| index)
}

/// Splits a line comment (or hashbang) from the line terminator after it.
fn split_line(text: &str) -> (&str, &str) {
    text.split_at(text.find(is_line_end).unwrap_or(text.len()))
}

fn punctuator_len(text: &str) -> Option<usize> {
    if text.starts_with("?.") && text.as_bytes().get(2).is_some_and(u8::is_ascii_digit) {
        // `a?.5:b` is a conditional, not optional chaining.
        return Some(1);
    }
    PUNCTUATORS
        .iter()
        .find(|punctuator| text.starts_with(**punctuator))
        .map(|punctuator| punctuator.len())
        .or_else(|| text.starts_with(SINGLE_PUNCTUATORS.as_slice()).then_some(1))
}

/// Length of a numeric literal: digits, letters (hex digits, exponents,
/// `n`), separators, a decimal point, and a sign right after an exponent.
fn number_len(text: &str) -> usize {
    let hex = text.len() > 1
        && text
            .get(..2)
            .is_some_and(|start| start.eq_ignore_ascii_case("0x"));
    let mut previous = '\0';
    for (index, c) in text.char_indices() {
        let exponent_sign = matches!(c, '+' | '-') && matches!(previous, 'e' | 'E') && !hex;
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || exponent_sign) {
            return index;
        }
        previous = c;
    }
    text.len()
}

/// Scans a quoted string starting at its opening quote.
fn string_len(text: &str) -> End {
    let quote = text.chars().next().unwrap_or_default();
    let mut escaped = false;
    for (index, c) in text.char_indices().skip(1) {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == quote {
            return End::Closed(index + 1);
        } else if matches!(c, '\n' | '\r') {
            return End::Newline(index);
        }
    }
    End::Eof
}

/// Scans a regular expression literal and its flags from the opening `/`.
fn regex_len(text: &str) -> End {
    let mut escaped = false;
    let mut in_class = false;
    for (index, c) in text.char_indices().skip(1) {
        if is_line_end(c) {
            return End::Newline(index);
        }
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '[' {
            in_class = true;
        } else if c == ']' {
            in_class = false;
        } else if c == '/' && !in_class {
            let flags = text
                .get(index + 1..)
                .map_or(0, |after| prefix_len_from_start(after, is_word_part));
            return End::Closed(index + 1 + flags);
        }
    }
    End::Eof
}

fn prefix_len_from_start(text: &str, accept: impl Fn(char) -> bool) -> usize {
    text.find(|c| !accept(c)).unwrap_or(text.len())
}

/// Scans a template literal from its opening backtick, skipping `${…}`
/// substitutions with their nested strings, templates, and comments.
fn template_len(text: &str) -> End {
    let mut index = 1;
    while let Some(c) = text.get(index..).and_then(|rest| rest.chars().next()) {
        match c {
            '\\' => {
                index += 1;
                index += text
                    .get(index..)
                    .and_then(|rest| rest.chars().next())
                    .map_or(0, char::len_utf8);
            }
            '`' => return End::Closed(index + 1),
            '$' if text
                .get(index + 1..)
                .is_some_and(|rest| rest.starts_with('{')) =>
            {
                match substitution_len(text.get(index + 2..).unwrap_or_default()) {
                    Some(len) => index += 2 + len,
                    None => return End::Eof,
                }
            }
            _ => index += c.len_utf8(),
        }
    }
    End::Eof
}

/// Length of a template substitution body including its closing `}`, or
/// `None` when the text ends first.
fn substitution_len(text: &str) -> Option<usize> {
    let mut depth = 1_usize;
    let mut index = 0;
    while let Some(c) = text.get(index..).and_then(|rest| rest.chars().next()) {
        let rest = text.get(index..).unwrap_or_default();
        let len = match c {
            '{' => {
                depth += 1;
                1
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index + 1);
                }
                1
            }
            '"' | '\'' => match string_len(rest) {
                End::Closed(len) | End::Newline(len) => len,
                End::Eof => return None,
            },
            '`' => match template_len(rest) {
                End::Closed(len) => len,
                _ => return None,
            },
            '/' if rest.starts_with("//") => split_line(rest).0.len(),
            '/' if rest.starts_with("/*") => rest.get(2..)?.find("*/")? + 4,
            _ => c.len_utf8(),
        };
        index += len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str) -> Result<Vec<(Kind, &str)>, String> {
        Ok(lex(text)?
            .tokens
            .into_iter()
            .filter(|token| token.kind != Kind::Space)
            .map(|token| (token.kind, token.text))
            .collect())
    }

    fn kind_of(text: &str, piece: &str) -> Result<Vec<Kind>, String> {
        Ok(tokens(text)?
            .into_iter()
            .filter(|(_, text)| *text == piece)
            .map(|(kind, _)| kind)
            .collect())
    }

    #[test]
    fn tokens_reassemble_the_original_text() -> Result<(), String> {
        let text = "#!/usr/bin/env node\nconst a = `x${ {b:'}'}.b }y` /* c */ + 0x1F + 1e-3 + .5n; // end\n";
        let lexed = lex(text)?;
        let joined: String = lexed.tokens.iter().map(|token| token.text).collect();
        assert_eq!(joined, text);
        assert_eq!(lexed.unfinished, None);
        assert_eq!(
            tokens(text)?,
            [
                (Kind::LineComment, "#!/usr/bin/env node"),
                (Kind::Word, "const"),
                (Kind::Word, "a"),
                (Kind::Punct, "="),
                (Kind::Template, "`x${ {b:'}'}.b }y`"),
                (Kind::BlockComment, "/* c */"),
                (Kind::Punct, "+"),
                (Kind::Number, "0x1F"),
                (Kind::Punct, "+"),
                (Kind::Number, "1e-3"),
                (Kind::Punct, "+"),
                (Kind::Number, ".5n"),
                (Kind::Punct, ";"),
                (Kind::LineComment, "// end"),
            ]
        );
        Ok(())
    }

    #[test]
    fn tells_regular_expressions_from_division() -> Result<(), String> {
        assert_eq!(kind_of("a = b / c / d", "/")?, [Kind::Punct, Kind::Punct]);
        assert_eq!(kind_of("x = /[/]{}/g.test(s)", "/[/]{}/g")?, [Kind::Regex]);
        assert_eq!(kind_of("return /a;b/.source", "/a;b/")?, [Kind::Regex]);
        assert_eq!(kind_of("if (x) /}/.test(y)", "/}/")?, [Kind::Regex]);
        assert_eq!(kind_of("f(x) / 2", "/")?, [Kind::Punct]);
        assert_eq!(kind_of("a[0] / 2", "/")?, [Kind::Punct]);
        assert_eq!(kind_of("i++ / 2", "/")?, [Kind::Punct]);
        assert_eq!(kind_of("x /= 2", "/=")?, [Kind::Punct]);
        assert_eq!(kind_of("{}\n/=a/.test(s)", "/=a/")?, [Kind::Regex]);
        assert_eq!(kind_of("x = {} / 2", "/")?, [Kind::Punct]);
        assert_eq!(kind_of("f(`${a}` / 2)", "/")?, [Kind::Punct]);
        assert_eq!(
            kind_of("a.return / b.if / 2", "/")?,
            [Kind::Punct, Kind::Punct]
        );
        Ok(())
    }

    #[test]
    fn keeps_strings_templates_and_comments_whole() -> Result<(), String> {
        assert_eq!(
            tokens(r#"s = "a\"b;{" + 'c,}'"#)?,
            [
                (Kind::Word, "s"),
                (Kind::Punct, "="),
                (Kind::String, r#""a\"b;{""#),
                (Kind::Punct, "+"),
                (Kind::String, "'c,}'"),
            ]
        );
        assert_eq!(
            kind_of("t = `a ${`b ${c}`} \\` ; {`", "`a ${`b ${c}`} \\` ; {`")?,
            [Kind::Template]
        );
        assert_eq!(kind_of("a /* ; { */ b", "/* ; { */")?, [Kind::BlockComment]);
        assert_eq!(kind_of("a?.5:b", "?")?, [Kind::Punct]);
        assert_eq!(kind_of("a?.b", "?.")?, [Kind::Punct]);
        Ok(())
    }

    #[test]
    fn reports_what_a_truncated_script_left_unfinished() -> Result<(), String> {
        for (text, unfinished) in [
            ("f(\"abc", "string"),
            ("x = `a ${b", "template literal"),
            ("/* note", "comment"),
            ("x = /ab", "regular expression"),
            ("function f() { if (a) {", "block or object"),
            ("call(a, [1", "array or index"),
            ("f(a", "parenthesis"),
        ] {
            let lexed = lex(text)?;
            let joined: String = lexed.tokens.iter().map(|token| token.text).collect();
            assert_eq!(joined, text);
            assert_eq!(lexed.unfinished, Some(unfinished), "{text}");
        }
        Ok(())
    }

    #[test]
    fn rejects_text_that_needs_a_guess() {
        assert_eq!(
            lex("a = 'broken\nline'"),
            Err("unterminated string at byte 4".to_owned())
        );
        assert_eq!(
            lex("x = /a\n/"),
            Err("unterminated regular expression at byte 4".to_owned())
        );
        assert_eq!(lex("f(]"), Err("unbalanced \"]\" at byte 2".to_owned()));
        assert_eq!(lex("}"), Err("unbalanced \"}\" at byte 0".to_owned()));
    }
}
