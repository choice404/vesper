//! A cheap guard over the token stream, run before a buffer reaches the dusk
//! parser.
//!
//! The dusk parser is recursive descent with no depth limit, so a buffer that
//! nests deeply, a wall of `(((`, a long `****T`, or a `.f.f.f` chain tens of
//! thousands deep, overflows its stack and aborts the whole process. Rust cannot
//! catch a stack overflow, so vesper cannot let one start. This scans the tokens
//! the lexer already produced, which never recurses, and holds a buffer back from
//! the parser when its shape would drive the parser past a safe depth.
//!
//! The limits sit far above any real program. One line runs to thousands of
//! tokens, or brackets nest hundreds deep, only in generated or pathological
//! input, never in code a person writes.

use dusk::lexer::token::{Token, TokenKind};

/// The most tokens one logical line may hold. A dusk expression parses within a
/// single line, so this bounds how far a chain of prefixes, postfixes, or
/// operands can drive the parser down.
const MAX_LINE_TOKENS: u32 = 4096;

/// The deepest `()`, `[]`, or `{}` nesting allowed. Each open bracket is another
/// parser frame, so this caps recursion that spans several lines, like nested
/// blocks or a nested collection literal.
const MAX_BRACKET_DEPTH: u32 = 512;

/// Why a buffer was held back from the parser.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TooComplex {
    /// One line carries more tokens than the parser can descend safely.
    LineTooLong,
    /// Brackets nest deeper than the parser can descend safely.
    NestingTooDeep,
}

impl TooComplex {
    /// The message vesper shows in place of the analysis it skipped, so the
    /// silence has a reason the editor can surface.
    pub fn reason(self) -> &'static str {
        match self {
            TooComplex::LineTooLong => {
                "vesper skipped analysis here: this line holds too many tokens for the dusk parser to descend safely"
            }
            TooComplex::NestingTooDeep => {
                "vesper skipped analysis here: this nests too deeply for the dusk parser to descend safely"
            }
        }
    }
}

/// The first point, as a byte offset, where the token stream would drive the
/// parser past a safe depth, or nothing when the whole buffer stays within the
/// limits. The scan stops at the first breach, so it stays cheap even on a
/// runaway input.
pub fn check(tokens: &[Token]) -> Option<(TooComplex, u32)> {
    let mut line_tokens = 0u32;
    let mut depth = 0u32;
    for tok in tokens {
        if tok.nl_before {
            line_tokens = 0;
        }
        line_tokens += 1;
        if line_tokens > MAX_LINE_TOKENS {
            return Some((TooComplex::LineTooLong, tok.span.lo));
        }
        match tok.kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                depth += 1;
                if depth > MAX_BRACKET_DEPTH {
                    return Some((TooComplex::NestingTooDeep, tok.span.lo));
                }
            }
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk::lexer::lex;

    fn tokens(src: &str) -> Vec<Token> {
        lex(src).0
    }

    #[test]
    fn a_normal_program_passes() {
        let src = "@paradigm procedural\nfunc main() -> int32 {\n    return 0\n}\n";
        assert!(check(&tokens(src)).is_none());
    }

    #[test]
    fn a_deeply_nested_expression_is_held_back() {
        let deep = format!("func main() -> int32 {{\n    return {}0{}\n}}\n", "(".repeat(2000), ")".repeat(2000));
        let hit = check(&tokens(&deep));
        assert_eq!(hit.map(|(k, _)| k), Some(TooComplex::NestingTooDeep));
    }

    #[test]
    fn a_very_long_line_is_held_back() {
        // A postfix chain that never overflows a bracket, but runs the parser
        // deep on one line, is caught by the token count instead.
        let chain = format!("func main() -> int32 {{\n    return x{}\n}}\n", ".f".repeat(5000));
        let hit = check(&tokens(&chain));
        assert_eq!(hit.map(|(k, _)| k), Some(TooComplex::LineTooLong));
    }

    #[test]
    fn nesting_that_closes_does_not_accumulate() {
        // Shallow nesting repeated across many short lines stays under the depth
        // cap, since each line's brackets close before the next.
        let mut src = String::from("@paradigm procedural\n");
        for _ in 0..1000 {
            src.push_str("func f() -> int32 {\n    return (1 + 2)\n}\n");
        }
        assert!(check(&tokens(&src)).is_none());
    }
}
