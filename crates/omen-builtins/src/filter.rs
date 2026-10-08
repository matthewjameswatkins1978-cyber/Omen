//! Line-and-byte filters over files-or-stdin: `head`, `tail`, `wc`,
//! `sort`, `uniq`.
//!
//! Projection rule for every tool here: lines are split on `\n` only and all
//! bytes pass through untouched — NULs, invalid UTF-8 and lone CRs survive.
//! Case folding (`sort -f`, `uniq -i`, `grep -i` when it lands) is ASCII-only
//! and affects comparison, never the emitted bytes. Anything that cannot be
//! said truthfully in this byte model (character counts over non-UTF-8,
//! regex matching) is rejected or deferred, not faked.

use super::{BuiltinContext, BuiltinOutput};
use serde_json::json;
use std::path::PathBuf;

/// Operands resolved to raw bytes: files in order, or stdin when no operands
/// (or `-`) are given. Unreadable files are collected as diagnostics; the
/// caller continues with what survived and exits 1 when any failed.
struct Gathered {
    chunks: Vec<(Option<String>, Vec<u8>)>,
    failures: Vec<String>,
}

fn gather(operands: &[String], ctx: &BuiltinContext, builtin: &str) -> Gathered {
    let mut chunks = Vec::new();
    let mut failures = Vec::new();
    if operands.is_empty() {
        chunks.push((None, ctx.stdin.clone()));
        return Gathered { chunks, failures };
    }
    for operand in operands {
        if operand == "-" {
            chunks.push((None, ctx.stdin.clone()));
            continue;
        }
        let path = PathBuf::from(operand);
        let path = if path.is_absolute() {
            path
        } else {
            ctx.cwd.join(path)
        };
        match std::fs::read(&path) {
            Ok(data) => chunks.push((Some(operand.clone()), data)),
            Err(error) => failures.push(format!("{builtin}: {}: {error}", path.display())),
        }
    }
    Gathered { chunks, failures }
}

/// Splits bytes into lines, each retaining its trailing `\n` except a final
/// unterminated tail.
fn split_lines(data: &[u8]) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in data.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(data[start..=index].to_vec());
            start = index + 1;
        }
    }
    if start < data.len() {
        lines.push(data[start..].to_vec());
    }
    lines
}

/// Usage/option failure with grep's exit-2 convention (distinct from
/// "no match", which is exit 1). The truth keeps `"exit": 2` so agents
/// see the same contract in both channels.
fn fail2(message: &str, mut truth: serde_json::Value) -> BuiltinOutput {
    truth["exit"] = serde_json::json!(2);
    let mut result = BuiltinOutput::failed(message.to_string(), truth);
    result.code = 2;
    result
}

fn finish(
    _builtin: &str,
    out: Vec<u8>,
    truth: serde_json::Value,
    failures: Vec<String>,
) -> BuiltinOutput {
    if failures.is_empty() {
        BuiltinOutput::ok(out, truth)
    } else {
        let mut result = BuiltinOutput::failed(failures.join("\n"), truth);
        result.stdout = out;
        result
    }
}

/// Parses a head/tail count operand: `N`, `+N` (tail: from line N), `-N`
/// (head: all but last N). Returns the signed count.
fn parse_count(text: &str, builtin: &str) -> Result<i64, String> {
    text.parse::<i64>()
        .map_err(|_| format!("{builtin}: invalid count {text:?}"))
}

/// Shared `-n`/`-N`/`--lines=` flag parsing for `head` and `tail`.
/// Returns `(signed_count_or_default, operands)`.
fn parse_n_flag(
    args: &[String],
    builtin: &str,
    default: i64,
) -> Result<(i64, Vec<String>), String> {
    let mut count = default;
    let mut operands = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            operands.extend(args[index + 1..].iter().cloned());
            break;
        }
        if arg == "-n" || arg == "--lines" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| format!("{builtin}: -n needs a count"))?;
            count = parse_count(value, builtin)?;
        } else if let Some(value) = arg.strip_prefix("--lines=") {
            count = parse_count(value, builtin)?;
        } else if let Some(value) = arg.strip_prefix("-n") {
            count = parse_count(value, builtin)?;
        } else if arg.len() > 1
            && arg.starts_with('-')
            && arg[1..].chars().all(|c| c.is_ascii_digit())
        {
            // Old-style `-N`.
            count = parse_count(&arg[1..], builtin)?;
        } else if arg.starts_with('-') && arg != "-" {
            return Err(format!("{builtin}: unsupported option {arg:?}"));
        } else {
            operands.extend(args[index..].iter().cloned());
            break;
        }
        index += 1;
    }
    Ok((count, operands))
}

/// `head [-n N] [file ...]`: first N lines (default 10) of each input,
/// byte-exact. A negative N prints all but the last |N| lines (GNU).
pub fn head(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let (count, operands) = match parse_n_flag(args, "head", 10) {
        Ok(parsed) => parsed,
        Err(message) => {
            return BuiltinOutput::failed(message, json!({"builtin": "head", "error": "usage"}));
        }
    };
    if count < 0 {
        return head_all_but_last(operands, ctx, -count as usize);
    }
    let gathered = gather(&operands, ctx, "head");
    let want = count as usize;
    let mut out = Vec::new();
    let mut lines_emitted: u64 = 0;
    for (_, data) in &gathered.chunks {
        let remaining = want.saturating_sub(lines_emitted as usize);
        if remaining == 0 {
            break;
        }
        for line in split_lines(data).into_iter().take(remaining) {
            out.extend_from_slice(&line);
            lines_emitted += 1;
        }
        // NOTE: `head` over several files concatenates the first N lines of
        // the combined stream (POSIX), it does not repeat the count per file.
    }
    finish(
        "head",
        out,
        json!({"builtin": "head", "count": count, "lines": lines_emitted, "failures": gathered.failures.len()}),
        gathered.failures,
    )
}

fn head_all_but_last(operands: Vec<String>, ctx: &BuiltinContext, skip: usize) -> BuiltinOutput {
    let gathered = gather(&operands, ctx, "head");
    let mut all: Vec<Vec<u8>> = Vec::new();
    for (_, data) in &gathered.chunks {
        all.extend(split_lines(data));
    }
    let keep = all.len().saturating_sub(skip);
    let mut out = Vec::new();
    for line in all.into_iter().take(keep) {
        out.extend_from_slice(&line);
    }
    finish(
        "head",
        out,
        json!({"builtin": "head", "count": -(skip as i64), "lines": keep, "failures": gathered.failures.len()}),
        gathered.failures,
    )
}

