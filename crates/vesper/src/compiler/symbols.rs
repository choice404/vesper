//! Navigation over a single document: the symbols it declares, hover text for a
//! name, and the definition a name points at.
//!
//! The syntax tree carries a span only for a function name and for the `impl`
//! and `foreign` keywords, not for a struct, enum, or interface name, nor for a
//! type or a parameter. So the declaration ranges come from pairing a
//! declaration keyword in the token stream with the name that follows it, while
//! the tree supplies the detail, the rendered signature, keyed by name.

use std::collections::{HashMap, VecDeque};

use tower_lsp::lsp_types::{
    Hover, HoverContents, MarkupContent, MarkupKind, Position, Range, SymbolKind,
};

use dusk::lexer::lex;
use dusk::lexer::token::{Keyword, Token, TokenKind};
use dusk::parser::ast;
use dusk::parser::{self};

use super::builtins::is_primitive;
use crate::position::LineIndex;

/// One declared name in a document: where its name sits, what it is, and a one
/// line rendering of its signature for hover.
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub detail: String,
    pub name_range: Range,
}

/// The symbols a document declares, top level items and the methods inside impl
/// and foreign blocks, in source order.
pub fn document_symbols(text: &str) -> Vec<Symbol> {
    let (tokens, _) = lex(text);
    let index = LineIndex::new(text);
    let sites = decl_sites(&tokens, text, &index);
    let (module, _) = parser::parse(tokens);
    // Signatures are drawn per name in source order, so a method and a free
    // function that share a name each keep their own, and the outline, hover, and
    // definition never show a signature from the wrong declaration.
    let mut details = details(&module);

    sites
        .into_iter()
        .map(|(kind, name, name_range)| {
            let detail = details
                .get_mut(&name)
                .and_then(|q| q.pop_front())
                .unwrap_or_else(|| header(kind, &name));
            Symbol {
                name,
                kind,
                detail,
                name_range,
            }
        })
        .collect()
}

/// Hover text for the name under `pos`: a builtin type is named as such, and a
/// declared name shows its signature. An unknown name, a local, or a parameter
/// yields nothing, since those have no signature to show yet.
pub fn hover(text: &str, pos: Position) -> Option<Hover> {
    let index = LineIndex::new(text);
    let offset = index.offset(text, pos);
    let (name, range) = ident_at(text, offset, &index)?;

    let detail = if is_primitive(&name) {
        format!("{name}\n\nbuiltin type")
    } else {
        document_symbols(text)
            .into_iter()
            .find(|s| s.name == name)?
            .detail
    };

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("```dusk\n{detail}\n```"),
        }),
        range: Some(range),
    })
}

/// The range of the definition the name under `pos` refers to, within this
/// document. Cross file definitions wait for the workspace index.
pub fn definition(text: &str, pos: Position) -> Option<Range> {
    let index = LineIndex::new(text);
    let offset = index.offset(text, pos);
    let (name, _) = ident_at(text, offset, &index)?;
    document_symbols(text)
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| s.name_range)
}

/// The identifier whose span covers `offset`, with its range. The end is
/// inclusive so a cursor resting just past the last character still lands on it.
fn ident_at(text: &str, offset: u32, index: &LineIndex) -> Option<(String, Range)> {
    let (tokens, _) = lex(text);
    for tok in &tokens {
        if let TokenKind::Ident(name) = &tok.kind {
            if tok.span.lo <= offset && offset <= tok.span.hi {
                let range = Range::new(
                    index.position(text, tok.span.lo),
                    index.position(text, tok.span.hi),
                );
                return Some((name.clone(), range));
            }
        }
    }
    None
}

/// Pairs each declaration keyword with the name token that follows it, giving a
/// precise range for names the tree does not span.
fn decl_sites(tokens: &[Token], text: &str, index: &LineIndex) -> Vec<(SymbolKind, String, Range)> {
    let mut out = Vec::new();
    let mut pending: Option<SymbolKind> = None;
    for tok in tokens {
        match &tok.kind {
            TokenKind::Kw(kw) => {
                pending = match kw {
                    Keyword::Func => Some(SymbolKind::FUNCTION),
                    Keyword::Struct => Some(SymbolKind::STRUCT),
                    Keyword::Enum => Some(SymbolKind::ENUM),
                    Keyword::Interface => Some(SymbolKind::INTERFACE),
                    Keyword::Monad => Some(SymbolKind::NAMESPACE),
                    _ => None,
                };
            }
            TokenKind::Ident(name) => {
                if let Some(kind) = pending.take() {
                    let range = Range::new(
                        index.position(text, tok.span.lo),
                        index.position(text, tok.span.hi),
                    );
                    out.push((kind, name.clone(), range));
                }
            }
            _ => pending = None,
        }
    }
    out
}

