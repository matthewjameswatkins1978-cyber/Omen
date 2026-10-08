//! Byte-formatting builtins: `echo`, `printf`.

use super::BuiltinOutput;
use serde_json::json;

/// `echo [-n] [-e|-E] [word ...]`: writes words joined by spaces.
///
/// `-n` suppresses the trailing newline. `-e` interprets backslash escapes;
/// `-E` (default) leaves them literal. Operands after the flags are opaque
/// bytes only in the sense that Rust `String`s are UTF-8; the shell grammar
/// already decoded them, so `echo` never touches raw OS bytes.
pub fn echo(args: &[String]) -> BuiltinOutput {
    let mut newline = true;
    let mut interpret_escapes = false;
    let mut operands: &[String] = args;
    // POSIX allows flags to be combined (`-ne`) and parsing stops at the
    // first non-flag operand.
    for (index, arg) in args.iter().enumerate() {
        if arg == "--" {
            operands = &args[index + 1..];
            break;
        }
        let mut is_flag = arg.starts_with('-') && arg.len() > 1;
        if is_flag {
            for flag in arg[1..].chars() {
                match flag {
                    'n' => newline = false,
                    'e' => interpret_escapes = true,
                    'E' => interpret_escapes = false,
                    _ => {
                        is_flag = false;
                        break;
                    }
                }
            }
        }
        if !is_flag {
            operands = &args[index..];
            break;
        }
        if index + 1 == args.len() {
            operands = &[];
        }
    }
    let mut out = Vec::new();
    for (index, word) in operands.iter().enumerate() {
        if index > 0 {
            out.push(b' ');
        }
        if interpret_escapes {
            out.extend(interpret_echo_escapes(word));
        } else {
            out.extend(word.as_bytes());
        }
    }
    if newline {
        out.push(b'\n');
    }
    let bytes = out.len();
    BuiltinOutput::ok(
        out,
        json!({"builtin": "echo", "newline": newline, "operands": operands.len(), "bytes": bytes}),
    )
}

fn interpret_echo_escapes(word: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = word.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            let mut buffer = [0u8; 4];
            out.extend(ch.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => out.push(b'\n'),
            Some('t') => out.push(b'\t'),
            Some('r') => out.push(b'\r'),
            Some('\\') => out.push(b'\\'),
            Some('a') => out.push(0x07),
            Some('b') => out.push(0x08),
            Some('f') => out.push(0x0C),
            Some('v') => out.push(0x0B),
            Some('0') => {
                // `\0ooo`: up to three octal digits.
                let mut value: u32 = 0;
                for _ in 0..3 {
                    match chars.peek().copied() {
                        Some(digit @ '0'..='7') => {
                            value = value * 8 + (digit as u32 - '0' as u32);
                            chars.next();
                        }
                        _ => break,
                    }
                }
                out.push(value as u8);
            }
            Some(other) => {
                out.push(b'\\');
                let mut buffer = [0u8; 4];
                out.extend(other.encode_utf8(&mut buffer).as_bytes());
            }
            None => out.push(b'\\'),
        }
    }
    out
}

/// `printf format [arg ...]`: formats per POSIX `printf`.
///
/// Supported conversions: `%s %d %i %u %o %x %X %c %b %f %e %E %g %G %%`.
/// The format is reused cyclically until all args are consumed; missing args
/// behave as empty/zero. `%b` interprets backslash escapes in its argument.
/// Unknown conversions are a usage error (exit 1), not silent data corruption.
pub fn printf(args: &[String]) -> BuiltinOutput {
    let Some((format, operands)) = args.split_first() else {
        return BuiltinOutput::failed(
            "printf: usage: printf format [argument ...]".to_string(),
            json!({"builtin": "printf", "error": "missing_format"}),
        );
    };
    let expanded_format = interpret_echo_escapes(format);
    let format_text = String::from_utf8_lossy(&expanded_format);
    let mut renderer = PrintfRenderer::new(&format_text);
    match renderer.render(operands) {
        Ok(out) => {
            let bytes = out.len();
            BuiltinOutput::ok(
                out,
                json!({"builtin": "printf", "operands": operands.len(), "bytes": bytes}),
            )
        }
        Err(message) => BuiltinOutput::failed(
            format!("printf: {message}"),
            json!({"builtin": "printf", "error": message}),
        ),
    }
}

struct PrintfRenderer<'a> {
    format: &'a str,
}

impl<'a> PrintfRenderer<'a> {
    fn new(format: &'a str) -> Self {
        Self { format }
    }

    fn render(&mut self, operands: &[String]) -> Result<Vec<u8>, String> {
        // POSIX: an empty format with operands still succeeds with no output.
        if self.format.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut arg_index = 0;
        loop {
            let consumed = self.render_once(&mut out, operands, &mut arg_index)?;
            if arg_index >= operands.len() {
                break;
            }
            if !consumed {
                // Format has no conversions but operands remain: POSIX
                // reuses the whole format string.
                continue;
            }
        }
        Ok(out)
    }