/// `tail [-n N] [file ...]`: last N lines (default 10), byte-exact.
/// `-n +N` prints from line N of the combined stream.
pub fn tail(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let (count, operands) = match parse_n_flag(args, "tail", 10) {
        Ok(parsed) => parsed,
        Err(message) => {
            return BuiltinOutput::failed(message, json!({"builtin": "tail", "error": "usage"}));
        }
    };
    let gathered = gather(&operands, ctx, "tail");
    let mut all: Vec<Vec<u8>> = Vec::new();
    for (_, data) in &gathered.chunks {
        all.extend(split_lines(data));
    }
    let selected: Vec<Vec<u8>> = if count >= 0 {
        let want = count as usize;
        all.into_iter()
            .rev()
            .take(want)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        // `tail -n -N`: last N lines, same as plain N.
        let want = (-count) as usize;
        all.into_iter()
            .rev()
            .take(want)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    };
    let lines = selected.len();
    let mut out = Vec::new();
    for line in selected {
        out.extend_from_slice(&line);
    }
    // From-line form: re-derive when the raw `-n` value had a `+` prefix.
    let from_line: Option<i64> = args
        .windows(2)
        .find(|w| w[0] == "-n" && w[1].starts_with('+'))
        .and_then(|w| w[1][1..].parse().ok());
    if let Some(start) = from_line {
        let gathered = gather(&operands, ctx, "tail");
        let mut all: Vec<Vec<u8>> = Vec::new();
        for (_, data) in &gathered.chunks {
            all.extend(split_lines(data));
        }
        let skip = (start.max(1) - 1) as usize;
        out.clear();
        for line in all.into_iter().skip(skip) {
            out.extend_from_slice(&line);
        }
        let _lines = out.iter().filter(|b| **b == b'\n').count();
        return finish(
            "tail",
            out,
            json!({"builtin": "tail", "from_line": start, "failures": gathered.failures.len()}),
            gathered.failures,
        );
    }
    let line_count = out.iter().filter(|b| **b == b'\n').count();
    let _ = lines;
    finish(
        "tail",
        out,
        json!({"builtin": "tail", "count": count, "lines": line_count, "failures": gathered.failures.len()}),
        gathered.failures,
    )
}

/// `wc [-lwc] [file ...]`: lines, words, bytes. Default with no flags is all
/// three. Words are ASCII-whitespace-separated byte runs; `-m` (chars) is
/// refused because chars are not truthful over non-UTF-8 input.
pub fn wc(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut show_lines = false;
    let mut show_words = false;
    let mut show_bytes = false;
    let mut operands: &[String] = &[];
    for (index, arg) in args.iter().enumerate() {
        if arg == "--" {
            operands = &args[index + 1..];
            break;
        }
        if arg.starts_with('-') && arg.len() > 1 && arg != "-" {
            for flag in arg[1..].chars() {
                match flag {
                    'l' => show_lines = true,
                    'w' => show_words = true,
                    'c' => show_bytes = true,
                    _ => {
                        return BuiltinOutput::failed(
                            format!("wc: unsupported option '-{flag}' (only -l, -w, -c)"),
                            json!({"builtin": "wc", "error": "unsupported_option"}),
                        );
                    }
                }
            }
            if index + 1 == args.len() {
                operands = &[];
            }
            continue;
        }
        operands = &args[index..];
        break;
    }
    if !show_lines && !show_words && !show_bytes {
        show_lines = true;
        show_words = true;
        show_bytes = true;
    }
    let gathered = gather(operands, ctx, "wc");
    let named = operands.iter().any(|o| *o != "-");
    let mut out = Vec::new();
    let (mut total_lines, mut total_words, mut total_bytes) = (0u64, 0u64, 0u64);
    let mut files = 0u64;
    for (name, data) in &gathered.chunks {
        let (lines, words, bytes) = count_bytes(data);
        total_lines += lines;
        total_words += words;
        total_bytes += bytes;
        files += 1;
        out.extend(
            format_counts(show_lines, show_words, show_bytes, lines, words, bytes).as_bytes(),
        );
        if named {
            out.push(b' ');
            out.extend(name.clone().unwrap_or_default().as_bytes());
        }
        out.push(b'\n');
    }
    if gathered.chunks.len() > 1 {
        out.extend(
            format_counts(
                show_lines,
                show_words,
                show_bytes,
                total_lines,
                total_words,
                total_bytes,
            )
            .as_bytes(),
        );
        out.extend(b" total\n");
    }
    finish(
        "wc",
        out,
        json!({"builtin": "wc", "files": files, "lines": total_lines, "words": total_words, "bytes": total_bytes, "failures": gathered.failures.len()}),
        gathered.failures,
    )
}

fn count_bytes(data: &[u8]) -> (u64, u64, u64) {
    let lines = data.iter().filter(|b| **b == b'\n').count() as u64;
    let mut words = 0u64;
    let mut in_word = false;
    for byte in data {
        if byte.is_ascii_whitespace() {
            in_word = false;
        } else if !in_word {
            in_word = true;
            words += 1;
        }
    }
    (lines, words, data.len() as u64)
}

fn format_counts(show_l: bool, show_w: bool, show_c: bool, l: u64, w: u64, c: u64) -> String {
    let mut parts = Vec::new();
    if show_l {
        parts.push(format!("{l:>7}"));
    }
    if show_w {
        parts.push(format!("{w:>7}"));
    }
    if show_c {
        parts.push(format!("{c:>7}"));
    }
    parts.join("")
}

/// `sort [-r] [-n] [-u] [-f] [file ...]`: sorts combined lines (stable).
/// `-n` compares by leading numeric value, `-f` folds ASCII case for the
/// comparison only, `-r` reverses, `-u` keeps the first of each equal run.
/// Keys (`-k`), field separators (`-t`) and check mode (`-c`) are refused.
pub fn sort(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut reverse = false;
    let mut numeric = false;
    let mut unique = false;
    let mut fold = false;
    let mut operands: &[String] = &[];
    for (index, arg) in args.iter().enumerate() {
        if arg == "--" {
            operands = &args[index + 1..];
            break;
        }
        if arg.starts_with('-') && arg.len() > 1 && arg != "-" {
            for flag in arg[1..].chars() {
                match flag {
                    'r' => reverse = true,
                    'n' => numeric = true,
                    'u' => unique = true,
                    'f' => fold = true,
                    _ => {
                        return BuiltinOutput::failed(
                            format!("sort: unsupported option '-{flag}' (only -r, -n, -u, -f)"),
                            json!({"builtin": "sort", "error": "unsupported_option"}),
                        );
                    }
                }
            }
            if index + 1 == args.len() {
                operands = &[];
            }
            continue;
        }
        operands = &args[index..];
        break;
    }
    let gathered = gather(operands, ctx, "sort");
    let mut lines: Vec<Vec<u8>> = Vec::new();
    for (_, data) in &gathered.chunks {
        lines.extend(split_lines(data));
    }
    let input_lines = lines.len();
    lines.sort_by(|a, b| compare_lines(a, b, numeric, fold));
    if reverse {
        lines.reverse();
    }
    if unique {
        let mut kept: Vec<Vec<u8>> = Vec::with_capacity(lines.len());
        for line in lines {
            let equal = kept
                .last()
                .is_some_and(|last: &Vec<u8>| keys_equal(last, &line, numeric, fold));
            if !equal {
                kept.push(line);
            }
        }
        lines = kept;
    }
    let mut out = Vec::new();
    for line in &lines {
        out.extend_from_slice(line);
    }
    finish(
        "sort",
        out,
        json!({"builtin": "sort", "input_lines": input_lines, "output_lines": lines.len(),
               "reverse": reverse, "numeric": numeric, "unique": unique, "fold": fold,
               "failures": gathered.failures.len()}),
        gathered.failures,
    )
}

