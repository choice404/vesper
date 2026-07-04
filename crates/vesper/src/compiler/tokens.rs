//! Semantic tokens for highlighting.
//!
//! The dusk lexer already hands back every token with a span, so highlighting
//! rides the token stream rather than the tree. Roles the stream cannot settle
//! on its own, a name after `func` or a type after `->`, come from a small
//! amount of look around, not from the parser, since the syntax tree carries no
//! span for a type or a parameter name.
//!
//! Comments are the one thing the lexer drops, folding them into trivia, so they
//! are recovered by scanning the gaps between token spans for `//`.

use tower_lsp::lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend,
};

use dusk::lexer::lex;
use dusk::lexer::token::{Keyword, Token, TokenKind};

use super::builtins::is_primitive;
use crate::position::LineIndex;

/// The highlighting roles vesper emits. The order here is the legend order, so
/// [`type_index`] and [`legend`] must stay in step.
#[derive(Clone, Copy, PartialEq)]
enum Sem {
    Keyword,
    Function,
    Type,
    Struct,
    Enum,
    Interface,
    Number,
    String,
    Comment,
    Variable,
    Macro,
}

/// The index a role takes in the legend the client was handed.
fn type_index(sem: Sem) -> u32 {
    match sem {
        Sem::Keyword => 0,
        Sem::Function => 1,
        Sem::Type => 2,
        Sem::Struct => 3,
        Sem::Enum => 4,
        Sem::Interface => 5,
        Sem::Number => 6,
        Sem::String => 7,
        Sem::Comment => 8,
        Sem::Variable => 9,
        Sem::Macro => 10,
    }
}

/// The `declaration` and `defaultLibrary` modifier bits, matching the legend.
const DECL: u32 = 1;
const DEFLIB: u32 = 2;

/// The legend the server advertises, naming every role and modifier it will use.
/// The token type order matches [`type_index`].
pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::KEYWORD,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::TYPE,
            SemanticTokenType::STRUCT,
            SemanticTokenType::ENUM,
            SemanticTokenType::INTERFACE,
            SemanticTokenType::NUMBER,
            SemanticTokenType::STRING,
            SemanticTokenType::COMMENT,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::MACRO,
        ],
        token_modifiers: vec![
            SemanticTokenModifier::DECLARATION,
            SemanticTokenModifier::DEFAULT_LIBRARY,
        ],
    }
}

/// One classified span, in byte offsets, before it is converted to the protocol
/// line, column, and length shape.
struct Raw {
    lo: u32,
    hi: u32,
    sem: Sem,
    mods: u32,
}

/// Builds the full document semantic tokens for `text`. The lexer recovers past
/// errors, so this still highlights a buffer that does not fully parse.
pub fn semantic_tokens(text: &str) -> Vec<SemanticToken> {
    let (tokens, _errs) = lex(text);
    let mut raws = classify(&tokens, text);
    raws.extend(comments(&tokens, text));
    encode(text, raws)
}

