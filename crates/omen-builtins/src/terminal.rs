//! Terminal builtins: `clear`.

use super::BuiltinOutput;
use serde_json::json;

/// `clear`: emits the ANSI home-and-erase sequence (`ESC[H ESC[2J`).
///
/// No operands are accepted. The sequence is just bytes on stdout; the
/// terminal (or the capture harness) decides what it means. Exit is always 0.
pub fn clear(args: &[String]) -> BuiltinOutput {
    if !args.is_empty() {
        return BuiltinOutput::failed(
            "clear: takes no operands".to_string(),
            json!({"builtin": "clear", "error": "unexpected_operand"}),
        );
    }
    BuiltinOutput::ok(
        b"\x1b[H\x1b[2J".to_vec(),
        json!({"builtin": "clear", "sequence": "home+erase-display"}),
    )
}