fn sort_key(line: &[u8], numeric: bool, fold: bool) -> SortKey {
    if numeric {
        SortKey::Number(leading_number(line))
    } else if fold {
        SortKey::Folded(line.to_ascii_lowercase())
    } else {
        SortKey::Bytes(line.to_vec())
    }
}

#[derive(PartialEq)]
enum SortKey {
    Number(f64),
    Folded(Vec<u8>),
    Bytes(Vec<u8>),
}

fn compare_lines(a: &[u8], b: &[u8], numeric: bool, fold: bool) -> std::cmp::Ordering {
    match (sort_key(a, numeric, fold), sort_key(b, numeric, fold)) {
        (SortKey::Number(x), SortKey::Number(y)) => x
            .partial_cmp(&y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.cmp(b)),
        (SortKey::Folded(x), SortKey::Folded(y)) => x.cmp(&y).then_with(|| a.cmp(b)),
        (SortKey::Bytes(x), SortKey::Bytes(y)) => x.cmp(&y),
        _ => std::cmp::Ordering::Equal,
    }
}

fn keys_equal(a: &[u8], b: &[u8], numeric: bool, fold: bool) -> bool {
    compare_lines(a, b, numeric, fold) == std::cmp::Ordering::Equal
        && (!numeric || leading_number(a) == leading_number(b))
}

/// Leading numeric value of a line (after blanks); non-numeric lines are 0.
fn leading_number(line: &[u8]) -> f64 {
    let text = String::from_utf8_lossy(line);
    let trimmed = text.trim_start();
    let mut end = 0;
    for (index, ch) in trimmed.char_indices() {
        if ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+' || ch == 'e' || ch == 'E' {
            end = index + ch.len_utf8();
        } else if index == 0 {
            return 0.0;
        } else {
            break;
        }
    }
    trimmed[..end].parse::<f64>().unwrap_or(0.0)
}

/// `uniq [-c] [-d] [-u] [-i] [file ...]`: adjacent duplicate suppression.
/// `-c` prefixes repeat counts, `-d` keeps only duplicated groups, `-u`
/// keeps only unique lines, `-i` folds ASCII case for comparison only.
pub fn uniq(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut counted = false;
    let mut only_dup = false;
    let mut only_unique = false;
    let mut ignore_case = false;
    let mut operands: &[String] = &[];
    for (index, arg) in args.iter().enumerate() {
        if arg == "--" {
            operands = &args[index + 1..];
            break;
        }
        if arg.starts_with('-') && arg.len() > 1 && arg != "-" {
            for flag in arg[1..].chars() {
                match flag {
                    'c' => counted = true,
                    'd' => only_dup = true,
                    'u' => only_unique = true,
                    'i' => ignore_case = true,
                    _ => {
                        return BuiltinOutput::failed(
                            format!("uniq: unsupported option '-{flag}' (only -c, -d, -u, -i)"),
                            json!({"builtin": "uniq", "error": "unsupported_option"}),
                        );
                    }
                }
            }
            if only_dup && only_unique {
                return BuiltinOutput::failed(
                    "uniq: -d and -u are mutually exclusive".to_string(),
                    json!({"builtin": "uniq", "error": "usage"}),
                );
            }
            if index + 1 == args.len() {
                operands = &[];
            }
            continue;
        }
        operands = &args[index..];
        break;
    }
    let gathered = gather(operands, ctx, "uniq");
    let mut lines: Vec<Vec<u8>> = Vec::new();
    for (_, data) in &gathered.chunks {
        lines.extend(split_lines(data));
    }
    let input_lines = lines.len();
    let mut out = Vec::new();
    let mut groups = 0u64;
    let mut index = 0;
    while index < lines.len() {
        let mut run = 1;
        while index + run < lines.len()
            && same_line(&lines[index], &lines[index + run], ignore_case)
        {
            run += 1;
        }
        groups += 1;
        let emit = (!only_dup || run > 1) && (!only_unique || run == 1);
        if emit {
            if counted {
                out.extend(format!("{run:>7} ").as_bytes());
            }
            out.extend_from_slice(&lines[index]);
        }
        index += run;
    }
    finish(
        "uniq",
        out,
        json!({"builtin": "uniq", "input_lines": input_lines, "groups": groups,
               "counted": counted, "ignore_case": ignore_case, "failures": gathered.failures.len()}),
        gathered.failures,
    )
}

fn same_line(a: &[u8], b: &[u8], ignore_case: bool) -> bool {
    if ignore_case {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// `tee [-a] [file ...]`: copies stdin to stdout byte-exactly.
///
/// The no-operand form (pure passthrough) runs live. FILE operands are
/// REFUSED with exit 1: writing files is a consequential mutation needing
/// the admitted Tethers host-filesystem capability (P2 owner decision
/// pending — same posture as `>` redirects and `omen-mutation`). The
/// refusal names the missing authority instead of silently dropping data.
/// `-a` is accepted (it only matters for file writes) so pipelines do not
/// need flag surgery when the capability lands.
pub fn tee(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut operands: &[String] = &[];
    for (index, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "-a" | "--append" => {
                if index + 1 == args.len() {
                    operands = &[];
                }
            }
            "--" => {
                operands = &args[index + 1..];
                break;
            }
            _ if arg.starts_with('-') && arg != "-" => {
                return BuiltinOutput::failed(
                    format!("tee: unsupported option {arg:?} (only -a)"),
                    json!({"builtin": "tee", "error": "unsupported_option"}),
                );
            }
            _ => {
                operands = &args[index..];
                break;
            }
        }
    }
    if !operands.is_empty() {
        return BuiltinOutput::failed(
            "tee: writing files needs admitted filesystem authority: \
             no Tethers host-execution filesystem capability is admitted yet \
             (P2 owner decision pending)"
                .to_string(),
            json!({"builtin": "tee", "error": "refused_closed", "files": operands.len()}),
        );
    }
    let bytes = ctx.stdin.len();
    BuiltinOutput::ok(
        ctx.stdin.clone(),
        json!({"builtin": "tee", "mode": "passthrough", "bytes": bytes}),
    )
}

/// A 1-based inclusive byte range from a `cut` list item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ByteRange {
    start: usize,
    end: usize,
}