/// Classifies each token into a role, using light look around for the cases the
/// token alone cannot settle.
fn classify(tokens: &[Token], _text: &str) -> Vec<Raw> {
    let mut raws = Vec::new();
    // A name right after one of these keywords is that kind's declaration.
    let mut decl_pending: Option<Sem> = None;
    // A name right after `@` is a directive name.
    let mut at_pending = false;
    // Inside a type position, a return type after `->` or the head of an `impl`,
    // a bare name is a type. It ends at the `{` that opens the body, at a newline,
    // or at the comma or paren that closes the position it opened in.
    let mut type_mode = false;
    // The paren and bracket nesting depth where the current type position began,
    // and the angle bracket nesting inside it. A comma at the opening depth with
    // no open generic ends the type, but a comma deeper in, inside a tuple type's
    // parens or a generic's angle brackets, does not.
    let mut depth: i32 = 0;
    let mut type_depth: i32 = 0;
    let mut angle: i32 = 0;

    let clear_pending = |decl: &mut Option<Sem>, at: &mut bool| {
        *decl = None;
        *at = false;
    };

    for (i, tok) in tokens.iter().enumerate() {
        if tok.nl_before {
            type_mode = false;
        }
        match &tok.kind {
            TokenKind::Kw(kw) => {
                raws.push(raw(tok, Sem::Keyword, 0));
                decl_pending = match kw {
                    Keyword::Func => Some(Sem::Function),
                    Keyword::Struct => Some(Sem::Struct),
                    Keyword::Enum => Some(Sem::Enum),
                    Keyword::Interface => Some(Sem::Interface),
                    // A monad block names a type like construct after the keyword.
                    Keyword::Monad => Some(Sem::Type),
                    _ => None,
                };
                if matches!(kw, Keyword::Impl) {
                    type_mode = true;
                    type_depth = depth;
                    angle = 0;
                }
                at_pending = false;
            }
            TokenKind::Bool(_) => {
                raws.push(raw(tok, Sem::Keyword, 0));
                clear_pending(&mut decl_pending, &mut at_pending);
            }
            TokenKind::Int { .. } | TokenKind::Float { .. } => {
                raws.push(raw(tok, Sem::Number, 0));
                clear_pending(&mut decl_pending, &mut at_pending);
            }
            TokenKind::Str(_) | TokenKind::Char(_) => {
                raws.push(raw(tok, Sem::String, 0));
                clear_pending(&mut decl_pending, &mut at_pending);
            }
            TokenKind::Ident(name) => {
                let (sem, mods) = if at_pending {
                    (Sem::Macro, 0)
                } else if let Some(kind) = decl_pending {
                    (kind, DECL)
                } else if is_primitive(name) {
                    (Sem::Type, DEFLIB)
                } else if type_mode {
                    (Sem::Type, 0)
                } else if next_is_call(tokens, i) {
                    (Sem::Function, 0)
                } else {
                    (Sem::Variable, 0)
                };
                raws.push(raw(tok, sem, mods));
                clear_pending(&mut decl_pending, &mut at_pending);
            }
            TokenKind::At => {
                // The `@` sigil is not colored on its own; the name after it is.
                at_pending = true;
            }
            TokenKind::Arrow => {
                type_mode = true;
                type_depth = depth;
                angle = 0;
                clear_pending(&mut decl_pending, &mut at_pending);
            }
            TokenKind::LParen | TokenKind::LBracket => {
                depth += 1;
                clear_pending(&mut decl_pending, &mut at_pending);
            }
            TokenKind::RParen | TokenKind::RBracket => {
                depth -= 1;
                // The position that opened the type has closed.
                if type_mode && depth < type_depth {
                    type_mode = false;
                }
                clear_pending(&mut decl_pending, &mut at_pending);
            }
            TokenKind::Lt => {
                if type_mode {
                    angle += 1;
                }
                clear_pending(&mut decl_pending, &mut at_pending);
            }
            TokenKind::Gt => {
                if type_mode && angle > 0 {
                    angle -= 1;
                }
                clear_pending(&mut decl_pending, &mut at_pending);
            }
            TokenKind::Shr => {
                if type_mode {
                    angle = (angle - 2).max(0);
                }
                clear_pending(&mut decl_pending, &mut at_pending);
            }
            TokenKind::Comma => {
                // A comma at the depth the type opened, outside any generic, ends
                // it. One inside a tuple type's parens or a generic's angles does
                // not, so those type arguments stay types.
                if type_mode && depth == type_depth && angle == 0 {
                    type_mode = false;
                }
                clear_pending(&mut decl_pending, &mut at_pending);
            }
            TokenKind::LBrace => {
                type_mode = false;
                clear_pending(&mut decl_pending, &mut at_pending);
            }
            _ => {
                clear_pending(&mut decl_pending, &mut at_pending);
            }
        }
    }
    raws
}

/// True when the token at `i` is immediately followed by `(`, the shape of a
/// call or a declaration's parameter list.
fn next_is_call(tokens: &[Token], i: usize) -> bool {
    matches!(tokens.get(i + 1).map(|t| &t.kind), Some(TokenKind::LParen))
}

/// Recovers comment spans by scanning the gaps between token spans for `//`. A
/// `//` inside a string or char literal sits within that token's span, never in
/// a gap, so this never mistakes it for a comment.
fn comments(tokens: &[Token], text: &str) -> Vec<Raw> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    for tok in tokens {
        let lo = tok.span.lo as usize;
        if lo > cursor {
            scan_gap(text, cursor, lo, &mut out);
        }
        cursor = cursor.max(tok.span.hi as usize);
    }
    if cursor < text.len() {
        scan_gap(text, cursor, text.len(), &mut out);
    }
    out
}

/// Emits a comment span for each `//` found in `text[lo..hi]`, each running to
/// the end of its line.
fn scan_gap(text: &str, lo: usize, hi: usize, out: &mut Vec<Raw>) {
    let bytes = text.as_bytes();
    let mut p = lo;
    while p + 1 < hi {
        if bytes[p] == b'/' && bytes[p + 1] == b'/' {
            let mut end = p;
            while end < text.len() && bytes[end] != b'\n' && bytes[end] != b'\r' {
                end += 1;
            }
            out.push(Raw {
                lo: p as u32,
                hi: end as u32,
                sem: Sem::Comment,
                mods: 0,
            });
            p = end + 1;
        } else {
            p += 1;
        }
    }
}

/// Builds a raw span from a token.
fn raw(tok: &Token, sem: Sem, mods: u32) -> Raw {
    Raw {
        lo: tok.span.lo,
        hi: tok.span.hi,
        sem,
        mods,
    }
}