/// The signatures a document declares, queued per name in source order. A name
/// declared more than once, a free function and a method for instance, keeps one
/// entry per declaration, so each declaration site draws its own signature.
fn details(m: &ast::Module) -> HashMap<String, VecDeque<String>> {
    let mut map: HashMap<String, VecDeque<String>> = HashMap::new();
    // A monad block declares a namespace name of its own, so its `monad Name`
    // site has a header to show.
    for (name, _) in &m.monads {
        push_sig(&mut map, name, format!("monad {name}"));
    }
    for item in &m.items {
        match item {
            ast::Item::Func(f) => push_sig(&mut map, &f.name, func_sig(f)),
            ast::Item::Struct(s) => push_sig(&mut map, &s.name, struct_sig(s)),
            ast::Item::Enum(e) => push_sig(&mut map, &e.name, enum_sig(e)),
            ast::Item::Interface(i) => push_sig(&mut map, &i.name, interface_sig(i)),
            ast::Item::Impl(im) => {
                for meth in &im.methods {
                    push_sig(&mut map, &meth.name, func_sig(meth));
                }
            }
            ast::Item::Foreign(fo) => {
                for ff in &fo.funcs {
                    push_sig(&mut map, &ff.name, foreign_sig(ff));
                }
            }
        }
    }
    map
}

/// Queues a signature under a name's trailing segment, so a monad method the
/// parser renamed to `Monad.method` is keyed by `method`, matching the bare name
/// the token stream pairs the declaration with.
fn push_sig(map: &mut HashMap<String, VecDeque<String>>, name: &str, sig: String) {
    map.entry(tail(name).to_string()).or_default().push_back(sig);
}