/// Parses `N`, `N-`, `-M`, `N-M` (1-based, `N-M` requires N <= M).
fn parse_cut_list(list: &str) -> Result<Vec<ByteRange>, String> {
    let mut ranges = Vec::new();
    for item in list.split(',') {
        if item.is_empty() {
            return Err("cut: invalid byte/field list (empty item)".to_string());
        }
        let (lo, hi) = match item.split_once('-') {
            None => {
                let n: usize = item
                    .parse()
                    .map_err(|_| format!("cut: invalid position {item:?}"))?;
                if n == 0 {
                    return Err("cut: positions start at 1".to_string());
                }
                (n - 1, n)
            }
            Some(("", hi)) => {
                let m: usize = hi
                    .parse()
                    .map_err(|_| format!("cut: invalid position {item:?}"))?;
                if m == 0 {
                    return Err("cut: positions start at 1".to_string());
                }
                (0, m)
            }
            Some((lo, "")) => {
                let n: usize = lo
                    .parse()
                    .map_err(|_| format!("cut: invalid position {item:?}"))?;
                if n == 0 {
                    return Err("cut: positions start at 1".to_string());
                }
                (n - 1, usize::MAX)
            }
            Some((lo, hi)) => {
                let n: usize = lo
                    .parse()
                    .map_err(|_| format!("cut: invalid position {item:?}"))?;
                let m: usize = hi
                    .parse()
                    .map_err(|_| format!("cut: invalid position {item:?}"))?;
                if n == 0 || m == 0 {
                    return Err("cut: positions start at 1".to_string());
                }
                if n > m {
                    return Err(format!("cut: invalid range {item:?} (start after end)"));
                }
                (n - 1, m)
            }
        };
        ranges.push(ByteRange { start: lo, end: hi });
    }
    ranges.sort();
    Ok(ranges)
}

/// Merges overlapping/adjacent ranges so `-b1-2,2-3` emits bytes 1-3 once
/// (GNU), instead of duplicating the overlap.
fn merge_ranges(mut ranges: Vec<ByteRange>) -> Vec<ByteRange> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort();
    let mut merged = Vec::with_capacity(ranges.len());
    let mut current = ranges[0];
    for range in ranges.into_iter().skip(1) {
        if range.start <= current.end {
            current.end = current.end.max(range.end);
        } else {
            merged.push(current);
            current = range;
        }
    }
    merged.push(current);
    merged
}

/// `cut -b LIST | -c LIST | -f LIST [-d DELIM] [-s] [file ...]`.
///
/// `-b` and `-c` are identical here: positions are BYTES. Over multibyte
/// UTF-8 this may split a character — the projection is documented LOSSY
/// for non-ASCII and the typed truth says `"units": "bytes"`. `-d`
/// takes the first byte of its operand; `--output-delimiter` is refused.
/// `-n` is accepted as a no-op (there is no character splitting to avoid).
/// Only one of `-b`/`-c`/`-f` may be given.
pub fn cut(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    #[derive(PartialEq)]
    enum Mode {
        Bytes,
        Fields,
    }
    let mut mode: Option<Mode> = None;
    let mut list: Option<String> = None;
    let mut delim: u8 = b'\t';
    let mut suppress = false;
    let mut operands: &[String] = &[];
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            operands = &args[index + 1..];
            break;
        }
        if arg == "-d" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| "cut: -d needs a delimiter".to_string());
            let value = match value {
                Ok(v) => v,
                Err(message) => {
                    return BuiltinOutput::failed(
                        message,
                        json!({"builtin": "cut", "error": "usage"}),
                    );
                }
            };
            if value.is_empty() {
                return BuiltinOutput::failed(
                    "cut: the delimiter must be a single byte".to_string(),
                    json!({"builtin": "cut", "error": "usage"}),
                );
            }
            delim = value.as_bytes()[0];
        } else if arg.starts_with("-d") && arg.len() > 2 {
            if arg.as_bytes()[2] > 127 {
                return BuiltinOutput::failed(
                    "cut: the delimiter must be a single byte".to_string(),
                    json!({"builtin": "cut", "error": "usage"}),
                );
            }
            delim = arg.as_bytes()[2];
        } else if arg == "-s" {
            suppress = true;
        } else if arg == "-n" {
            // No-op: byte positions never split characters deliberately.
        } else if arg == "-b" || arg == "-c" || arg == "-f" {
            if mode.is_some() {
                return BuiltinOutput::failed(
                    "cut: only one of -b, -c, -f may be given".to_string(),
                    json!({"builtin": "cut", "error": "usage"}),
                );
            }
            mode = Some(if arg == "-f" {
                Mode::Fields
            } else {
                Mode::Bytes
            });
            index += 1;
            let value = args.get(index).cloned().unwrap_or_default();
            if value.is_empty() {
                return BuiltinOutput::failed(
                    format!("cut: {arg} needs a list"),
                    json!({"builtin": "cut", "error": "usage"}),
                );
            }
            list = Some(value);
        } else if arg.len() > 2
            && arg.starts_with('-')
            && arg.as_bytes()[1] != b'-'
            && "bcf".contains(arg.as_bytes()[1] as char)
        {
            // Joined form: `-f1,3`, `-d,`, `-b2-`.
            let flag = arg.as_bytes()[1] as char;
            let rest = &arg[2..];
            match flag {
                'd' => {
                    if rest.is_empty() || rest.as_bytes()[0] > 127 {
                        return BuiltinOutput::failed(
                            "cut: the delimiter must be a single byte".to_string(),
                            json!({"builtin": "cut", "error": "usage"}),
                        );
                    }
                    delim = rest.as_bytes()[0];
                }
                _ => {
                    if mode.is_some() {
                        return BuiltinOutput::failed(
                            "cut: only one of -b, -c, -f may be given".to_string(),
                            json!({"builtin": "cut", "error": "usage"}),
                        );
                    }
                    mode = Some(if flag == 'f' {
                        Mode::Fields
                    } else {
                        Mode::Bytes
                    });
                    if rest.is_empty() {
                        return BuiltinOutput::failed(
                            format!("cut: -{flag} needs a list"),
                            json!({"builtin": "cut", "error": "usage"}),
                        );
                    }
                    list = Some(rest.to_string());
                }
            }
        } else if arg.starts_with('-') && arg != "-" {
            return BuiltinOutput::failed(
                format!("cut: unsupported option {arg:?} (only -b, -c, -f, -d, -s, -n)"),
                json!({"builtin": "cut", "error": "unsupported_option"}),
            );
        } else {
            operands = &args[index..];
            break;
        }
        index += 1;
    }
    let mode = match mode {
        Some(m) => m,
        None => {
            return BuiltinOutput::failed(
                "cut: usage: cut -b LIST | -c LIST | -f LIST [-d DELIM] [-s] [file ...]"
                    .to_string(),
                json!({"builtin": "cut", "error": "usage"}),
            );
        }
    };
    let ranges = match parse_cut_list(&list.unwrap_or_default()) {
        Ok(r) => merge_ranges(r),
        Err(message) => {
            return BuiltinOutput::failed(message, json!({"builtin": "cut", "error": "usage"}));
        }
    };
    let gathered = gather(operands, ctx, "cut");
    let mut out = Vec::new();
    let mut lines_in = 0u64;
    for (_, data) in &gathered.chunks {
        for line in split_lines(data) {
            lines_in += 1;
            let (body, terminator) = match line.strip_suffix(b"\n") {
                Some(body) => (body, true),
                None => (line.as_slice(), false),
            };
            if mode == Mode::Fields {
                if !body.contains(&delim) {
                    if suppress {
                        continue;
                    }
                    out.extend_from_slice(&line);
                    continue;
                }
                let fields: Vec<&[u8]> = body.split(|b| *b == delim).collect();
                let mut first = true;
                // Ranges index fields 1-based the same way they index bytes.
                for range in &ranges {
                    for field in fields
                        .iter()
                        .take(range.end.min(fields.len()))
                        .skip(range.start)
                    {
                        if !first {
                            out.push(delim);
                        }
                        first = false;
                        out.extend_from_slice(field);
                    }
                }
            } else {
                for range in &ranges {
                    let lo = range.start.min(body.len());
                    let hi = range.end.min(body.len());
                    if lo < hi {
                        out.extend_from_slice(&body[lo..hi]);
                    }
                }
            }
            if terminator {
                out.push(b'\n');
            }
        }
    }
    finish(
        "cut",
        out,
        json!({"builtin": "cut", "units": "bytes", "lines": lines_in,
               "fields": mode == Mode::Fields, "failures": gathered.failures.len()}),
        gathered.failures,
    )
}

