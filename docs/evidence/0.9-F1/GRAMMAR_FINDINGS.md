# 0.9-F1 Grammar Findings — No Second Grammar

## Verdict

**F1 introduces no new runtime syntax.** Completion assists editing only.
Canonical executable text remains canonical executable text.

## Accepted grammar is argv-only

On accepted main (`888d381`), `GrammarScanner::scan` classifies input into
three lanes and `split_words` produces argv. The following are **literal argv
text**, not operators:

| Token | Accepted meaning on main | F1 completion behaviour |
|-------|--------------------------|-------------------------|
| `\|` | literal word character | never treated as a pipe; never completed |
| `>` | literal word character | never completed as redirection |
| `>>` | literal word character | never completed as append |
| `2>&1` | one literal argv word | never completed as redirection |
| `&&` | literal word characters | never completed |
| `;` | literal word character | never completed |

Proof: `f1_redirection_and_pipes_are_never_completed` and
`f1_pipe_characters_are_literal_argv_not_operators`.

The earlier packet's "explain a phrase then insert `2>&1`" example was
illustrative of the editing-vs-runtime principle, **not** permission to add
unsupported grammar. F1 does not implement pipes or redirection.

## ONE grammar, with spans

`scan_words_with_spans` is the accepted argv scanner enriched with:

- raw byte span
- decoded literal
- quote state
- quote-unclosed flag

`GrammarScanner::split_words` is a **projection** of that scanner (empty
literals dropped, matching accepted-main behaviour exactly).

Proof: `f1_grammar_span_scanner_projects_split_words` and the existing
`h2_grammar_tests` / `windows_path_regression_tests` which pass unchanged.

Cursor analysis (`token_at_cursor`) replays the same state machine and
produces `decoded_prefix` / `decoded_suffix` / `quote_at_cursor` /
`split_unsafe`. Escape pairs that straddle the cursor mark `split_unsafe` and
completion declines.

## Quoting must round-trip through the real grammar

Required invariant:

```
candidate literal -> quote/insert -> canonical text -> accepted parser -> same literal
```

Implemented as `quote_literal` + `round_trips`, and enforced mechanically in
`build_edit` (every offered edit is decoded back before it is returned).

### Representable

- plain filename
- spaces
- apostrophe (`"it's.txt"`)
- double quote (`'say "hi".txt'` or `"say \"hi\".txt"`)
- unicode
- Windows backslashes (`C:\Users\Matmus\project` unquoted)
- spaces + backslashes (`"C:\Program Files\App"`)
- both apostrophe and double quote (`"both ' and \".txt"`)
- `&`, `2>&1.txt` as literal name text

### Grammar limitations (declined, not corrupted)

- **empty literal** — `""` / `''` decode to a dropped word in the accepted
  grammar. Unrepresentable. Completion declines.
- **apostrophe + trailing backslash** (`it's\`) — cannot use single quotes
  (contains `'`); double quotes cannot escape a trailing `\` (the closing
  quote would be consumed as `\"`). Unrepresentable. `quote_literal` returns
  `None`; `round_trips` returns false.

UNKNOWN / UNSUPPORTED is better than corrupt argv.

## Mid-token editing

Decoded model: `decoded_prefix` + cursor + `decoded_suffix`.

A candidate is non-destructively insertable when it is compatible with BOTH
halves. Only the missing middle is inserted at the cursor.

- `file|.txt` + `fileX.txt` -> inserts `X`, suffix `.txt` preserved.
- `foo|bar` + `foobaz` -> refused (suffix `bar` not preserved).
- `file|.txt` + `file long.txt` -> refused unquoted (middle ` long` needs
  quote surgery around `.txt`). Allowed inside an existing double quote where
  the middle encodes without touching the right-hand text.
- `'file with spa|ces.txt'` + compatible candidate -> inserts only the missing
  middle; `ces.txt'` survives byte-for-byte.
- open single quote + candidate containing `'` -> declined (would require
  quote surgery).

End-of-token unquoted may replace the typed span with a freshly constructed
canonical token (quoting may be introduced). End-of-token inside an open quote
inserts the remaining middle (plus a closing quote when the quote is unclosed).

No destructive mid-token replace mode exists in F1.
