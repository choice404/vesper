//! Builtin names the language blesses, shared by highlighting and navigation.

/// The builtin primitive type names, matching the compiler's own set in
/// `sema::resolve::is_type_name`. They lex as ordinary identifiers, so both
/// highlighting and hover recognize them by spelling. `error` is not among them:
/// it is a value type the compiler resolves elsewhere, and a name spelled
/// `error` in value position is an ordinary identifier.
pub fn is_primitive(name: &str) -> bool {
    matches!(
        name,
        "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "float32"
            | "float64"
            | "bool"
            | "char"
            | "string"
            | "void"
            | "thread"
    )
}