/// Expands a `tr` set operand to an explicit byte list. Supports ranges
/// (`a-z`), escapes (`\\ \n \t \r \a \b \f \v \0ooo`) and the
/// classes `[:upper:] [:lower:] [:alpha:] [:digit:] [:space:] [:alnum:]`.
fn expand_tr_set(set: &str) -> Result<Vec<u8>, String> {
    let bytes = set.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    // Class names are matched greedily before single-char handling.
    const CLASSES: &[(&str, &[u8])] = &[
        ("[:upper:]", b"ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        ("[:lower:]", b"abcdefghijklmnopqrstuvwxyz"),
        (
            "[:alpha:]",
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
        ),
        ("[:digit:]", b"0123456789"),
        (
            "[:alnum:]",
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
        ),
        ("[:space:]", b" \t\n\r\x0B\x0C"),
    ];
    while index < bytes.len() {
        let rest = &set[index..];
        let mut matched_class = false;
        for (name, expansion) in CLASSES {
            if rest.starts_with(name) {
                out.extend_from_slice(expansion);
                index += name.len();
                matched_class = true;
                break;
            }
        }
        if matched_class {
            continue;
        }
        let current = decode_tr_char(bytes, &mut index)?;
        // `x-y` range, where both ends are literal (post-escape) bytes.
        if index < bytes.len() && bytes[index] == b'-' && index + 1 < bytes.len() {
            index += 1;
            let end = decode_tr_char(bytes, &mut index)?;
            if current > end {
                return Err(format!("tr: invalid range {current:#04X}-{end:#04X}"));
            }
            for byte in current..=end {
                out.push(byte);
            }
        } else {
            out.push(current);
        }
    }
    Ok(out)
}

fn decode_tr_char(bytes: &[u8], index: &mut usize) -> Result<u8, String> {
    let byte = bytes[*index];
    if byte != b'\\' {
        *index += 1;
        return Ok(byte);
    }
    *index += 1;
    if *index >= bytes.len() {
        return Ok(b'\\');
    }
    let escaped = bytes[*index];
    *index += 1;
    match escaped {
        b'n' => Ok(b'\n'),
        b't' => Ok(b'\t'),
        b'r' => Ok(b'\r'),
        b'a' => Ok(0x07),
        b'b' => Ok(0x08),
        b'f' => Ok(0x0C),
        b'v' => Ok(0x0B),
        b'\\' => Ok(b'\\'),
        b'0'..=b'7' => {
            let mut value = (escaped - b'0') as u32;
            for _ in 0..2 {
                if *index < bytes.len() && (b'0'..=b'7').contains(&bytes[*index]) {
                    value = value * 8 + (bytes[*index] - b'0') as u32;
                    *index += 1;
                } else {
                    break;
                }
            }
            Ok(value as u8)
        }
        other => Ok(other),
    }
}

/// `tr [-c] [-d] [-s] [-t] SET1 [SET2]`: byte translation over stdin.
///
/// Reads stdin only — file operands are refused (GNU `tr` takes no files).
/// `-c` complements SET1 over all 256 bytes (note: this includes `\n`, so
/// `-c` sets usually name it explicitly). With `-d` matching bytes are
/// deleted; otherwise each SET1 byte maps to SET2 (short SET2 repeats its
/// last byte, POSIX). `-s` squeezes repeats of the SET2 bytes when SET2 is
/// given, else of SET1. `-t` is accepted ( its truncation is the default).
pub fn tr(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut delete = false;
    let mut squeeze = false;
    let mut complement = false;
    let mut sets: Vec<String> = Vec::new();
    for arg in args {
        if arg == "--" {
            continue;
        }
        if arg.starts_with('-') && arg.len() > 1 && arg != "-" && sets.is_empty() {
            let mut flags_only = true;
            for flag in arg[1..].chars() {
                if !"cdst".contains(flag) {
                    flags_only = false;
                    break;
                }
            }
            if flags_only {
                for flag in arg[1..].chars() {
                    match flag {
                        'c' => complement = true,
                        'd' => delete = true,
                        's' => squeeze = true,
                        't' => {}
                        _ => unreachable!(),
                    }
                }
                continue;
            }
        }
        sets.push(arg.clone());
    }
    if sets.is_empty() || sets.len() > 2 {
        return BuiltinOutput::failed(
            "tr: usage: tr [-c] [-d] [-s] SET1 [SET2] (stdin only)".to_string(),
            json!({"builtin": "tr", "error": "usage"}),
        );
    }
    if sets.len() == 2 && delete && !squeeze {
        // `tr -d SET1 SET2`: GNU warns; the second set is meaningless.
        return BuiltinOutput::failed(
            "tr: -d takes exactly one set (add -s to squeeze with two sets)".to_string(),
            json!({"builtin": "tr", "error": "usage"}),
        );
    }
    let mut set1 = match expand_tr_set(&sets[0]) {
        Ok(s) => s,
        Err(message) => {
            return BuiltinOutput::failed(message, json!({"builtin": "tr", "error": "usage"}));
        }
    };
    if complement {
        let present = {
            let mut table = [false; 256];
            for byte in &set1 {
                table[*byte as usize] = true;
            }
            table
        };
        set1 = (0u16..=255)
            .filter(|b| !present[*b as usize])
            .map(|b| b as u8)
            .collect();
    }
    let set2: Vec<u8> = match sets.get(1) {
        Some(text) => match expand_tr_set(text) {
            Ok(s) => s,
            Err(message) => {
                return BuiltinOutput::failed(message, json!({"builtin": "tr", "error": "usage"}));
            }
        },
        None => Vec::new(),
    };
    // Translation table: byte -> replacement (identity default).
    let mut table: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut doomed = [false; 256];
    if delete {
        for byte in &set1 {
            doomed[*byte as usize] = true;
        }
    } else if !set2.is_empty() {
        let last = *set2.last().expect("non-empty");
        for (position, byte) in set1.iter().enumerate() {
            table[*byte as usize] = *set2.get(position).unwrap_or(&last);
        }
    } else if squeeze {
        // `tr -s SET1`: squeeze only, no translation (identity table).
    } else {
        return BuiltinOutput::failed(
            "tr: translating without SET2 deletes nothing; give SET2 or -d/-s".to_string(),
            json!({"builtin": "tr", "error": "usage"}),
        );
    }
    let squeeze_set: Vec<bool> = if squeeze {
        let source = if !set2.is_empty() && !delete {
            &set2
        } else {
            &set1
        };
        let mut present = [false; 256];
        for byte in source {
            present[*byte as usize] = true;
        }
        // When translating, squeeze applies to the translated bytes: map
        // the squeeze set through the table.
        if !delete && !set2.is_empty() {
            let mut translated = [false; 256];
            for (input, keep) in present.iter().enumerate() {
                if *keep {
                    translated[table[input] as usize] = true;
                }
            }
            translated.to_vec()
        } else {
            present.to_vec()
        }
    } else {
        vec![false; 256]
    };
    let mut out = Vec::with_capacity(ctx.stdin.len());
    let mut previous: Option<u8> = None;
    for byte in &ctx.stdin {
        if doomed[*byte as usize] {
            continue;
        }
        let mapped = table[*byte as usize];
        if squeeze && squeeze_set[mapped as usize] && previous == Some(mapped) {
            continue;
        }
        out.push(mapped);
        previous = Some(mapped);
    }
    let bytes_in = ctx.stdin.len();
    let bytes_out = out.len();
    BuiltinOutput::ok(
        out,
        json!({"builtin": "tr", "bytes_in": bytes_in, "bytes_out": bytes_out,
               "delete": delete, "squeeze": squeeze, "complement": complement}),
    )
}

/// `grep [-i] [-v] [-c] [-n] [-q] [-x] [-F] [-H|-h] [-e PATTERN]... PATTERN [file ...]`.
///
/// Matching is LITERAL substring (`"mode": "literal"` in the typed
/// truth) — there is no regex engine in the workspace, and pretending
/// otherwise would be a truth violation. `-F` is accepted as a no-op
/// (it states what is already true); `-E`/`-G` are refused. External `rg`
/// remains the regex answer. No binary-file special casing: selected bytes
/// are emitted as-is; a NUL-containing match is still a match.
pub fn grep(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut ignore_case = false;
    let mut invert = false;
    let mut counted = false;
    let mut numbered = false;
    let mut quiet = false;
    let mut whole_line = false;
    let mut force_names: Option<bool> = None;
    let mut patterns: Vec<Vec<u8>> = Vec::new();
    let mut operands: Vec<String> = Vec::new();
    let mut index = 0;
    let mut pattern_done = false;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            // Everything after `--`: first item is the pattern if none yet.
            for rest in args.iter().skip(index + 1) {
                if !pattern_done {
                    patterns.push(rest.as_bytes().to_vec());
                    pattern_done = true;
                } else {
                    operands.push(rest.clone());
                }
            }
            break;
        }
        if arg == "-e" || arg == "--regexp" {
            index += 1;
            match args.get(index) {
                Some(p) => patterns.push(p.as_bytes().to_vec()),
                None => {
                    return fail2(
                        "grep: -e needs a pattern",
                        json!({"builtin": "grep", "error": "usage"}),
                    );
                }
            }
        } else if arg.starts_with("-e") && arg.len() > 2 {
            patterns.push(arg.as_bytes()[2..].to_vec());
        } else if !pattern_done && arg.starts_with('-') && arg.len() > 1 && arg != "-" {
            let mut only_flags = true;
            for flag in arg[1..].chars() {
                if !"ivcnqxFHhfe".contains(flag) {
                    only_flags = false;
                    break;
                }
            }
            if !only_flags {
                return fail2(
                    &format!(
                        "grep: unsupported option {arg:?} (only -i -v -c -n -q -x -F -H -h -e; -E/-G regex is refused, use rg)"
                    ),
                    json!({"builtin": "grep", "error": "unsupported_option"}),
                );
            }
            for flag in arg[1..].chars() {
                match flag {
                    'i' => ignore_case = true,
                    'v' => invert = true,
                    'c' => counted = true,
                    'n' => numbered = true,
                    'q' => quiet = true,
                    'x' => whole_line = true,
                    'F' => {}
                    'H' => force_names = Some(true),
                    'h' => force_names = Some(false),
                    'f' => {
                        return fail2(
                            "grep: -f pattern files are not supported (use -e)",
                            json!({"builtin": "grep", "error": "unsupported_option"}),
                        );
                    }
                    'e' => {
                        // `-ePATTERN` handled above; bare `-e` at flag-group
                        // end consumes the next arg.
                        index += 1;
                        match args.get(index) {
                            Some(p) => patterns.push(p.as_bytes().to_vec()),
                            None => {
                                return fail2(
                                    "grep: -e needs a pattern",
                                    json!({"builtin": "grep", "error": "usage"}),
                                );
                            }
                        }
                    }
                    _ => unreachable!(),
                }
            }
        } else if arg == "-E"
            || arg == "--extended-regexp"
            || arg == "-G"
            || arg == "--basic-regexp"
        {
            return fail2(
                "grep: regex matching is not implemented (literal only); use rg for patterns",
                json!({"builtin": "grep", "error": "regex_refused"}),
            );
        } else if !pattern_done {
            patterns.push(arg.as_bytes().to_vec());
            pattern_done = true;
        } else {
            operands.push(arg.clone());
        }
        index += 1;
    }
    // Bare `-E`/`-G` are caught by the combined-flag arm above (exit 2);
    // the arm below covers them after the pattern position (`grep foo -E`).
    if patterns.is_empty() {
        return fail2(
            "grep: usage: grep [options] PATTERN [file ...]",
            json!({"builtin": "grep", "error": "usage"}),
        );
    }
    let gathered = gather(&operands, ctx, "grep");
    // Read errors are exit 2, distinct from "no match" (exit 1).
    if !gathered.failures.is_empty() && gathered.chunks.iter().all(|(_, d)| d.is_empty()) {
        let mut result = BuiltinOutput::failed(
            gathered.failures.join("\n"),
            json!({"builtin": "grep", "mode": "literal", "error": "read_failure", "exit": 2}),
        );
        result.code = 2;
        return result;
    }
    let show_names = force_names.unwrap_or(gathered.chunks.len() > 1);
    let mut out = Vec::new();
    let mut matched_lines = 0u64;
    let read_failures = gathered.failures.clone();
    for (name, data) in &gathered.chunks {
        let mut count = 0u64;
        let mut line_number = 0u64;
        let mut selected = Vec::new();
        for line in split_lines(data) {
            line_number += 1;
            let body = line.strip_suffix(b"\n").unwrap_or(&line);
            let hit = patterns
                .iter()
                .any(|p| line_matches(body, p, ignore_case, whole_line));
            let selected_line = if invert { !hit } else { hit };
            if selected_line {
                count += 1;
                matched_lines += 1;
                if quiet || counted {
                    continue;
                }
                if show_names {
                    selected.extend(name.clone().unwrap_or_default().as_bytes());
                    selected.push(b':');
                }
                if numbered {
                    selected.extend(format!("{line_number}:").as_bytes());
                }
                selected.extend_from_slice(&line);
            }
        }
        if counted && !quiet {
            if show_names {
                out.extend(name.clone().unwrap_or_default().as_bytes());
                out.push(b':');
            }
            out.extend(format!("{count}\n").as_bytes());
        } else if !quiet {
            out.extend_from_slice(&selected);
        }
    }
    if quiet {
        let truth = json!({"builtin": "grep", "mode": "literal", "matched_lines": matched_lines,
                           "quiet": true, "failures": read_failures.len()});
        if matched_lines > 0 {
            return BuiltinOutput::ok(Vec::new(), truth);
        }
        let mut result = BuiltinOutput::failed(String::new(), truth);
        result.stderr = Vec::new();
        result.code = 1;
        return result;
    }
    let truth = json!({"builtin": "grep", "mode": "literal", "matched_lines": matched_lines,
                       "bytes": out.len(), "failures": read_failures.len()});
    if matched_lines > 0 {
        if read_failures.is_empty() {
            BuiltinOutput::ok(out, truth)
        } else {
            let mut result = BuiltinOutput::failed(read_failures.join("\n"), truth);
            result.stdout = out;
            result
        }
    } else {
        let mut result = BuiltinOutput::failed(read_failures.join("\n"), truth);
        result.stdout = out;
        result.code = if read_failures.is_empty() { 1 } else { 2 };
        if read_failures.is_empty() {
            result.stderr = Vec::new();
        }
        result
    }
}

