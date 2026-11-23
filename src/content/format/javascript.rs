//! JavaScript pretty printer: breaks lines after `{`, `;`, and `,` and before
//! `}`, and indents by brace depth.
//!
//! Only whitespace between tokens changes. Tokens (strings, template
//! literals, regular expressions, comments) are copied unchanged, line ends
//! already in the source are kept so automatic semicolon insertion reads the
//! same, and new line ends are added only where they cannot change how the
//! script parses. The script is never evaluated.

mod lexer;

use super::{Formatted, Notice, Styled, TokenKind, comment_align, starts_with_blank_line};
use lexer::{Kind, Token};

/// Words shown as keywords.
const KEYWORDS: [&str; 41] = [
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "from",
    "function",
    "get",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "of",
    "return",
    "set",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];
/// Words shown as literal values.
const LITERALS: [&str; 6] = ["true", "false", "null", "undefined", "NaN", "Infinity"];
/// Words that continue the statement of a preceding `}` on the same line.
const CONTINUATIONS: [&str; 7] = [
    "else",
    "catch",
    "finally",
    "while",
    "instanceof",
    "in",
    "of",
];
/// Longest source map URL shown in the notice.
const SOURCE_MAP_URL_CHARS: usize = 120;

/// Formats `text`, or fails when it cannot be tokenized without guessing.
/// Notices say what the script leaves unfinished where the preview was cut
/// and which source map it points to; the source map is never fetched.
pub(super) fn pretty(text: &str) -> Result<Formatted, String> {
    let lexed = lexer::lex(text)?;
    let unfinished = lexed.unfinished.map(|unfinished| {
        Notice::warning(format!(
            "The script preview is incomplete: it ends inside an unfinished {unfinished}. Formatted up to where it ends."
        ))
    });
    let source_map = source_map_url(&lexed.tokens)
        .map(|url| Notice::info(format!("Source map: {url} (not fetched).")));
    Ok(Formatted {
        styled: Printer::default().print(&lexed.tokens),
        notices: unfinished.into_iter().chain(source_map).collect(),
    })
}

#[derive(Default)]
struct Printer {
    out: Styled,
    /// Open brackets, innermost last.
    opens: Vec<char>,
}

/// Where a line break goes between two tokens.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Gap {
    None,
    Space,
    Line,
    BlankLine,
}

