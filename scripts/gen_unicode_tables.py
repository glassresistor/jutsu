"""Generate crates/jutsu-core/src/unicode_tables.rs.

The rust lexer needs character classes that match CPython's `re` semantics
exactly (\\s, \\d) plus the identifier class jinja generated for its name rule.
Run from the repository root::

    python3 scripts/gen_unicode_tables.py
"""
import re
import sys

sys.path.insert(0, "src")

from jutsu._identifier import pattern as name_re  # noqa: E402

MAX = 0x110000


def ranges_for(comp):
    out = []
    in_set = False
    start = None
    for cp in range(MAX):
        ok = comp.match(chr(cp)) is not None
        if ok and not in_set:
            in_set = True
            start = cp
        elif not ok and in_set:
            in_set = False
            out.append((start, cp - 1))
    if in_set:
        out.append((start, MAX - 1))
    return out


def fmt(rs):
    return ",\n".join(f"    ({a}, {b})" for a, b in rs)


ident = ranges_for(name_re)
ws = ranges_for(re.compile(r"\s"))
digit = ranges_for(re.compile(r"\d"))

src = f'''//! Unicode character tables generated from CPython's `re` semantics.
//! See `scripts/gen_unicode_tables.py`. Do not edit by hand.

/// Ranges of characters matched by the jinja name rule (`[\\w...]` class).
pub static IDENT_CHARS: &[(u32, u32)] = &[
{fmt(ident)}
];

/// Characters matching Python's `\\s` for str patterns.
pub static WS_CHARS: &[(u32, u32)] = &[
{fmt(ws)}
];

/// Characters matching Python's `\\d` for str patterns.
pub static DIGIT_CHARS: &[(u32, u32)] = &[
{fmt(digit)}
];

pub fn in_ranges(cp: u32, rs: &[(u32, u32)]) -> bool {{
    let (mut lo, mut hi) = (0usize, rs.len());
    if hi == 0 {{
        return false;
    }}
    hi -= 1;
    while lo <= hi {{
        let mid = (lo + hi) / 2;
        let (a, b) = rs[mid];
        if cp < a {{
            if mid == 0 {{
                break;
            }}
            hi = mid - 1;
        }} else if cp > b {{
            lo = mid + 1;
        }} else {{
            return true;
        }}
    }}
    false
}}

pub fn is_ident(c: char) -> bool {{
    in_ranges(c as u32, IDENT_CHARS)
}}

pub fn is_ws(c: char) -> bool {{
    in_ranges(c as u32, WS_CHARS)
}}

pub fn is_digit(c: char) -> bool {{
    in_ranges(c as u32, DIGIT_CHARS)
}}
'''

with open("crates/jutsu-core/src/unicode_tables.rs", "w") as fh:
    fh.write(src)

print(
    f"wrote unicode_tables.rs: {len(ident)} ident ranges,"
    f" {len(ws)} ws ranges, {len(digit)} digit ranges"
)