fn line_matches(line: &[u8], pattern: &[u8], ignore_case: bool, whole_line: bool) -> bool {
    if whole_line {
        return if ignore_case {
            line.eq_ignore_ascii_case(pattern)
        } else {
            line == pattern
        };
    }
    if pattern.is_empty() {
        return true;
    }
    if !ignore_case {
        return line.windows(pattern.len()).any(|w| w == pattern);
    }
    line.windows(pattern.len())
        .any(|w| w.eq_ignore_ascii_case(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BuiltinContext;

    fn ctx_with(stdin: &[u8]) -> BuiltinContext {
        BuiltinContext {
            cwd: std::path::PathBuf::from("."),
            env: Vec::new(),
            stdin: stdin.to_vec(),
        }
    }

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn head_takes_first_n_lines() {
        let ctx = ctx_with(b"a\nb\nc\nd\n");
        let out = head(&s(&["-n", "2"]), &ctx);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b"a\nb\n");
    }

    #[test]
    fn head_negative_count_drops_trailing_lines() {
        let ctx = ctx_with(b"a\nb\nc\nd\n");
        let out = head(&s(&["-n", "-1"]), &ctx);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b"a\nb\nc\n");
    }

    #[test]
    fn tail_takes_last_n_lines_and_preserves_no_trailing_newline() {
        let ctx = ctx_with(b"a\nb\nc");
        let out = tail(&s(&["-n", "2"]), &ctx);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b"b\nc");
    }

    #[test]
    fn tail_from_line_form() {
        let ctx = ctx_with(b"a\nb\nc\n");
        let out = tail(&s(&["-n", "+2"]), &ctx);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b"b\nc\n");
    }

    #[test]
    fn wc_counts_lines_words_bytes() {
        let ctx = ctx_with(b"hello world\nfoo\n");
        let out = wc(&s(&[]), &ctx);
        assert_eq!(out.code, 0);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "      2      3     16\n"
        );
    }

    #[test]
    fn wc_refuses_char_count_over_bytes() {
        let ctx = ctx_with(b"x\n");
        let out = wc(&s(&["-m"]), &ctx);
        assert_eq!(out.code, 1);
    }

    #[test]
    fn sort_numeric_unique_reverse() {
        let ctx = ctx_with(b"10\n2\n2\n30\n");
        let out = sort(&s(&["-n", "-u"]), &ctx);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b"2\n10\n30\n");
        let out = sort(&s(&["-r"]), &ctx_with(b"b\na\n"));
        assert_eq!(out.stdout, b"b\na\n");
    }

    #[test]
    fn sort_is_byte_exact_over_non_utf8() {
        let ctx = ctx_with(b"b\xff\na\n");
        let out = sort(&s(&[]), &ctx);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b"a\nb\xff\n");
    }

    #[test]
    fn uniq_counts_and_filters_groups() {
        let ctx = ctx_with(b"a\na\nb\n");
        let out = uniq(&s(&["-c"]), &ctx);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b"      2 a\n      1 b\n");
        let out = uniq(&s(&["-d"]), &ctx_with(b"a\na\nb\n"));
        assert_eq!(out.stdout, b"a\n");
        let out = uniq(&s(&["-u"]), &ctx_with(b"a\na\nb\n"));
        assert_eq!(out.stdout, b"b\n");
    }

    #[test]
    fn missing_file_fails_but_keeps_good_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let good = dir.path().join("good.txt");
        std::fs::write(&good, b"kept\n").expect("write");
        let ctx = BuiltinContext {
            cwd: dir.path().to_path_buf(),
            env: Vec::new(),
            stdin: Vec::new(),
        };
        let out = head(&s(&["good.txt", "absent.txt"]), &ctx);
        assert_eq!(out.code, 1);
        assert_eq!(out.stdout, b"kept\n");
        assert!(String::from_utf8_lossy(&out.stderr).contains("absent.txt"));
    }

    #[test]
    fn unknown_options_fail_predictably() {
        let ctx = ctx_with(b"");
        assert_eq!(sort(&s(&["-k"]), &ctx).code, 1);
        assert_eq!(uniq(&s(&["-s"]), &ctx).code, 1);
        assert_eq!(wc(&s(&["-L"]), &ctx).code, 1);
    }

    #[test]
    fn tee_passes_bytes_and_refuses_files() {
        let out = tee(&s(&[]), &ctx_with(b"a\0b\n"));
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b"a\0b\n");
        assert_eq!(out.truth["mode"], serde_json::json!("passthrough"));
        let out = tee(&s(&["-a"]), &ctx_with(b"x"));
        assert_eq!(out.stdout, b"x");
        // File operands refuse closed: no write, no stdout leak.
        let out = tee(&s(&["out.txt"]), &ctx_with(b"data"));
        assert_eq!(out.code, 1);
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains("admitted filesystem authority"));
        assert_eq!(tee(&s(&["-i"]), &ctx_with(b"")).code, 1);
    }

    #[test]
    fn cut_bytes_fields_and_merged_ranges() {
        let ctx = ctx_with(b"abcdef\n");
        let out = cut(&s(&["-b", "1,3"]), &ctx);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b"ac\n");
        // Overlapping ranges emit once (GNU merge).
        let out = cut(&s(&["-b", "1-2,2-3"]), &ctx);
        assert_eq!(out.stdout, b"abc\n");
        // Fields with -s suppression.
        let ctx = ctx_with(b"a:b:c\nplain\n");
        let out = cut(&s(&["-f", "2", "-d", ":"]), &ctx);
        assert_eq!(out.stdout, b"b\nplain\n");
        let out = cut(
            &s(&["-f", "2", "-d", ":", "-s"]),
            &ctx_with(b"a:b:c\nplain\n"),
        );
        assert_eq!(out.stdout, b"b\n");
        // Only one mode; bad ranges fail.
        assert_eq!(cut(&s(&["-b", "1", "-f", "1"]), &ctx).code, 1);
        assert_eq!(cut(&s(&["-b", "3-1"]), &ctx).code, 1);
        assert_eq!(cut(&s(&["-b", "0"]), &ctx).code, 1);
    }

    #[test]
    fn tr_translates_deletes_squeezes() {
        let out = tr(&s(&["a-z", "A-Z"]), &ctx_with(b"hello\n"));
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b"HELLO\n");
        let out = tr(&s(&["-d", "aeiou"]), &ctx_with(b"hello\n"));
        assert_eq!(out.stdout, b"hll\n");
        let out = tr(&s(&["-s", "l"]), &ctx_with(b"hello\n"));
        assert_eq!(out.stdout, b"helo\n");
        // Complement of everything-but-newline deletes letters, keeps newline.
        let out = tr(&s(&["-c", "-d", "\\n"]), &ctx_with(b"ab\ncd\n"));
        assert_eq!(out.stdout, b"\n\n");
        // Files refused: stdin only.
        assert_eq!(tr(&s(&["a", "b", "file"]), &ctx_with(b"")).code, 1);
        assert_eq!(tr(&s(&["a"]), &ctx_with(b"x")).code, 1);
    }

    #[test]
    fn grep_literal_match_exit_codes() {
        let ctx = ctx_with(b"foo bar\nbaz\n");
        let out = grep(&s(&["foo"]), &ctx);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b"foo bar\n");
        // No match: exit 1, no stderr.
        let out = grep(&s(&["zzz"]), &ctx);
        assert_eq!(out.code, 1);
        assert!(out.stderr.is_empty());
        // Invert, count, numbers, whole-line, casefold.
        assert_eq!(grep(&s(&["-v", "foo"]), &ctx).stdout, b"baz\n");
        assert_eq!(grep(&s(&["-c", "a"]), &ctx).stdout, b"2\n");
        assert_eq!(grep(&s(&["-n", "baz"]), &ctx).stdout, b"2:baz\n");
        assert_eq!(grep(&s(&["-x", "foo"]), &ctx).code, 1);
        assert_eq!(grep(&s(&["-i", "FOO"]), &ctx).stdout, b"foo bar\n");
        assert_eq!(grep(&s(&["-q", "baz"]), &ctx).code, 0);
        assert!(grep(&s(&["-q", "zzz"]), &ctx).stdout.is_empty());
        // Regex refused (exit 2), missing pattern is usage (exit 2).
        assert_eq!(grep(&s(&["-E", "f.o"]), &ctx).code, 2);
        assert_eq!(grep(&s(&[]), &ctx).code, 2);
        // Truth declares the literal limitation.
        let out = grep(&s(&["foo"]), &ctx);
        assert_eq!(out.truth["mode"], serde_json::json!("literal"));
    }

    #[test]
    fn grep_missing_file_is_exit_2() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = BuiltinContext {
            cwd: dir.path().to_path_buf(),
            env: Vec::new(),
            stdin: Vec::new(),
        };
        let out = grep(&s(&["x", "absent.txt"]), &ctx);
        assert_eq!(out.code, 2);
    }
}