/// The segment after the last dot, or the whole name when it has none.
fn tail(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// A bare header for a declaration the tree offered no detail for.
fn header(kind: SymbolKind, name: &str) -> String {
    let word = match kind {
        SymbolKind::FUNCTION => "func",
        SymbolKind::STRUCT => "struct",
        SymbolKind::ENUM => "enum",
        SymbolKind::INTERFACE => "interface",
        SymbolKind::NAMESPACE => "monad",
        _ => "",
    };
    format!("{word} {name}").trim().to_string()
}

fn func_sig(f: &ast::Func) -> String {
    let word = if f.is_async { "async func" } else { "func" };
    format!(
        "{word} {}{}({}) -> {}",
        f.name,
        generics(&f.generics),
        params(&f.params),
        type_str(&f.ret)
    )
}

/// Renders a parameter list, keeping the `using` marker that binds the ambient
/// allocator.
fn params(ps: &[ast::Param]) -> String {
    ps.iter().map(param_str).collect::<Vec<_>>().join(", ")
}

fn param_str(p: &ast::Param) -> String {
    let marker = if p.using { "using " } else { "" };
    format!("{marker}{}: {}", p.name, type_str(&p.ty))
}

fn struct_sig(s: &ast::Struct) -> String {
    let fields = s
        .fields
        .iter()
        .map(|f| format!("{}: {}", f.name, type_str(&f.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("struct {}{} {{ {} }}", s.name, generics(&s.generics), fields)
}

fn enum_sig(e: &ast::Enum) -> String {
    let variants = e
        .variants
        .iter()
        .map(|v| {
            if v.fields.is_empty() {
                v.name.clone()
            } else {
                // Variant payloads are named fields in dusk, so keep the names.
                let fields = v
                    .fields
                    .iter()
                    .map(|f| format!("{}: {}", f.name, type_str(&f.ty)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", v.name, fields)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("enum {}{} {{ {} }}", e.name, generics(&e.generics), variants)
}

fn interface_sig(i: &ast::Interface) -> String {
    let methods = i
        .methods
        .iter()
        .map(method_sig)
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "interface {}{} {{ {} }}",
        i.name,
        generics(&i.generics),
        methods
    )
}

fn method_sig(m: &ast::MethodSig) -> String {
    format!("{}({}) -> {}", m.name, params(&m.params), type_str(&m.ret))
}

fn foreign_sig(f: &ast::ForeignFunc) -> String {
    format!("func {}({}) -> {}", f.name, params(&f.params), type_str(&f.ret))
}

/// Renders a generic list as `<A, B>`, or nothing when there are no parameters.
fn generics(g: &[String]) -> String {
    if g.is_empty() {
        String::new()
    } else {
        format!("<{}>", g.join(", "))
    }
}

/// Renders a type back to its dusk spelling.
fn type_str(t: &ast::Type) -> String {
    match t {
        ast::Type::Named(name, args) => {
            if args.is_empty() {
                name.clone()
            } else {
                let inner = args.iter().map(type_str).collect::<Vec<_>>().join(", ");
                format!("{name}<{inner}>")
            }
        }
        ast::Type::Ptr(inner) => format!("*{}", type_str(inner)),
        ast::Type::RawPtr(inner) => format!("*raw {}", type_str(inner)),
        // Slices and arrays write their length suffix after the element type in
        // dusk. A pointer or function element is parenthesized so the suffix
        // binds to the whole element and the text reads back as the same type.
        ast::Type::Slice(inner) => format!("{}[]", postfix_atom(inner)),
        ast::Type::Array(inner, n) => format!("{}[{n}]", postfix_atom(inner)),
        ast::Type::Tuple(items) => {
            let inner = items.iter().map(type_str).collect::<Vec<_>>().join(", ");
            format!("({inner})")
        }
        ast::Type::Func(params, ret) => {
            let inner = params.iter().map(type_str).collect::<Vec<_>>().join(", ");
            format!("({inner}) -> {}", type_str(ret))
        }
        ast::Type::Unit => "void".to_string(),
    }
}

/// Renders a type for use before a postfix `[]` or `[N]`, wrapping a pointer or
/// function type in parens so the suffix does not rebind to part of it.
fn postfix_atom(t: &ast::Type) -> String {
    match t {
        ast::Type::Ptr(_) | ast::Type::RawPtr(_) | ast::Type::Func(_, _) => {
            format!("({})", type_str(t))
        }
        _ => type_str(t),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "@paradigm procedural\n\
struct Point { x: int64, y: int64 }\n\
\n\
func dist(a: Point, b: Point) -> int64 {\n\
    return 0\n\
}\n\
\n\
func main() -> int32 {\n\
    d := dist(a, b)\n\
    return 0\n\
}\n";

    fn offset_of(sub: &str) -> u32 {
        SRC.find(sub).expect("substring present") as u32
    }

    #[test]
    fn lists_the_documents_declarations() {
        let syms = document_symbols(SRC);
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Point"));
        assert!(names.contains(&"dist"));
        assert!(names.contains(&"main"));
    }

    #[test]
    fn hovers_a_function_with_its_signature() {
        // Hover over the call to dist on the line inside main.
        let pos = LineIndex::new(SRC).position(SRC, offset_of("dist(a, b)"));
        let h = hover(SRC, pos).expect("hover over a call resolves");
        let text = match h.contents {
            HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(text.contains("func dist(a: Point, b: Point) -> int64"), "{text}");
    }

    #[test]
    fn hovers_a_builtin_type() {
        let pos = LineIndex::new(SRC).position(SRC, offset_of("int32"));
        let h = hover(SRC, pos).expect("hover over a builtin type resolves");
        let text = match h.contents {
            HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(text.contains("builtin type"), "{text}");
    }

    #[test]
    fn goes_from_a_call_to_the_definition() {
        let index = LineIndex::new(SRC);
        // The call site of dist inside main.
        let call = index.position(SRC, offset_of("dist(a, b)"));
        // The definition's name, in `func dist(...)`.
        let def = index.position(SRC, offset_of("dist(a: Point"));
        let range = definition(SRC, call).expect("definition resolves");
        assert_eq!(range.start, def, "jumps to the dist declaration name");
    }

    fn detail_of(src: &str, name: &str) -> String {
        document_symbols(src)
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} present"))
            .detail
    }

    #[test]
    fn renders_a_slice_type_with_a_postfix_suffix() {
        let d = detail_of("func f(xs: int32[]) -> void {\n}\n", "f");
        assert!(d.contains("xs: int32[]"), "{d}");
    }

    #[test]
    fn renders_a_using_parameter_with_its_marker() {
        let d = detail_of("func g(using a: int32) -> void {\n}\n", "g");
        assert!(d.contains("using a: int32"), "{d}");
    }

    #[test]
    fn renders_named_enum_variant_fields() {
        let d = detail_of("enum Shape { Circle(radius: float64), Empty }\n", "Shape");
        assert!(d.contains("Circle(radius: float64)"), "{d}");
    }

    #[test]
    fn two_declarations_of_a_name_keep_their_own_signatures() {
        let src = "func foo() -> int32 {\n    return 0\n}\n\
                   func foo() -> int64 {\n    return 1\n}\n";
        let details: Vec<String> = document_symbols(src)
            .into_iter()
            .filter(|s| s.name == "foo")
            .map(|s| s.detail)
            .collect();
        assert_eq!(details.len(), 2, "{details:?}");
        assert!(details.iter().any(|d| d.contains("-> int32")), "{details:?}");
        assert!(details.iter().any(|d| d.contains("-> int64")), "{details:?}");
    }
}