/// Converts classified byte spans into the protocol's delta encoded tokens,
/// sorted by position, with lengths counted in UTF-16 code units. A span that
/// somehow crosses a line is dropped rather than emitted malformed.
fn encode(text: &str, raws: Vec<Raw>) -> Vec<SemanticToken> {
    let index = LineIndex::new(text);
    let mut items: Vec<(u32, u32, u32, u32, u32)> = Vec::with_capacity(raws.len());
    for r in &raws {
        let start = index.position(text, r.lo);
        let end = index.position(text, r.hi);
        if end.line != start.line {
            continue;
        }
        let len = end.character.saturating_sub(start.character);
        if len == 0 {
            continue;
        }
        items.push((start.line, start.character, len, type_index(r.sem), r.mods));
    }
    items.sort_by_key(|it| (it.0, it.1));

    let mut out = Vec::with_capacity(items.len());
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;
    for (line, start, len, ty, mods) in items {
        let delta_line = line - prev_line;
        let delta_start = if delta_line == 0 {
            start - prev_start
        } else {
            start
        };
        out.push(SemanticToken {
            delta_line,
            delta_start,
            length: len,
            token_type: ty,
            token_modifiers_bitset: mods,
        });
        prev_line = line;
        prev_start = start;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rebuilds absolute (line, char, len, type, mods) tuples from the delta
    /// encoded stream, so a test can look for a role at a position.
    fn decoded(text: &str) -> Vec<(u32, u32, u32, u32, u32)> {
        let mut out = Vec::new();
        let (mut line, mut start) = (0u32, 0u32);
        for t in semantic_tokens(text) {
            if t.delta_line != 0 {
                line += t.delta_line;
                start = t.delta_start;
            } else {
                start += t.delta_start;
            }
            out.push((
                line,
                start,
                t.length,
                t.token_type,
                t.token_modifiers_bitset,
            ));
        }
        out
    }

    #[test]
    fn colors_a_function_declaration() {
        let text = "func main() -> int32 {\n    return 0\n}\n";
        let d = decoded(text);
        // `func` is a keyword at the start of line 0.
        assert!(d.iter().any(|t| t.0 == 0 && t.1 == 0 && t.3 == type_index(Sem::Keyword)));
        // `main` is a function declaration at column 5.
        assert!(d
            .iter()
            .any(|t| t.0 == 0 && t.1 == 5 && t.3 == type_index(Sem::Function) && t.4 & DECL != 0));
        // `int32` is a builtin type.
        assert!(d
            .iter()
            .any(|t| t.3 == type_index(Sem::Type) && t.4 & DEFLIB != 0));
    }

    #[test]
    fn colors_a_call_as_a_function() {
        let text = "func main() -> int32 {\n    work()\n    return 0\n}\n";
        let d = decoded(text);
        // `work` on line 1 is followed by `(`, so it is a function.
        assert!(d
            .iter()
            .any(|t| t.0 == 1 && t.3 == type_index(Sem::Function) && t.4 == 0));
    }

    #[test]
    fn colors_a_directive_name_as_a_macro() {
        let text = "@paradigm procedural\nfunc main() -> int32 {\n    return 0\n}\n";
        let d = decoded(text);
        assert!(d.iter().any(|t| t.0 == 0 && t.3 == type_index(Sem::Macro)));
    }

    #[test]
    fn colors_a_comment() {
        let text = "func f() -> void {\n    // hi\n}\n";
        let d = decoded(text);
        assert!(d.iter().any(|t| t.3 == type_index(Sem::Comment)));
    }

    /// Pairs each token with the source text it covers, for tests that look a
    /// role up by name. The inputs here are ASCII, so a character column equals a
    /// UTF-16 unit.
    fn roled(text: &str) -> Vec<(u32, String)> {
        let lines: Vec<&str> = text.split('\n').collect();
        let mut out = Vec::new();
        let (mut line, mut start) = (0u32, 0u32);
        for t in semantic_tokens(text) {
            if t.delta_line != 0 {
                line += t.delta_line;
                start = t.delta_start;
            } else {
                start += t.delta_start;
            }
            let slice: String = lines[line as usize]
                .chars()
                .skip(start as usize)
                .take(t.length as usize)
                .collect();
            out.push((t.token_type, slice));
        }
        out
    }

    #[test]
    fn a_parameter_after_a_function_type_parameter_is_not_a_type() {
        // The inner `->` opens a type, but the comma that ends the first parameter
        // must close it, so the second parameter name stays a variable.
        let text = "func apply(f: (int32) -> int32, x: int32) -> void {\n}\n";
        let toks = roled(text);
        let x = toks.iter().find(|(_, s)| s == "x").expect("x is present");
        assert_eq!(x.0, type_index(Sem::Variable), "x is a parameter, not a type");
    }

    #[test]
    fn a_generic_return_type_argument_stays_a_type() {
        // The comma here sits inside the angle brackets, so the type stays open
        // and the second argument is still a type.
        let text = "func f() -> Map<K, V> {\n}\n";
        let toks = roled(text);
        let v = toks.iter().find(|(_, s)| s == "V").expect("V is present");
        assert_eq!(v.0, type_index(Sem::Type), "V is a type argument");
    }

    #[test]
    fn does_not_treat_a_slash_in_a_string_as_a_comment() {
        let text = "func f() -> void {\n    let s = \"http://x\"\n}\n";
        let d = decoded(text);
        assert!(
            !d.iter().any(|t| t.3 == type_index(Sem::Comment)),
            "a slash inside a string is not a comment"
        );
        assert!(d.iter().any(|t| t.3 == type_index(Sem::String)));
    }
}