impl Printer {
    fn print(mut self, tokens: &[Token<'_>]) -> Styled {
        let mut previous: Option<Token<'_>> = None;
        let mut source_gap = Gap::None;
        for token in tokens {
            if token.kind == Kind::Space {
                source_gap = source_gap.max(space_gap(token.text));
                continue;
            }
            let gap = previous.map_or(Gap::None, |previous| {
                source_gap.max(self.added_gap(previous, *token))
            });
            self.write_gap(gap, *token);
            self.emit(*token, previous);
            self.track_brackets(*token);
            previous = Some(*token);
            source_gap = Gap::None;
        }
        self.out
    }

    fn emit(&mut self, token: Token<'_>, previous: Option<Token<'_>>) {
        let token_kind = kind(token, previous);
        if !token.text.contains('\n') {
            self.out.push(token.text, token_kind);
            return;
        }
        match token.kind {
            Kind::Template => {
                let after = token.text.strip_prefix('`').unwrap_or(token.text);
                if starts_with_blank_line(after) {
                    let depth = self.opens.iter().filter(|open| **open == '{').count();
                    self.out
                        .push_multiline(token.text, token_kind, &"  ".repeat(depth + 1));
                } else {
                    self.out.push_quoted_multiline(token.text, token_kind);
                }
            }
            Kind::String => self.out.push_quoted_multiline(token.text, token_kind),
            Kind::BlockComment => {
                let align = comment_align(&self.out.visual_current_line(), token.text, "/*");
                self.out.push_multiline(token.text, token_kind, &align);
            }
            _ => self.out.push(token.text, token_kind),
        }
    }

    /// A break the printer adds where the source had none.
    fn added_gap(&self, previous: Token<'_>, next: Token<'_>) -> Gap {
        let in_braces = matches!(self.opens.last(), None | Some('{'));
        let break_after = match (previous.kind, previous.text) {
            (Kind::Punct, "{") => next.text != "}",
            (Kind::Punct, ";" | ",") => in_braces,
            (Kind::Punct, "}") => {
                in_braces
                    && match next.kind {
                        Kind::Word => !CONTINUATIONS.contains(&next.text),
                        Kind::String | Kind::Template | Kind::Number | Kind::Regex => true,
                        _ => false,
                    }
            }
            _ => false,
        };
        let break_before = next.kind == Kind::Punct && next.text == "}" && previous.text != "{";
        if break_after || break_before {
            Gap::Line
        } else {
            Gap::None
        }
    }

    fn write_gap(&mut self, gap: Gap, next: Token<'_>) {
        match gap {
            Gap::None => {}
            Gap::Space => self.out.push(" ", TokenKind::Text),
            Gap::Line | Gap::BlankLine => {
                if gap == Gap::BlankLine {
                    self.out.push("\n", TokenKind::Text);
                }
                let depth = self.opens.iter().filter(|open| **open == '{').count();
                let closing = next.kind == Kind::Punct && next.text == "}";
                self.out.newline(depth.saturating_sub(usize::from(closing)));
            }
        }
    }

    fn track_brackets(&mut self, token: Token<'_>) {
        if token.kind != Kind::Punct {
            return;
        }
        match token.text {
            "{" => self.opens.push('{'),
            "(" => self.opens.push('('),
            "[" => self.opens.push('['),
            // The lexer has already checked that brackets match.
            ")" | "]" | "}" => {
                self.opens.pop();
            }
            _ => {}
        }
    }
}

/// The break that whitespace from the source asks for: line ends are kept
/// (at most one blank line), other whitespace becomes one space.
fn space_gap(space: &str) -> Gap {
    let line_ends = space
        .chars()
        .filter(|c| matches!(c, '\n' | '\u{2028}' | '\u{2029}'))
        .count()
        + usize::from(space.contains('\r') && !space.contains('\n'));
    match line_ends {
        0 => Gap::Space,
        1 => Gap::Line,
        _ => Gap::BlankLine,
    }
}

/// How a token is highlighted, given the significant token before it.
fn kind(token: Token<'_>, previous: Option<Token<'_>>) -> TokenKind {
    match token.kind {
        Kind::Space | Kind::Other => TokenKind::Text,
        Kind::LineComment | Kind::BlockComment => TokenKind::Comment,
        Kind::String | Kind::Template => TokenKind::String,
        Kind::Regex => TokenKind::Literal,
        Kind::Number => TokenKind::Number,
        Kind::Punct => TokenKind::Punctuation,
        Kind::Word => {
            let property = previous.is_some_and(|previous| matches!(previous.text, "." | "?."));
            if property {
                TokenKind::Text
            } else if LITERALS.contains(&token.text) {
                TokenKind::Literal
            } else if KEYWORDS.contains(&token.text) {
                TokenKind::Key
            } else {
                TokenKind::Text
            }
        }
    }
}

/// The URL of the last `sourceMappingURL` comment, shortened for display.
fn source_map_url(tokens: &[Token<'_>]) -> Option<String> {
    let url = tokens.iter().rev().find_map(|token| {
        let body = match token.kind {
            Kind::LineComment => token.text.strip_prefix("//")?,
            Kind::BlockComment => token.text.strip_prefix("/*")?.strip_suffix("*/")?,
            _ => return None,
        };
        let url = body
            .strip_prefix("# sourceMappingURL=")
            .or_else(|| body.strip_prefix("@ sourceMappingURL="))?;
        Some(url.trim())
    })?;
    if url.starts_with("data:") {
        return Some("inline data: URL".to_owned());
    }
    let mut shown: String = url.chars().take(SOURCE_MAP_URL_CHARS).collect();
    if shown.len() < url.len() {
        shown.push('…');
    }
    Some(shown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::format::NoticeLevel;

    fn pieces(styled: &Styled, kind: TokenKind) -> Vec<&str> {
        styled
            .pieces()
            .filter(|(_, piece_kind)| *piece_kind == kind)
            .map(|(text, _)| text)
            .collect()
    }

    fn formatted(text: &str) -> Result<String, String> {
        Ok(pretty(text)?.styled.text().to_owned())
    }

    /// Removes whitespace outside tokens, which formatting may change.
    fn significant(text: &str) -> Result<Vec<String>, String> {
        Ok(lexer::lex(text)?
            .tokens
            .into_iter()
            .filter(|token| token.kind != Kind::Space)
            .map(|token| token.text.to_owned())
            .collect())
    }

    #[test]
    fn indents_minified_script() -> Result<(), String> {
        assert_eq!(
            formatted(
                "!function(e){var t={a:1,b:[1,2]};for(var n=0;n<3;n++){if(n)t.a++;else{e(n)}}try{e()}catch(r){}return t}(window);"
            )?,
            "!function(e){\n  var t={\n    a:1,\n    b:[1,2]\n  };\n  for(var n=0;n<3;n++){\n    if(n)t.a++;\n    else{\n      e(n)\n    }\n  }\n  try{\n    e()\n  }catch(r){}\n  return t\n}(window);"
        );
        Ok(())
    }

    #[test]
    fn keeps_source_line_ends_and_spaces() -> Result<(), String> {
        assert_eq!(
            formatted("function f() {\n\n\n  return\n  x\n}\na++\n/re/g.test(s)")?,
            "function f() {\n\n  return\n  x\n}\na++\n/re/g.test(s)"
        );
        assert_eq!(formatted("let  a =\t1")?, "let a = 1");
        Ok(())
    }

    #[test]
    fn never_changes_token_text() -> Result<(), String> {
        let text = r#"var r=/[;{,]}/g,s="a;{b,}c",t=`x;${{k:"}"}.k},{y`;/* a;{,} */x=a/b/c;"#;
        let output = formatted(text)?;
        assert_eq!(significant(&output)?, significant(text)?);
        assert!(output.contains("/[;{,]}/g"));
        assert!(output.contains("`x;${{k:\"}\"}.k},{y`"));
        assert!(output.contains("/* a;{,} */"));
        Ok(())
    }

    #[test]
    fn formats_truncated_script_and_says_so() -> Result<(), String> {
        let result = pretty("(function(){var a={b:\"unfinish")?;
        assert_eq!(
            result.styled.text(),
            "(function(){\n  var a={\n    b:\"unfinish"
        );
        assert!(matches!(
            result.notices.as_slice(),
            [Notice { level: NoticeLevel::Warning, text }] if text.contains("string")
        ));
        let result = pretty("f(function(){a;b")?;
        assert_eq!(result.styled.text(), "f(function(){\n  a;\n  b");
        assert!(matches!(
            result.notices.as_slice(),
            [Notice { level: NoticeLevel::Warning, text }] if text.contains("block")
        ));
        assert_eq!(pretty("a=1;")?.notices, []);
        Ok(())
    }

    #[test]
    fn shows_the_source_map_url_without_fetching_it() -> Result<(), String> {
        let result = pretty("a();\n//# sourceMappingURL=app.min.js.map")?;
        assert_eq!(
            result.notices,
            [Notice::info("Source map: app.min.js.map (not fetched).")]
        );
        assert!(
            pieces(&result.styled, TokenKind::Comment)
                .contains(&"//# sourceMappingURL=app.min.js.map")
        );
        let inline = pretty("a()\n/*# sourceMappingURL=data:application/json;base64,e30= */")?;
        assert_eq!(
            inline.notices,
            [Notice::info("Source map: inline data: URL (not fetched).")]
        );
        Ok(())
    }

    #[test]
    fn classifies_tokens_for_highlighting() -> Result<(), String> {
        let styled = pretty("const n=a.return+1.5;if(x)return /a/g.test(`t`)?'s':null// c")?.styled;
        assert_eq!(pieces(&styled, TokenKind::Key), ["const", "if", "return"]);
        assert_eq!(pieces(&styled, TokenKind::Number), ["1.5"]);
        assert_eq!(pieces(&styled, TokenKind::Literal), ["/a/g", "null"]);
        assert_eq!(pieces(&styled, TokenKind::String), ["`t`", "'s'"]);
        assert_eq!(pieces(&styled, TokenKind::Comment), ["// c"]);
        assert!(pieces(&styled, TokenKind::Text).contains(&"return"));
        assert!(pieces(&styled, TokenKind::Punctuation).contains(&")?"));
        Ok(())
    }

    #[test]
    fn falls_back_when_tokens_need_a_guess() {
        assert!(pretty("a = 'line\nbreak'").is_err());
        assert!(pretty("<html>}</html>").is_err());
    }

    #[test]
    fn visually_indents_multiline_templates_and_block_comments_without_altering_text()
    -> Result<(), String> {
        let js = "const data={nested:{msg:`line1\nline2\nline3`,next:1}};/* a\nb */";
        let out = pretty(js)?;
        let text = out.styled.text();
        assert!(text.contains("`line1\nline2\nline3`"), "{text}");
        assert!(text.contains("/* a\nb */"), "{text}");
        assert_eq!(significant(text)?, significant(js)?);

        let indents = out.styled.indents();
        let align_for = |needle: &str| {
            let pos = text.find(needle)?;
            indents
                .iter()
                .find(|indent| indent.offset == pos)
                .map(|indent| indent.align_to.as_str())
        };
        assert_eq!(align_for("line2"), Some("    msg:`"));
        assert_eq!(align_for("line3"), Some("    msg:`"));
        assert_eq!(align_for("b */"), Some("/* "));
        assert_eq!(align_for("next:1"), None);
        Ok(())
    }
}