    /// Renders one pass over the format string. Returns whether any
    /// conversion consumed an operand.
    fn render_once(
        &self,
        out: &mut Vec<u8>,
        operands: &[String],
        arg_index: &mut usize,
    ) -> Result<bool, String> {
        let mut consumed = false;
        let mut chars = self.format.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '%' {
                let mut buffer = [0u8; 4];
                out.extend(ch.encode_utf8(&mut buffer).as_bytes());
                continue;
            }
            // Flags / width / precision: accepted and honored for numeric
            // padding in the common `-`, `0`, width, `.precision` shape.
            let mut spec = String::from("%");
            while let Some(&next) = chars.peek() {
                if next.is_ascii_alphabetic() || next == '%' {
                    break;
                }
                spec.push(next);
                chars.next();
            }
            let conversion = chars
                .next()
                .ok_or_else(|| "format ends with bare '%'".to_string())?;
            spec.push(conversion);
            if conversion == '%' {
                out.push(b'%');
                continue;
            }
            let operand = operands.get(*arg_index).cloned().unwrap_or_default();
            *arg_index += 1;
            consumed = true;
            match conversion {
                's' => out.extend(operand.as_bytes()),
                'b' => out.extend(interpret_echo_escapes(&operand)),
                'c' => {
                    let byte = operand.bytes().next().unwrap_or(0);
                    out.push(byte);
                }
                'd' | 'i' => {
                    let value: i64 = parse_int_operand(&operand)?;
                    out.extend(apply_width(&spec, &value.to_string()).as_bytes());
                }
                'u' => {
                    let value: i64 = parse_int_operand(&operand)?;
                    let unsigned = value as u64;
                    out.extend(apply_width(&spec, &unsigned.to_string()).as_bytes());
                }
                'o' => {
                    let value: u64 = parse_uint_operand(&operand)?;
                    out.extend(apply_width(&spec, &format!("{value:o}")).as_bytes());
                }
                'x' => {
                    let value: u64 = parse_uint_operand(&operand)?;
                    out.extend(apply_width(&spec, &format!("{value:x}")).as_bytes());
                }
                'X' => {
                    let value: u64 = parse_uint_operand(&operand)?;
                    out.extend(apply_width(&spec, &format!("{value:X}")).as_bytes());
                }
                'f' | 'F' | 'e' | 'E' | 'g' | 'G' => {
                    let value: f64 = operand.parse().map_err(|_| {
                        format!("expected a number for %{conversion}, got {operand:?}")
                    })?;
                    let rendered = render_float(&spec, conversion, value);
                    out.extend(rendered.as_bytes());
                }
                _ => return Err(format!("unsupported conversion '%{conversion}'")),
            }
        }
        Ok(consumed)
    }
}

fn parse_int_operand(operand: &str) -> Result<i64, String> {
    let trimmed = operand.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    // Accept leading `0x`/`0o`/`0b` and float truncation like POSIX shells do.
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        return i64::from_str_radix(hex, 16)
            .map_err(|_| format!("expected an integer, got {operand:?}"));
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        return Ok(value);
    }
    if let Ok(value) = trimmed.parse::<f64>() {
        return Ok(value.trunc() as i64);
    }
    Err(format!("expected an integer, got {operand:?}"))
}

fn parse_uint_operand(operand: &str) -> Result<u64, String> {
    Ok(parse_int_operand(operand)? as u64)
}

/// Applies `-`/`0`/width/precision padding from a printf spec to a
/// pre-rendered integer body. Only the common subset is honored; anything
/// else passes the body through unchanged.
fn apply_width(spec: &str, body: &str) -> String {
    let inner = spec
        .trim_start_matches('%')
        .trim_end_matches(|ch: char| ch.is_ascii_alphabetic());
    let (left, width) = match inner.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, inner),
    };
    let (zero, width) = match width.strip_prefix('0') {
        Some(rest) if !rest.is_empty() => (true, rest),
        _ => (false, width),
    };
    let width: usize = width.split('.').next().unwrap_or("").parse().unwrap_or(0);
    if width <= body.len() {
        return body.to_string();
    }
    let pad = width - body.len();
    if left {
        format!("{body}{}", " ".repeat(pad))
    } else if zero && (body.starts_with('-') || body.starts_with('+')) {
        format!("{}{}", &body[..1], "0".repeat(pad) + &body[1..])
    } else if zero {
        "0".repeat(pad) + body
    } else {
        " ".repeat(pad) + body
    }
}

fn render_float(spec: &str, conversion: char, value: f64) -> String {
    let inner = spec
        .trim_start_matches('%')
        .trim_end_matches(|ch: char| ch.is_ascii_alphabetic());
    let inner = inner
        .strip_prefix('-')
        .or_else(|| inner.strip_prefix(' '))
        .unwrap_or(inner);
    let precision: usize = inner
        .split('.')
        .nth(1)
        .and_then(|part| {
            part.chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or(6);
    match conversion {
        'f' | 'F' => format!("{value:.precision$}"),
        'e' => format!("{value:.precision$e}"),
        'E' => format!("{value:.precision$E}"),
        _ => format!("{value:.precision$}"),
    }
}
