#!/usr/bin/env python3
"""Drop redundant `any()` erasure and turn vec-list containers into builder chains.

A checked-in, run-manually codemod for frust's erasure-at-the-API-boundary
migration. It works at the token level on the original text, so comments,
blank lines and formatting outside the rewritten spans survive untouched (run
`cargo fmt` afterwards to reflow the rewritten chains). Standard library only.

Rewrites (each one is type-preserving, or changes only an argument's type):

  T1  `api(.., any(X), ..)` -> `api(.., X, ..)` and `api(Some(any(X)))` ->
      `api(Some(X))`, when `api` is a slot entry of the name list
      (`erasing_apis.txt`, next to this script) and the erasure call is the
      *whole* argument. A `vec![..]` argument is never touched.
  T4  `api(.., || any(X), ..)` -> `api(.., || X, ..)` when `api` is a builder
      entry of the name list and the closure body is exactly one erasure call,
      optionally wrapped in a block (`|| { any(X) }` -> `|| { X }`). Closures
      with a `-> Type` annotation or any other statement are left alone.
  T2  `Column(vec![a, b])` -> `column().child(a').child(b')` (`Row` -> `row()`,
      `Stack` -> `stack()`), where `a'` drops a top-level erasure wrapper.
  T3  `FlexView::new(Axis::Vertical | Axis::Horizontal, vec![..])` ->
      `column()` / `row()` chained: `inflexible(X)` -> `.child(X')`,
      `flexible(N, X)` -> `.flex(N, X')`, `keyed(K, X)` -> `.keyed(K, X')`,
      anything else -> `.push(elem)`. A list mixing literal `keyed(..)` with
      literal `inflexible`/`flexible` children is left alone (keyed flex lists
      are all-or-nothing) and reported as a note.
  T5  (opt-in, `--t5` only) `api(.., vec![any(E1), .., any(En)], ..)` ->
      `api(.., vec![E1, .., En], ..)` when the `vec![..]` literal (n >= 1) is a
      whole argument of a list entry of the name list, of `Column`/`Row`/
      `Stack` sugar, or of `.children`, every element is one erasure call
      (`any(E)`, `frust..::any(E)`, `authoring::any(E)`, `AnyView::new(E)`),
      and all E_i share the same *head*. The rewrite only drops the wrappers:
      element text, trailing commas and comments between elements are kept.
      A `Column`/`Row`/`Stack` site T2 already rewrites is not listed again.
  T7  (opt-in, `--t7` only) a helper fn (free, nested, or an inherent-impl
      method) `fn f(..) -> AnyView<S> { ..; any(E) }` -> `fn f(..) -> impl
      View<S> { ..; E }`: the return type is `AnyView<T>`, bare or path-qualified
      (`frust::..::AnyView<T>`, `authoring::AnyView<T>`), and the tail — the
      last expression of the body, after any `let`s and statements, or a final
      `return any(E);` — is exactly one erasure call (`any(E)`,
      `frust..::any(E)`, `authoring::any(E)`, `AnyView::new(E)`). The new return
      type is qualified the way `AnyView` was (`impl frust::View<T>`); for a
      bare `AnyView` a missing `View` import is added next to the `AnyView` one
      (or, in a `use super::*` test module, as its own `use`), and an `AnyView`/
      `any` import the rewrite left unused is removed. Async and const fns are
      never touched.

T7 captures: edition 2024 `-> impl Trait` captures every in-scope generic
parameter and lifetime, and `View<S>: 'static` keeps a captured borrow from
outliving its use, so nothing is added. In an edition 2015-2021 crate, when the
fn (or its impl) has a lifetime parameter or the fn takes an elided reference
(`&self`, `&State`), `+ use<..>` lists every lifetime (`'_` for elided ones)
and every type/const parameter in scope; a fn that also takes an
argument-position `impl Trait` (which `use<..>` cannot name) is skipped with a
note. The edition comes from the crate's `Cargo.toml` (`edition.workspace =
true` resolves through the nearest `[workspace]` manifest; no `edition` key is
2015; a file outside any package counts as 2024).

T7 exclusions — listed by `--check --t7` as `<bucket> excluded: <fn> (<why>)`,
never rewritten, never counted as candidates (they do not affect the exit
code); the first matching bucket wins:
  T7-trait   a trait method declaration (with or without a default body) or a
             method of a trait impl: the signature is the trait's.
  T7-arms    an `if`/`else` chain or a `match` as the tail, or an early
             `return` anywhere in the body: several exits of different types
             (`Either` candidates, not T7's).
  T7-ref     a recursive fn (`f(..)` in a free fn, `Self::f(..)`/`self.f(..)`
             in a method), or a fn named as a value — a fn pointer, a stored
             callback, an argument of `Box::new`/`.map(..)`/`.push(..)` —
             anywhere in its package (the `Cargo.toml` directory's `*.rs`
             files, nested packages and `--exclude`d paths left out). The
             value-use scan is a heuristic that errs towards a hit: a local
             binding or pattern of the same name does not count, a
             type-qualified `Kind::f` only counts against a method.
  T7-public  a `pub fn` (a restricted `pub(..)` is not API) in a `crates/` or
             `plugins/` library source — not under a `tests/`, `examples/` or
             `src/corpus/` directory, and not inside a `#[cfg(test)] mod`:
             changing a library signature is a human decision.
A tail that is neither one erasure call nor an `if`/`match` (a `vec![..]`, a
loop, an accumulator, a call of another helper, a turbofish `any::<S, _>(..)`)
is neither rewritten nor listed.

T5 heuristic: the head of `E` is the full callee path of its outermost call
plus the exact sequence of trailing method-call names, so `text(a).size(1)`
and `text(b).size(2)` match while `text(a)` and `text(b).size(2)` (or
`text(a)` and `frust::text(b)`) do not. An element that is not
`path(..)` followed only by `.name(..)` calls (a variable, a macro, a
turbofish, a field access, `?`, `.await`) has no head, so its list is left
alone, as is a list with any non-erasure element or with two different heads.
Failure mode: T5 judges heads, not types. A same-head list whose elements are
not type-homogeneous — a generic callee whose result type follows its
arguments (`component(Header{..})` vs `component(Counter{..})`,
`breadcrumb_item(a)` vs `breadcrumb_item(b)`), or a builder method that
changes the type — is still flagged, and dropping the `any()` does not compile
(`Vec<V>` needs one `V`). For such a list keep `any()` and mark the `vec![`
line with `// erasure: keep <why>`; the alternatives are binding the vec to a
local or using the fluent builder (`.child(..)` chain).

Opt-out: a line comment `// erasure: keep` (exact text after `//`, optional
trailing reason) on the same line as the start of a candidate's span, or alone
on the line immediately above it, exempts that site from every rule (T1-T7).
For T7 the site is the signature: the marker goes on the `fn` line or the `->`
line, or alone on the line above the `fn` line.
Kept sites are never listed by `--check` and never rewritten; `--stats`
counts them as `kept=N`. T5 applies `Column`/`Row`/`Stack` only when T2
would resolve them to frust's own sugar (a local `struct Row` or a foreign
`ui::Row` is left alone).

Erasure calls are bare `any(X)`, `frust..::any(X)`, and `AnyView::new(X)`
(optionally path-qualified). A turbofish erasure (`any::<S, _>(X)`) is never
stripped: it carries the `State` type the surrounding code may rely on. A file
that defines its own non-view `fn any` disables bare `any(` erasure. Nothing
inside an attribute (`#[cfg(any(..))]`) or a macro other than `vec!` is
rewritten; `.any(..)` iterator calls are method calls and never erasure.

`use` declarations are kept compiling: a T2/T3 chain adds `column`/`row`/
`stack` next to the `Column`/`Row`/`Stack`/`FlexView` import it came from, and
an import (`any`, `AnyView`, `Column`, `Row`, `Stack`, `FlexView`, `Axis`,
`inflexible`, `flexible`, `keyed`) that a rewrite left unused is removed from
the (non-`pub`) `use` that brought it in. A chain is skipped, with a note, when
a local binding or `fn` (free or method) named `column`/`row`/`stack` is in
scope (unless that name is itself imported by a `use`), or the builder's import
cannot be resolved.

Usage:

    python3 scripts/codemod/frust_any_codemod.py [--check] [--write] [--stats]
        [--strict] [--t5] [--t7] [--apis FILE] [--exclude PATH]... PATH...

PATH is a `.rs` file or a directory (recursed for `*.rs`, skipping `target/`).
`--check` (the default) lists `file:line` of every remaining candidate and
exits 1 if there is one, 0 otherwise; skipped-site notes do not affect the exit
code. A file that cannot be tokenised or decoded is reported as skipped, counted,
and makes `--check` exit 2 when no candidate was found (candidates still win: 1).
The summary always reports the number of shadow-skipped sites (a local binding
or fn shadows the builder, a mixed keyed flex list, an unresolvable import, or a
comment inside the replaced call head); `--strict` makes any of them exit 2
under `--check` (candidates still win: 1).
`--exclude PATH` (repeatable; repository-relative or absolute) drops the
file or directory, matched by whole path components on the normalised path
(`a/src` does not exclude `a/src2`), from `--check`, `--write` and `--stats`;
excluded files are neither read nor counted. `--write` rewrites in place
atomically (temp file in the same directory, mode copied, fsync, then
`os.replace`), iterating to a fixpoint, so a second run is a no-op. Symlinked
`.rs` files are skipped in directory walks and refused (exit 2) as explicit
PATH arguments. `--stats` prints per-file erasure-call counts before/after.
`--t5` turns rule T5 on for every mode (off by default): `--check --t5` also
lists `T5 homogeneous list argument of <callee>` candidates, `--write --t5`
applies them, and `--stats --t5` additionally reports the T5 candidate count
per file (when non-zero) and in total, separately from the erasure counts.
`--t7` likewise turns rule T7 on for every mode (off by default): `--check
--t7` also lists `T7 helper returns impl View: <fn>` candidates and the T7
exclusions (with a `N T7 exclusion(s)` summary line), `--write --t7` applies
the candidates, and `--stats --t7` reports the T7 candidate count and the four
exclusion buckets (`T7-trait`, `T7-arms`, `T7-ref`, `T7-public`) per file
(when non-zero) and in total.

Known blind spot: a closure parameter typed `AnyView<..>` whose body is
`any(x)` is still a T1/T4 candidate when the call sits in a slot position; the
rewrite then fails to compile (the closure must return the erased type), and
the compiler, not this tool, catches it.

Known T7 limitation: T7 changes a helper's return type but not its callers. A
call site that relied on the erased type — the tail of another `-> AnyView<S>`
fn, an element of a `Vec<AnyView<S>>`/`vec![..]`, an `if`/`match` arm, a
`let v: AnyView<S>` — no longer compiles; erase there with `any(f(..))` (a
caller whose tail then becomes one erasure call is itself a T7 candidate on the
next run) or mark the helper `// erasure: keep <why>`. The compiler, not this
tool, finds those sites.
"""

from __future__ import annotations

import argparse
import bisect
import contextlib
import os
import re
import shutil
import sys
import tempfile
from dataclasses import dataclass, field

# ---------------------------------------------------------------------------
# Lexer
# ---------------------------------------------------------------------------

WS, LINE_COMMENT, BLOCK_COMMENT, IDENT, LIFETIME, CHAR, STR, NUM, PUNCT = range(9)
TRIVIA = (WS, LINE_COMMENT, BLOCK_COMMENT)

KEYWORDS = {
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
    "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop",
    "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self",
    "static", "struct", "super", "trait", "true", "type", "union", "unsafe",
    "use", "where", "while", "yield",
}

_RAW_STR = re.compile(r'(?:br|cr|r)(#*)"')
_IDENT = re.compile(r"(?:r#)?[^\W\d]\w*")
_NUM = re.compile(r"\d\w*(?:\.\d\w*)?")


class LexError(Exception):
    pass


@dataclass
class Tok:
    kind: int
    start: int
    end: int
    text: str


def lex(src: str) -> list[Tok]:
    toks: list[Tok] = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        kind = PUNCT
        if c.isspace():
            j = i + 1
            while j < n and src[j].isspace():
                j += 1
            kind = WS
        elif src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            kind = LINE_COMMENT
        elif src.startswith("/*", i):
            depth, j = 1, i + 2
            while depth and j < n:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            if depth:
                raise LexError(f"unterminated block comment at offset {i}")
            kind = BLOCK_COMMENT
        else:
            m = _RAW_STR.match(src, i)
            if m:
                close = '"' + m.group(1)
                j = src.find(close, m.end())
                if j < 0:
                    raise LexError(f"unterminated raw string at offset {i}")
                j += len(close)
                kind = STR
            elif c == '"' or (c in "bc" and src.startswith('"', i + 1)):
                j = _scan_quoted(src, i if c == '"' else i + 1, '"')
                kind = STR
            elif c == "b" and src.startswith("'", i + 1):
                j = _scan_quoted(src, i + 1, "'")
                kind = CHAR
            elif c == "'":
                if src.startswith("\\", i + 1):
                    j = src.find("'", i + 3)
                    if j < 0:
                        raise LexError(f"unterminated char literal at offset {i}")
                    j += 1
                    kind = CHAR
                elif i + 2 < n and src[i + 2] == "'":
                    j, kind = i + 3, CHAR
                else:
                    m = _IDENT.match(src, i + 1)
                    j, kind = (m.end(), LIFETIME) if m else (i + 1, PUNCT)
            else:
                m = _IDENT.match(src, i)
                if m:
                    j, kind = m.end(), IDENT
                elif c.isdigit():
                    j, kind = _NUM.match(src, i).end(), NUM
                elif src.startswith(("::", "->", "=>"), i):
                    j = i + 2
                else:
                    j = i + 1
        toks.append(Tok(kind, i, j, src[i:j]))
        i = j
    return toks


def _scan_quoted(src: str, q: int, quote: str) -> int:
    j, n = q + 1, len(src)
    while j < n:
        if src[j] == "\\":
            j += 2
        elif src[j] == quote:
            return j + 1
        else:
            j += 1
    raise LexError(f"unterminated literal at offset {q}")


# ---------------------------------------------------------------------------
# Token-tree view of one file
# ---------------------------------------------------------------------------

OPEN = {"(": ")", "[": "]", "{": "}"}
CLOSE = {v: k for k, v in OPEN.items()}
ROOT_PREFIX_OK = ("crate", "super", "self")
REMOVABLE_IMPORTS = (
    "any", "AnyView", "Column", "Row", "Stack", "FlexView", "Axis",
    "inflexible", "flexible", "keyed",
)
BUILDER_OF = {"Column": "column", "Row": "row", "Stack": "stack"}


@dataclass
class Leaf:
    path: tuple            # full import path segments, e.g. ("frust", "Column")
    name: str              # local name (alias when renamed)
    aliased: bool
    item: "Item"


@dataclass
class Item:
    start: int             # code-token index range of this tree item
    end: int
    group: "Group | None" = None   # set when the item ends in `{..}`
    is_glob: bool = False


@dataclass
class Group:
    open: int
    close: int
    items: list = field(default_factory=list)


@dataclass
class UseDecl:
    ordinal: int
    use_idx: int           # code index of the `use` keyword
    stmt_start: int        # code index of the first token (`pub` or `use`)
    semi: int              # code index of `;`
    is_pub: bool
    root: Item
    leaves: list
    globs: list            # path prefixes of `*` imports
    scope: tuple           # (start_char, end_char)


class Source:
    def __init__(self, src: str):
        self.src = src
        self.all = lex(src)
        self.t = [tok for tok in self.all if tok.kind not in TRIVIA]
        t = self.t
        self.match: dict[int, int] = {}
        self.parent = [-1] * len(t)
        stack: list[int] = []
        for k, tok in enumerate(t):
            self.parent[k] = stack[-1] if stack else -1
            if tok.kind != PUNCT:
                continue
            if tok.text in OPEN:
                stack.append(k)
            elif tok.text in CLOSE:
                if not stack or t[stack[-1]].text != CLOSE[tok.text]:
                    raise LexError(f"unbalanced {tok.text!r} at offset {tok.start}")
                o = stack.pop()
                self.match[o], self.match[k] = k, o
                self.parent[k] = self.parent[o]
        if stack:
            raise LexError(f"unclosed {t[stack[-1]].text!r} at offset {t[stack[-1]].start}")
        self.group_kind = {o: self._classify_group(o) for o in self.match if t[o].text in OPEN}
        self.line_starts = [0] + [m.end() for m in re.finditer("\n", src)]
        self.line_comment_ends = {tok.end for tok in self.all if tok.kind == LINE_COMMENT}
        self.keep_lines, self.keep_above = self._keep_markers()
        self.kept = 0
        self.t7_excluded: list = []   # (line, bucket, fn name, reason), set by T7
        self.bare_any_ok = not self._defines_foreign_any()
        self.decls = self._parse_uses()
        self.local_types = self._local_type_names()
        self.local_fns = {t[k + 1].text for k in range(len(t) - 1)
                          if t[k].text == "fn" and t[k + 1].kind == IDENT}

    def _keep_markers(self):
        """Lines carrying a `// erasure: keep` comment, from the comment tokens.

        Returns (every marker line, marker lines whose comment is the only thing
        on its line). The marker is the exact text `erasure: keep` right after
        `//`, ending the comment or followed by whitespace and a reason."""
        anywhere, alone = set(), set()
        for tok in self.all:
            if tok.kind != LINE_COMMENT or not tok.text.startswith("//"):
                continue
            body = tok.text[2:].lstrip(" \t")
            if body != "erasure: keep" and not re.match(r"erasure: keep[ \t]", body):
                continue
            line = self.line_of(tok.start)
            anywhere.add(line)
            if not self.src[self.line_starts[line - 1]:tok.start].strip():
                alone.add(line)
        return anywhere, alone

    def is_kept(self, line: int) -> bool:
        """True when a site starting on `line` carries the opt-out marker: on
        that line, or alone on the line immediately above."""
        return line in self.keep_lines or (line - 1) in self.keep_above

    # -- basic helpers -----------------------------------------------------

    def is_p(self, k: int, text: str) -> bool:
        return 0 <= k < len(self.t) and self.t[k].kind == PUNCT and self.t[k].text == text

    def is_id(self, k: int, text: str | None = None) -> bool:
        if not (0 <= k < len(self.t)) or self.t[k].kind != IDENT:
            return False
        return text is None or self.t[k].text == text

    def line_of(self, pos: int) -> int:
        return bisect.bisect_right(self.line_starts, pos)

    def indent_of(self, pos: int) -> str:
        ls = self.line_starts[self.line_of(pos) - 1]
        m = re.match(r"[ \t]*", self.src[ls:])
        return m.group(0)

    def text(self, a: int, b: int) -> str:
        return self.src[self.t[a].start:self.t[b].end]

    def _classify_group(self, o: int):
        t = self.t
        if t[o].text == "[" and (self.is_p(o - 1, "#") or (self.is_p(o - 1, "!") and self.is_p(o - 2, "#"))):
            return "attr"
        if self.is_p(o - 1, "!") and self.is_id(o - 2) and t[o - 2].text not in KEYWORDS:
            return "macro:" + t[o - 2].text
        if t[o].text == "{" and self.is_id(o - 1) and self.is_p(o - 2, "!") and self.is_id(o - 3, "macro_rules"):
            return "macro:macro_rules"
        return None

    def forbidden(self, k: int) -> bool:
        """True when token `k` sits in an attribute or a non-`vec!` macro."""
        g = self.parent[k]
        while g >= 0:
            kind = self.group_kind.get(g)
            if kind == "attr" or (kind and kind != "macro:vec"):
                return True
            g = self.parent[g]
        return False

    def commas(self, o: int) -> list[int]:
        """Top-level comma indices of the group opened at `o`.

        Commas inside a turbofish (`f::<A, B>(..)`) are not separators."""
        out, angle = [], 0
        for k in range(o + 1, self.match[o]):
            if self.parent[k] != o:
                continue
            if self.is_p(k, "<") and (angle or self.is_p(k - 1, "::")):
                angle += 1
            elif self.is_p(k, ">") and angle:
                angle -= 1
            elif self.is_p(k, ",") and not angle:
                out.append(k)
        return out

    def split_args(self, o: int) -> list[tuple[int, int]]:
        """Top-level comma-separated arguments of the group opened at `o`."""
        c = self.match[o]
        args, start = [], o + 1
        for k in self.commas(o):
            if start <= k - 1:
                args.append((start, k - 1))
            start = k + 1
        if start <= c - 1:
            args.append((start, c - 1))
        return args

    def path_before(self, k: int) -> list[int]:
        """Code indices of the `seg ::` path segments directly before ident `k`."""
        segs = []
        j = k - 1
        while self.is_p(j, "::") and self.is_id(j - 1):
            segs.insert(0, j - 1)
            j -= 2
        return segs

    def scope_of(self, k: int) -> tuple[int, int]:
        g = self.parent[k]
        while g >= 0 and self.t[g].text != "{":
            g = self.parent[g]
        if g < 0:
            return (0, len(self.src))
        return (self.t[g].start, self.t[self.match[g]].end)

    # -- erasure recognition -------------------------------------------------

    def erasure_call(self, a: int, b: int):
        """Return the `(` index when tokens a..b are exactly one erasure call."""
        t = self.t
        k = a
        if self.is_p(k, "::"):
            k += 1
        idents = []
        while self.is_id(k):
            idents.append(k)
            if self.is_p(k + 1, "::") and self.is_id(k + 2):
                k += 2
            else:
                break
        if not idents or not self.is_p(idents[-1] + 1, "("):
            return None
        o = idents[-1] + 1
        if self.match[o] != b:
            return None
        names = [t[i].text for i in idents]
        if names[-1] == "any":
            if len(names) == 1:
                if not self.bare_any_ok:
                    return None
            elif not _frust_root(names[0]):
                return None
        elif names[-2:] == ["AnyView", "new"]:
            if len(names) > 2 and not _frust_root(names[0]):
                return None
        else:
            return None
        if len(self.split_args(o)) != 1:
            return None
        return o

    def inner_text(self, o: int) -> str:
        """Text of the single argument of the call opened at `o`, trimmed."""
        c = self.match[o]
        end_tok = c
        last = c - 1
        if last > o and self.is_p(last, ",") and self.parent[last] == o:
            end_tok = last
        start = self.t[o].end
        end = self.t[end_tok].start
        raw = self.src[start:end]
        lead = len(raw) - len(raw.lstrip())
        body = raw.strip()
        if start + lead + len(body) in self.line_comment_ends:
            body += "\n" + self.indent_of(self.t[c].start)
        return body

    def strip_view(self, a: int, b: int) -> str:
        """Text of tokens a..b, minus a top-level erasure wrapper if it is one."""
        o = self.erasure_call(a, b)
        return self.text(a, b) if o is None else self.inner_text(o)

    def erasure_count(self) -> int:
        """Erasure *calls* in code, turbofish forms included (attributes and
        `cfg!` excluded, since `any(..)` there is a cfg predicate)."""
        t, n = self.t, 0
        for k, tok in enumerate(t):
            if tok.kind != IDENT or self.is_p(k - 1, ".") or self.is_id(k - 1, "fn"):
                continue
            if tok.text == "any":
                segs = self.path_before(k)
                if segs and not _frust_root(t[segs[0]].text):
                    continue
                if not segs and not self.bare_any_ok:
                    continue
            elif tok.text == "new" and self.is_p(k - 1, "::"):
                j = k - 2
                if self.is_p(j, ">"):  # AnyView::<S>::new
                    j = self._skip_generics_back(j)
                    if j is None or not self.is_p(j - 1, "::"):
                        continue
                    j -= 2
                if not self.is_id(j, "AnyView"):
                    continue
                segs = self.path_before(j)
                if segs and not _frust_root(t[segs[0]].text):
                    continue
            else:
                continue
            o = self._call_open_after(k)
            if o is None or self.in_attr_or_cfg(k):
                continue
            if len(self.split_args(o)) == 1:
                n += 1
        return n

    def _skip_generics_back(self, j: int):
        """From a `>` at j, return the index of its matching `<` (None if absent)."""
        depth = 0
        while j >= 0:
            if self.is_p(j, ">"):
                depth += 1
            elif self.is_p(j, "<"):
                depth -= 1
                if depth == 0:
                    return j
            elif self.is_p(j, ";") or self.is_p(j, "{") or self.is_p(j, "}"):
                return None
            j -= 1
        return None

    def _call_open_after(self, k: int):
        """Index of the `(` opening a call of ident k (`f(` or `f::<..>(`)."""
        j = k + 1
        if self.is_p(j, "("):
            return j
        if self.is_p(j, "::") and self.is_p(j + 1, "<"):
            depth, j = 0, j + 1
            while j < len(self.t):
                if self.is_p(j, "<"):
                    depth += 1
                elif self.is_p(j, ">"):
                    depth -= 1
                    if depth == 0:
                        break
                elif self.is_p(j, ";") or self.is_p(j, "{"):
                    return None
                j += 1
            if self.is_p(j + 1, "("):
                return j + 1
        return None

    def in_attr_or_cfg(self, k: int) -> bool:
        g = self.parent[k]
        while g >= 0:
            kind = self.group_kind.get(g)
            if kind == "attr" or kind == "macro:cfg":
                return True
            g = self.parent[g]
        return False

    def _defines_foreign_any(self) -> bool:
        """True when the file defines a `fn any` that is not a view eraser."""
        t = self.t
        for k in range(len(t) - 1):
            if not (self.is_id(k, "fn") and self.is_id(k + 1, "any")):
                continue
            j, has_view = k + 2, False
            while j < len(t) and not (self.is_p(j, "{") or self.is_p(j, ";")):
                if self.is_id(j, "View"):
                    has_view = True
                    break
                j += 1
            if not has_view:
                return True
        return False

    def _local_type_names(self) -> set:
        names = set()
        for k in range(len(self.t) - 1):
            if self.is_id(k) and self.t[k].text in ("struct", "enum", "type", "trait", "union") \
                    and self.is_id(k + 1):
                names.add(self.t[k + 1].text)
        return names

    # -- use declarations -----------------------------------------------------

    def _parse_uses(self) -> list:
        decls = []
        t = self.t
        for k, tok in enumerate(t):
            if tok.kind != IDENT or tok.text != "use":
                continue
            j = k - 1
            is_pub = False
            if self.is_p(j, ")") and self.is_id(self.match[j] - 1, "pub"):
                j = self.match[j] - 1
            if self.is_id(j, "pub"):
                is_pub, j = True, j - 1
            if j >= 0 and not (self.is_p(j, ";") or self.is_p(j, "{") or self.is_p(j, "}")
                               or self.is_p(j, "]")):
                continue
            if self.forbidden(k):
                continue
            stmt_start = j + 1
            leaves, globs = [], []
            try:
                root, semi = self._parse_tree(k + 1, (), leaves, globs)
            except (IndexError, ValueError):
                continue
            if not self.is_p(semi, ";"):
                continue
            decls.append(UseDecl(len(decls), k, stmt_start, semi, is_pub, root, leaves, globs,
                                 self.scope_of(k)))
        return decls

    def _parse_tree(self, k: int, prefix: tuple, leaves: list, globs: list):
        """Parse one use-tree at code index k; returns (Item, next index)."""
        start = k
        segs = list(prefix)
        if self.is_p(k, "::"):
            k += 1
        while True:
            if self.is_id(k):
                segs.append(self.t[k].text)
                k += 1
                if self.is_p(k, "::"):
                    k += 1
                    continue
                item = Item(start, k - 1)
                if self.is_id(k, "as") and (self.is_id(k + 1) or self.is_p(k + 1, "_")):
                    alias = self.t[k + 1].text
                    item.end = k + 1
                    leaves.append(Leaf(tuple(segs), alias, True, item))
                    return item, k + 2
                name = segs[-1]
                path = tuple(segs[:-1]) if name == "self" else tuple(segs)
                name = path[-1] if name == "self" and path else name
                leaves.append(Leaf(path, name, False, item))
                return item, k
            if self.is_p(k, "*"):
                globs.append(tuple(segs))
                return Item(start, k, is_glob=True), k + 1
            if self.is_p(k, "{"):
                c = self.match[k]
                grp = Group(k, c)
                j = k + 1
                while j < c:
                    sub, j = self._parse_tree(j, tuple(segs), leaves, globs)
                    grp.items.append(sub)
                    if self.is_p(j, ","):
                        j += 1
                    elif j != c:
                        raise ValueError("bad use group")
                return Item(start, c, group=grp), c + 1
            raise ValueError("bad use tree")

    def decl_for(self, name: str, pos: int):
        """Innermost use-decl leaf importing `name` whose scope covers `pos`."""
        best = None
        for d in self.decls:
            if not (d.scope[0] <= pos < d.scope[1]):
                continue
            for leaf in d.leaves:
                if leaf.name == name:
                    if best is None or d.scope[0] >= best[0].scope[0]:
                        best = (d, leaf)
        return best

    def module_of(self, pos: int) -> tuple[int, int]:
        """Char span of the innermost inline `mod name { .. }` body containing `pos`."""
        best = (0, len(self.src))
        t = self.t
        for o in self.group_kind:
            if t[o].text != "{" or not (self.is_id(o - 1) and self.is_id(o - 2, "mod")):
                continue
            a, b = t[o].start, t[self.match[o]].end
            if a <= pos < b and a >= best[0]:
                best = (a, b)
        return best

    def glob_covers(self, pos: int) -> bool:
        return any(d.globs and d.scope[0] <= pos < d.scope[1] for d in self.decls)

    def usage_counts(self, names) -> dict:
        """{(decl ordinal, leaf path, name): code usages bound to that import}."""
        out = {}
        decl_spans = [(d.stmt_start, d.semi) for d in self.decls]
        for k, tok in enumerate(self.t):
            if tok.kind != IDENT or tok.text not in names:
                continue
            if self.is_p(k - 1, ".") or self.is_p(k - 1, "::") or self.is_id(k - 1, "fn"):
                continue
            if self.is_p(k + 1, "!"):
                continue
            if any(a <= k <= b for a, b in decl_spans):
                continue
            hit = self.decl_for(tok.text, tok.start)
            if hit:
                d, leaf = hit
                key = (d.ordinal, leaf.path, leaf.name)
                out[key] = out.get(key, 0) + 1
        return out

    # -- binding / shadowing ----------------------------------------------------

    def enclosing_fn(self, k: int):
        g = self.parent[k]
        while g >= 0:
            if self.t[g].text == "{":
                j = g - 1
                while j >= 0 and not (self.is_p(j, ";") or self.is_p(j, "{") or self.is_p(j, "}")):
                    if self.is_id(j, "fn"):
                        return j, self.match[g]
                    if self.t[j].kind == PUNCT and self.t[j].text in CLOSE:
                        j = self.match[j]   # step over `(..)` / `[..]` in the signature
                    j -= 1
            g = self.parent[g]
        return None

    def binding_shadows(self, name: str, site: int) -> bool:
        """True when a local binding `name` may be in scope at code index `site`."""
        span = self.enclosing_fn(site)
        if span is None:
            return False
        a, b = span
        site_pos = self.t[site].start
        for k in range(a, b + 1):
            if self.t[k].start >= site_pos:
                break
            if not self.is_id(k, name):
                continue
            if self.is_p(k - 1, ".") or self.is_p(k - 1, "::") or self.is_id(k - 1, "fn"):
                continue
            if self.is_p(k + 1, "(") or self.is_p(k + 1, "!") or self.is_p(k + 1, "::"):
                continue
            g = self.parent[k]
            if self.is_p(k + 1, ":") and g >= 0 and self.t[g].text == "{":
                continue  # struct-literal / struct-pattern field name
            # A binding is in scope for the rest of its innermost enclosing
            # block; a parameter of the enclosing fn for the whole body.
            blk = g
            while blk >= 0 and self.t[blk].text != "{":
                blk = self.parent[blk]
            in_params = blk < 0 or self.t[blk].start < self.t[a].start
            if not in_params and not (self.t[blk].start <= site_pos < self.t[self.match[blk]].end):
                continue
            # `let name = <site>`: the binding is not yet in scope in its own initialiser.
            j = k - 1
            let_tok = None
            while j >= 0 and self.parent[j] == g and not (
                    self.is_p(j, ";") or self.is_p(j, "{") or self.is_p(j, "}")):
                if self.is_id(j, "let"):
                    let_tok = j
                    break
                j -= 1
            if let_tok is not None:
                e = k
                while e < len(self.t) and not (self.is_p(e, ";") and self.parent[e] == g):
                    e += 1
                if e < len(self.t) and self.t[let_tok].start <= site_pos <= self.t[e].end:
                    continue
            return True
        return False


def _frust_root(seg: str) -> bool:
    return seg.startswith("frust") or seg in ROOT_PREFIX_OK


# ---------------------------------------------------------------------------
# Name list
# ---------------------------------------------------------------------------

@dataclass
class Apis:
    slot: set = field(default_factory=set)
    builder: set = field(default_factory=set)
    exclude: list = field(default_factory=list)
    lists: set = field(default_factory=set)


DEFAULT_APIS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "erasing_apis.txt")


def load_apis(path: str = DEFAULT_APIS) -> Apis:
    apis = Apis()
    section = None
    with open(path, encoding="utf-8") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            if line.startswith("[") and line.endswith("]"):
                section = line[1:-1].strip()
                if section not in ("slot", "builder", "exclude", "list"):
                    raise ValueError(f"{path}: unknown section [{section}]")
                continue
            for entry in line.split():
                if section == "slot":
                    apis.slot.add(entry)
                elif section == "builder":
                    apis.builder.add(entry)
                elif section == "exclude":
                    apis.exclude.append(tuple(entry.split("::")))
                elif section == "list":
                    apis.lists.add(entry)
                else:
                    raise ValueError(f"{path}: entry {entry!r} before any [section]")
    return apis


# ---------------------------------------------------------------------------
# Candidate discovery
# ---------------------------------------------------------------------------

@dataclass
class Edit:
    start: int
    end: int
    text: str


@dataclass
class Candidate:
    rule: str
    line: int
    desc: str
    span: tuple                  # (start, end) chars of the outermost replaced region
    edits: list                  # list[Edit]
    imports: list = field(default_factory=list)   # (decl ordinal, anchor leaf path, new name)


def _call_key(s: Source, k: int):
    """For ident `k` followed by `(`: ('.name' | 'name' | 'Type::name', qualifier segs)."""
    t = s.t
    name = t[k].text
    if s.is_p(k - 1, "."):
        return "." + name, []
    if s.is_id(k - 1) and t[k - 1].text in ("fn", "struct", "enum", "union", "trait", "type", "mod"):
        return None, []
    segs = [t[i].text for i in s.path_before(k)]
    if segs and segs[-1][:1].isupper():
        return segs[-1] + "::" + name, segs[:-1]
    return name, segs


def _excluded(s: Source, apis: Apis, name: str, qual: list, pos: int) -> bool:
    if not apis.exclude:
        return False
    if qual:
        full = tuple(qual) + (name,)
    else:
        hit = s.decl_for(name, pos)
        if not hit or hit[1].aliased:
            return False
        full = hit[1].path
    return any(full[-len(e):] == e for e in apis.exclude if len(e) <= len(full))


def find_candidates(s: Source, apis: Apis, t5: bool = False, t7: bool = False,
                    ctx: "T7Ctx | None" = None):
    """Return (candidates, notes). Candidates may overlap; callers pick outermost.

    `t5` turns on the opt-in homogeneous list rule T5, `t7` the opt-in helper
    return rule T7 (`ctx` carries the crate facts it needs; T7 exclusions land
    in `s.t7_excluded`)."""
    t = s.t
    cands: list[Candidate] = []
    notes: list[tuple[int, str]] = []
    first_new = 0
    first_note = 0
    s.kept = 0
    s.t7_excluded = []
    for k, tok in enumerate(t):
        if tok.kind != IDENT or not s.is_p(k + 1, "("):
            continue
        key, qual = _call_key(s, k)
        if key is None or s.forbidden(k):
            continue
        o = k + 1
        is_list = t5 and (key in apis.lists or key in T5_BUILTIN) and _t5_sugar_resolves(s, k, key, qual)
        if key in apis.slot or key in apis.builder or is_list:
            if "::" not in key and not key.startswith(".") and _excluded(s, apis, key, qual, tok.start):
                continue
        if key in apis.slot:
            _t1(s, key, k, o, cands)
        if key in apis.builder:
            _t4(s, key, k, o, cands)
        before_t2 = len(cands)
        if key in BUILDER_OF:
            _t2(s, key, k, o, qual, cands, notes)
        if key == "FlexView::new":
            _t3(s, k, o, qual, cands, notes)
        if is_list and len(cands) == before_t2:
            _t5(s, key, o, cands)
        if len(cands) > first_new or len(notes) > first_note:
            # A `// erasure: keep` site is exempt from every rule: neither
            # listed, rewritten, nor noted; only counted.
            fresh = cands[first_new:]
            kept = [c for c in fresh if s.is_kept(s.line_of(c.span[0]))]
            if kept:
                s.kept += len(kept)
                cands[first_new:] = [c for c in fresh if c not in kept]
            notes[first_note:] = [n for n in notes[first_note:] if not s.is_kept(n[0])]
            first_note = len(notes)
        if len(cands) > first_new:
            _drop_comment_losers(s, cands, first_new, notes)
            first_new = len(cands)
    if t7:
        _t7(s, ctx or T7Ctx(), cands, notes)
        _drop_comment_losers(s, cands, first_new, notes)
    return cands, notes


def _comment_count(text: str) -> int:
    try:
        return sum(1 for tk in lex(text) if tk.kind in (LINE_COMMENT, BLOCK_COMMENT))
    except LexError:
        return 0


def _drop_comment_losers(s: Source, cands: list, first: int, notes: list) -> None:
    """Remove new candidates whose rewrite would lose a comment; note each one."""
    keep = []
    for c in cands[first:]:
        # Compare per replaced range, so comments T5 keeps between elements
        # (outside every edit) are not mistaken for lost ones.
        old = sum(_comment_count(s.src[e.start:e.end]) for e in c.edits)
        new = sum(_comment_count(e.text) for e in c.edits)
        if new < old:
            notes.append((c.line, f"note: {c.rule} skipped: comment inside the call head"))
        else:
            keep.append(c)
    cands[first:] = keep


def _t1(s, key, k, o, cands):
    for a, b in s.split_args(o):
        target = None
        if s.is_id(a, "Some") and s.is_p(a + 1, "(") and s.match[a + 1] == b:
            inner = s.split_args(a + 1)
            if len(inner) == 1:
                target = inner[0]
        else:
            target = (a, b)
        if target is None:
            continue
        eo = s.erasure_call(*target)
        if eo is None:
            continue
        span = (s.t[target[0]].start, s.t[target[1]].end)
        cands.append(Candidate("T1", s.line_of(span[0]), f"redundant any() in argument of {key}",
                               span, [Edit(span[0], span[1], s.inner_text(eo))]))


def _t4(s, key, k, o, cands):
    for a, b in s.split_args(o):
        i = a
        if s.is_id(i, "move"):
            i += 1
        if not s.is_p(i, "|"):
            continue
        j = i + 1
        while j <= b and not (s.is_p(j, "|") and s.parent[j] == s.parent[i]):
            j += 1
        body = j + 1
        if body > b or s.is_p(body, "->"):
            continue
        if s.is_p(body, "{") and s.match[body] == b:
            if body + 1 > b - 1:
                continue
            ra, rb = body + 1, b - 1
        else:
            ra, rb = body, b
        eo = s.erasure_call(ra, rb)
        if eo is None:
            continue
        span = (s.t[ra].start, s.t[rb].end)
        cands.append(Candidate("T4", s.line_of(span[0]), f"closure body any() in builder of {key}",
                               span, [Edit(span[0], span[1], s.inner_text(eo))]))


def _vec_literal(s: Source, a: int, b: int):
    """Return the `[` index when tokens a..b are exactly `vec![..]`."""
    if s.is_id(a, "vec") and s.is_p(a + 1, "!") and s.is_p(a + 2, "[") and s.match[a + 2] == b:
        lb = a + 2
        for q in range(lb + 1, b):
            if s.parent[q] == lb and s.is_p(q, ";"):
                return None
        return lb
    return None


def _elements(s: Source, lb: int):
    """Split a `[`..`]` group into elements with their surrounding trivia text.

    Returns (elements, tail) where each element is (a, b, lead, trail) — the
    code-token range plus the verbatim text before/after it up to the commas —
    and tail is the text between the last comma and `]`.
    """
    rb = s.match[lb]
    commas = s.commas(lb)
    elems = []
    prev_end = s.t[lb].end
    start = lb + 1
    for bnd in commas + [rb]:
        if start <= bnd - 1:
            a, b = start, bnd - 1
            elems.append((a, b, s.src[prev_end:s.t[a].start], s.src[s.t[b].end:s.t[bnd].start]))
        prev_end = s.t[bnd].end
        start = bnd + 1
    tail = ""
    if commas and (not elems or commas[-1] > elems[-1][1]):
        tail = s.src[s.t[commas[-1]].end:s.t[rb].start]
    return elems, tail


def _render_chain(s: Source, head: str, pieces: list, tail: str, rb: int) -> str:
    """Join `head` and the chained calls, keeping inter-element comments verbatim."""
    out = [head]
    for lead, call, trail in pieces:
        if "\n" not in lead and not lead.strip():
            lead = ""
        if not trail.strip():
            trail = ""
        out.append(lead + call + trail)
    if tail.strip():
        out.append(tail)
    text = "".join(out).rstrip()
    # A trailing line comment would swallow whatever follows the chain.
    if _ends_in_line_comment(text):
        text += "\n" + s.indent_of(s.t[rb].start)
    return text


def _ends_in_line_comment(text: str) -> bool:
    try:
        toks = [tk for tk in lex(text) if tk.kind != WS]
    except LexError:
        return False
    return bool(toks) and toks[-1].kind == LINE_COMMENT


def _resolve_builder(s, k, o, anchor, builder, qual, notes, rule):
    """Decide how to spell `builder()`; returns (prefix, import edits) or None."""
    pos = s.t[k].start
    if anchor in s.local_types:
        return None
    if qual:
        if not _frust_root(qual[0]):
            return None
        return "::".join(qual) + "::", []
    hit = s.decl_for(anchor, pos)
    if hit and (hit[1].aliased or not _frust_root(hit[1].path[0])):
        return None
    if s.binding_shadows(builder, k):
        notes.append((s.line_of(pos), f"note: {rule} skipped: a local binding `{builder}` is in scope"))
        return None
    if s.decl_for(builder, pos):
        return "", []
    if builder in s.local_fns:
        notes.append((s.line_of(pos), f"note: {rule} skipped: a local fn `{builder}` is in scope"))
        return None
    if hit and not hit[0].is_pub:
        decl, leaf = hit
        site_mod = s.module_of(pos)
        if site_mod != s.module_of(s.t[decl.use_idx].start) and leaf.path[0] not in ("self", "super"):
            # The site sits in a nested module (typically `#[cfg(test)] mod tests`)
            # that sees the anchor through `use super::*`: import the builder
            # there, so the outer import is not left unused in the other cfg.
            for g in s.decls:
                if not g.is_pub and ("super",) in g.globs and g.scope == site_mod:
                    return "", [("new", g.ordinal, leaf.path[:-1] + (builder,))]
        return "", [("add", decl.ordinal, leaf.path, builder)]
    if s.glob_covers(pos):
        return "", []
    notes.append((s.line_of(pos), f"note: {rule} skipped: cannot resolve an import for `{builder}`"))
    return None


def _t2(s, key, k, o, qual, cands, notes):
    args = s.split_args(o)
    if len(args) != 1:
        return
    lb = _vec_literal(s, *args[0])
    if lb is None:
        return
    elems, tail = _elements(s, lb)
    if any(s.is_p(a, "#") for a, _, _, _ in elems):
        return
    builder = BUILDER_OF[key]
    res = _resolve_builder(s, k, o, key, builder, qual, notes, "T2")
    if res is None:
        return
    prefix, imports = res
    pieces = []
    for a, b, lead, trail in elems:
        pieces.append((lead, f".child({s.strip_view(a, b)})", trail))
    start = s.t[s.path_before(k)[0]].start if qual else s.t[k].start
    end = s.t[s.match[o]].end
    text = _render_chain(s, f"{prefix}{builder}()", pieces, tail, s.match[lb])
    cands.append(Candidate("T2", s.line_of(start), f"{key}(vec![..]) -> {builder}() chain",
                           (start, end), [Edit(start, end, text)], imports))


def _flex_child(s, a, b):
    """Classify a FlexView::new element: (kind, rendered call text)."""
    k = a
    segs = []
    while s.is_id(k) and s.is_p(k + 1, "::") and s.is_id(k + 2):
        segs.append(s.t[k].text)
        k += 2
    if s.is_id(k) and s.t[k].text in ("inflexible", "flexible", "keyed") \
            and s.is_p(k + 1, "(") and s.match[k + 1] == b \
            and (not segs or _frust_root(segs[0])):
        name = s.t[k].text
        o = k + 1
        args = s.split_args(o)
        want = 1 if name == "inflexible" else 2
        if len(args) == want:
            va, vb = args[-1]
            eo = s.erasure_call(va, vb)
            body = s.src[s.t[o].end:s.t[s.match[o]].start]
            if eo is not None:
                rel_a = s.t[va].start - s.t[o].end
                rel_b = s.t[vb].end - s.t[o].end
                body = body[:rel_a] + s.inner_text(eo) + body[rel_b:]
            method = {"inflexible": ".child", "flexible": ".flex", "keyed": ".keyed"}[name]
            return name, f"{method}({body})"
    return "other", f".push({s.text(a, b)})"


def _t3(s, k, o, qual, cands, notes):
    args = s.split_args(o)
    if len(args) != 2:
        return
    (aa, ab), (va, vb) = args
    # The axis must be a literal (optionally path-qualified) `Axis::Vertical|Horizontal`.
    path_ok = all(s.is_id(i) for i in range(aa, ab + 1, 2)) and \
        all(s.is_p(i, "::") for i in range(aa + 1, ab, 2)) and (ab - aa) % 2 == 0
    segs = [s.t[i].text for i in range(aa, ab + 1, 2)] if path_ok else []
    if len(segs) < 2 or segs[-2] != "Axis" or segs[-1] not in ("Vertical", "Horizontal"):
        return
    if len(segs) > 2 and not _frust_root(segs[0]):
        return
    axis = segs[-1]
    lb = _vec_literal(s, va, vb)
    if lb is None:
        return
    elems, tail = _elements(s, lb)
    if any(s.is_p(a, "#") for a, _, _, _ in elems):
        return
    rendered = [(_flex_child(s, a, b), lead, trail) for a, b, lead, trail in elems]
    kinds = {r[0][0] for r in rendered}
    if "keyed" in kinds and kinds & {"inflexible", "flexible"}:
        notes.append((s.line_of(s.t[k].start),
                      "note: T3 skipped: FlexView::new list mixes keyed(..) with unkeyed children"))
        return
    builder = "column" if axis == "Vertical" else "row"
    res = _resolve_builder(s, k, o, "FlexView", builder, qual, notes, "T3")
    if res is None:
        return
    prefix, imports = res
    pieces = [(lead, call, trail) for (_, call), lead, trail in rendered]
    first = s.path_before(k - 2)
    start = s.t[first[0]].start if first else s.t[k - 2].start
    end = s.t[s.match[o]].end
    text = _render_chain(s, f"{prefix}{builder}()", pieces, tail, s.match[lb])
    cands.append(Candidate("T3", s.line_of(start), f"FlexView::new(Axis::{axis}, vec![..]) -> {builder}() chain",
                           (start, end), [Edit(start, end, text)], imports))


# Callees whose `vec![..]` argument is always a list of views, whatever the
# name list says (T5 also uses every [list] entry of `erasing_apis.txt`).
T5_BUILTIN = frozenset({"Column", "Row", "Stack", ".children"})
_T5_SUGAR = frozenset({"Column", "Row", "Stack"})


def _t5_sugar_resolves(s: Source, k: int, key: str, qual: list) -> bool:
    """Whether the `Column`/`Row`/`Stack` call at `k` is frust's own sugar.

    Same conditions T2 applies: a frust-rooted qualifier, or a non-aliased
    frust import (or a glob import); a local type or foreign path declines.
    `.children` is a method key and always passes."""
    if key not in _T5_SUGAR:
        return True
    if key in s.local_types:
        return False
    if qual:
        return _frust_root(qual[0])
    pos = s.t[k].start
    hit = s.decl_for(key, pos)
    if hit:
        return not hit[1].aliased and _frust_root(hit[1].path[0])
    return s.glob_covers(pos)
# Keywords that can never start a callee path (path roots like `crate` can).
_T5_NON_PATH = frozenset(KEYWORDS - {"crate", "super", "self", "Self"})


def _t5_erasure(s: Source, a: int, b: int):
    """Like `erasure_call`, also accepting a module-relative `authoring::any(X)`."""
    o = s.erasure_call(a, b)
    if o is not None:
        return o
    if s.is_id(a, "authoring") and s.is_p(a + 1, "::") and s.is_id(a + 2, "any") \
            and s.is_p(a + 3, "(") and s.match[a + 3] == b and len(s.split_args(a + 3)) == 1:
        return a + 3
    return None


def _t5_head(s: Source, a: int, b: int):
    """Head of the expression a..b: (callee path, trailing method names), or None.

    The expression must be exactly `path(..)` followed by zero or more
    `.name(..)` method calls; anything else has no head."""
    k = a
    segs = []
    if s.is_p(k, "::"):
        segs.append("::")
        k += 1
    while True:
        if not s.is_id(k) or s.t[k].text in _T5_NON_PATH:
            return None
        segs.append(s.t[k].text)
        k += 1
        if s.is_p(k, "::") and s.is_id(k + 1):
            segs.append("::")
            k += 1
            continue
        break
    if not s.is_p(k, "("):
        return None
    k = s.match[k]
    methods = []
    while k < b:
        if not (s.is_p(k + 1, ".") and s.is_id(k + 2) and s.is_p(k + 3, "(")):
            return None
        methods.append(s.t[k + 2].text)
        k = s.match[k + 3]
    if k != b:
        return None
    return "".join(segs), tuple(methods)


def _t5(s, key, o, cands):
    for a, b in s.split_args(o):
        lb = _vec_literal(s, a, b)
        if lb is None:
            continue
        elems, _ = _elements(s, lb)
        if not elems:
            continue
        heads, edits = set(), []
        for ea, eb, _, _ in elems:
            eo = _t5_erasure(s, ea, eb)
            if eo is None:
                break
            inner = s.split_args(eo)
            head = _t5_head(s, *inner[0]) if len(inner) == 1 else None
            if head is None:
                break
            heads.add(head)
            if len(heads) > 1:
                break
            edits.append(Edit(s.t[ea].start, s.t[eb].end, s.inner_text(eo)))
        else:
            span = (s.t[a].start, s.t[b].end)
            cands.append(Candidate("T5", s.line_of(span[0]), f"homogeneous list argument of {key}",
                                   span, edits))


# ---------------------------------------------------------------------------
# T7: helper fns returning AnyView
# ---------------------------------------------------------------------------

T7_BUCKETS = ("T7-trait", "T7-arms", "T7-ref", "T7-public")


@dataclass
class T7Ctx:
    """Per-file facts T7 needs that the file text alone does not carry."""
    edition: str = "2024"          # the crate's Rust edition
    library: bool = False          # a crates/ or plugins/ library source
    values: frozenset = frozenset()   # `_t7_value_names` keys found elsewhere in the package


@dataclass
class T7Fn:
    fn: int                # code index of `fn`
    name: int              # code index of the fn name
    gen: tuple | None      # (`<`, `>`) of the fn's own generics
    params: int            # `(` of the parameter list
    arrow: int             # `->`
    ret: tuple             # (first, last) code index of the return type
    av: int                # code index of `AnyView` in the return type
    body: int | None       # `{` of the body (None for a trait declaration)


def _angle_end(s: Source, j: int):
    """From `<` at j, the index of its matching `>`; None if a `{`/`;` comes first."""
    depth = 0
    while j < len(s.t):
        tok = s.t[j]
        if tok.kind == PUNCT:
            if tok.text == "<":
                depth += 1
            elif tok.text == ">":
                depth -= 1
                if depth == 0:
                    return j
            elif tok.text in ("{", ";"):
                return None
            elif tok.text in ("(", "["):
                j = s.match[j]
        j += 1
    return None


def _t7_anyview(s: Source, a: int, b: int):
    """`AnyView` index when tokens a..b are exactly `[path::]AnyView<..>`."""
    k = a
    if s.is_p(k, "::"):
        k += 1
    segs = []
    while s.is_id(k):
        segs.append(k)
        if s.is_p(k + 1, "::") and s.is_id(k + 2):
            k += 2
        else:
            break
    if not segs or s.t[segs[-1]].text != "AnyView" or not s.is_p(segs[-1] + 1, "<"):
        return None
    if k != a and len(segs) == 1:
        return None   # `::AnyView<..>`: not a frust path
    if _angle_end(s, segs[-1] + 1) != b:
        return None
    if len(segs) > 1:
        root = s.t[segs[0]].text
        if not (_frust_root(root) or root == "authoring"):
            return None
    else:
        if "AnyView" in s.local_types:
            return None
        hit = s.decl_for("AnyView", s.t[a].start)
        if hit and (hit[1].aliased or not _frust_root(hit[1].path[0])):
            return None
    return segs[-1]


def _t7_fns(s: Source) -> list:
    """Every non-async, non-const `fn` whose return type is frust's `AnyView<..>`."""
    t, out = s.t, []
    for k in range(len(t) - 1):
        if not (s.is_id(k, "fn") and s.is_id(k + 1)) or s.forbidden(k):
            continue
        q, quals = k - 1, set()
        while q >= 0 and (t[q].kind == STR or (
                s.is_id(q) and t[q].text in ("unsafe", "extern", "async", "const", "default"))):
            quals.add(t[q].text)
            q -= 1
        if "async" in quals or "const" in quals:
            continue
        j, gen = k + 2, None
        if s.is_p(j, "<"):
            e = _angle_end(s, j)
            if e is None:
                continue
            gen, j = (j, e), e + 1
        if not s.is_p(j, "(") or not s.is_p(s.match[j] + 1, "->"):
            continue
        params, arrow = j, s.match[j] + 1
        m = arrow + 1
        while m < len(t) and not (s.is_p(m, "{") or s.is_p(m, ";") or s.is_id(m, "where")):
            if t[m].kind == PUNCT and t[m].text in OPEN:
                m = s.match[m]
            m += 1
        ret = (arrow + 1, m - 1)
        while m < len(t) and not (s.is_p(m, "{") or s.is_p(m, ";")):
            if t[m].kind == PUNCT and t[m].text in OPEN:
                m = s.match[m]
            m += 1
        if m >= len(t) or ret[0] > ret[1]:
            continue
        av = _t7_anyview(s, *ret)
        if av is None:
            continue
        out.append(T7Fn(k, k + 1, gen, params, arrow, ret, av, m if s.is_p(m, "{") else None))
    return out


def _t7_container(s: Source, fn: int):
    """('trait' | 'trait_impl' | 'impl' | 'free', impl-generics `<`/`>` or None)."""
    g = s.parent[fn]
    if g < 0 or s.t[g].text != "{":
        return "free", None
    j = g - 1
    while j >= 0 and s.parent[j] == s.parent[g] and not (
            s.is_p(j, ";") or s.is_p(j, "{") or s.is_p(j, "}")):
        if s.t[j].kind == PUNCT and s.t[j].text in CLOSE:
            j = s.match[j]
        j -= 1
    head = [k for k in range(j + 1, g) if s.parent[k] == s.parent[g]]
    if any(s.is_id(k, "fn") or s.is_id(k, "mod") for k in head):
        return "free", None
    if any(s.is_id(k, "trait") for k in head):
        return "trait", None
    impls = [k for k in head if s.is_id(k, "impl")]
    if not impls:
        return "free", None
    if any(s.is_id(k, "for") and not s.is_p(k + 1, "<") for k in head):
        return "trait_impl", None
    i = impls[0]
    gen = None
    if s.is_p(i + 1, "<"):
        e = _angle_end(s, i + 1)
        gen = (i + 1, e) if e is not None else None
    return "impl", gen


def _t7_returns(s: Source, f: T7Fn) -> list:
    """`return` tokens of this fn's own body (nested fn items excluded)."""
    close = s.match[f.body]
    out = []
    for k in range(f.body + 1, close):
        if s.is_id(k, "return"):
            enc = s.enclosing_fn(k)
            if enc is not None and enc[0] == f.fn:
                out.append(k)
    return out


def _t7_arms_tail(s: Source, body: int, a: int, b: int):
    """Reason text when tokens a..b are one whole `if`/`else` chain or `match`."""
    if s.is_id(a, "match"):
        for k in range(a + 1, b + 1):
            if s.parent[k] == body and s.is_p(k, "{"):
                return "match tail" if s.match[k] == b else None
        return None
    j = a
    while s.is_id(j, "if"):
        blk = next((k for k in range(j + 1, b + 1) if s.parent[k] == body and s.is_p(k, "{")), None)
        if blk is None:
            return None
        e = s.match[blk]
        if not s.is_id(e + 1, "else"):
            return None
        if s.is_p(e + 2, "{"):
            return "if/else tail" if s.match[e + 2] == b else None
        j = e + 2
    return None


def _t7_tail(s: Source, f: T7Fn):
    """Classify the body: ('single', erasure `(`, a, b) | ('arms', reason) | None."""
    body, close = f.body, s.match[f.body]
    # Statement boundaries: a top-level `;`, or the `}` ending a block-like
    # statement (`for .. {}`, `if .. {}`) that is not the tail itself and is not
    # continued (`} else`, `}.child(..)`, `}?`).
    top_semis = [k for k in range(body + 1, close) if s.parent[k] == body and (
        s.is_p(k, ";") or (s.is_p(k, "}") and k != close - 1 and not (
            s.is_id(k + 1, "else") or s.is_id(k + 1, "as") or s.is_p(k + 1, ".")
            or s.is_p(k + 1, "?"))))]
    last = top_semis[-1] if top_semis else body
    returns = _t7_returns(s, f)
    tail_return = None
    if last + 1 <= close - 1:
        a, b = last + 1, close - 1
        if s.is_id(a, "return"):
            tail_return, a = a, a + 1
    else:
        prev = top_semis[-2] if len(top_semis) > 1 else body
        top_rets = [r for r in returns if s.parent[r] == body and prev < r < last]
        if not top_rets:
            return None
        tail_return = top_rets[-1]
        a, b = tail_return + 1, last - 1
    if any(r != tail_return for r in returns):
        return "arms", "early return"
    if a > b:
        return None
    if tail_return is None and (s.is_id(a, "if") or s.is_id(a, "match")):
        reason = _t7_arms_tail(s, body, a, b)
        return ("arms", reason) if reason else None
    eo = _t5_erasure(s, a, b)
    if eo is None:
        return None
    return "single", eo, a, b


def _t7_recursive(s: Source, f: T7Fn, kind: str) -> bool:
    """True when the fn calls itself (`name(..)`, or `Self::name(..)`/`self.name(..)` in an impl)."""
    name = s.t[f.name].text
    for k in range(f.body + 1, s.match[f.body]):
        if not (s.is_id(k, name) and s.is_p(k + 1, "(")):
            continue
        if kind == "free":
            if not (s.is_p(k - 1, ".") or s.is_p(k - 1, "::")):
                return True
        elif (s.is_p(k - 1, "::") and s.is_id(k - 2, "Self")) or \
                (s.is_p(k - 1, ".") and s.is_id(k - 2, "self")):
            return True
    return False


def _t7_value_names(s: Source, names) -> set:
    """Names in `names` that this file uses as a *value* (not a call): a fn
    pointer, a stored callback, an argument of `Box::new`/`.map(..)`.

    A bare or module-qualified use (`name`, `views::name`) is reported as
    `name` (it can name a free fn); a type-qualified one (`Self::name`,
    `Card::name`) as `::name` (it can name a method). A heuristic that errs
    towards reporting a use, so a false hit only excludes."""
    if not names:
        return set()
    t, out = s.t, set()
    spans = [(d.stmt_start, d.semi) for d in s.decls]
    for k, tok in enumerate(t):
        if tok.kind != IDENT or tok.text not in names:
            continue
        typed = s.is_p(k - 1, "::") and s.is_id(k - 2) and s.t[k - 2].text[:1].isupper()
        key = "::" + tok.text if typed else tok.text
        if key in out or s.is_p(k - 1, ".") or s.is_id(k - 1, "fn"):
            continue
        if s.is_p(k + 1, "(") or s.is_p(k + 1, "!") or s.is_p(k + 1, "::") or s.is_p(k + 1, ":") \
                or s.is_p(k + 1, "@") or s.is_p(k + 1, "=>") or s.is_id(k + 1, "in") \
                or s.is_p(k + 1, ".") or s.is_p(k + 1, "?"):
            continue
        if s.is_p(k + 1, "=") and not s.is_p(k + 2, "="):
            continue   # assignment target or `let` binding
        if s.is_p(k - 1, "|") or s.is_p(k + 1, "|"):
            continue   # closure parameter
        if any(s.is_id(k - 1, w) for w in ("let", "mut", "ref", "for", "struct", "enum", "mod",
                                           "type", "const", "static", "trait", "as")):
            continue
        if any(a <= k <= b for a, b in spans) or s.forbidden(k):
            continue   # a `use`, an attribute, or a macro other than `vec!`
        if _t7_in_pattern(s, k):
            continue   # `Some(name) =>`, `for (i, name) in`, `|(a, name)|`, `let (name, ..) =`
        if not s.is_p(k - 1, "::") and (
                s.binding_shadows(tok.text, k) or _t7_let_typed(s, tok.text, k)):
            continue   # a local binding of the same name
        out.add(key)
    return out


def _t7_in_pattern(s: Source, k: int) -> bool:
    """True when token `k` sits in a parenthesised pattern (any nesting depth)."""
    g = s.parent[k]
    while g >= 0 and s.t[g].text in ("(", "["):
        after = s.match[g] + 1
        if s.is_p(after, "=>") or s.is_id(after, "in") or s.is_id(after, "if") \
                or s.is_p(after, "|") or s.is_p(g - 1, "|") \
                or (s.is_p(after, "=") and not s.is_p(after + 1, "=")):
            return True
        g = s.parent[g]
    return False


def _t7_let_typed(s: Source, name: str, site: int) -> bool:
    """True when `let [mut] name: T` precedes `site` in its enclosing fn
    (`binding_shadows` reads `name:` in a block as a struct-literal field)."""
    span = s.enclosing_fn(site)
    if span is None:
        return False
    for j in range(span[0], site):
        if s.is_id(j, name) and s.is_p(j + 1, ":") and (
                s.is_id(j - 1, "let") or (s.is_id(j - 1, "mut") and s.is_id(j - 2, "let"))):
            return True
    return False


def _t7_is_pub(s: Source, fn: int) -> bool:
    """True for a bare `pub fn` (a restricted `pub(..)` is not library API)."""
    j = fn - 1
    while s.is_id(j) and s.t[j].text in ("unsafe", "extern", "default") or \
            (j >= 0 and s.t[j].kind == STR):
        j -= 1
    return s.is_id(j, "pub")


def _t7_in_test_mod(s: Source, k: int) -> bool:
    """True when token `k` sits inside a `#[cfg(test)] mod name { .. }`."""
    g = s.parent[k]
    while g >= 0:
        if s.t[g].text == "{" and s.is_id(g - 1) and s.is_id(g - 2, "mod"):
            j = g - 3
            if s.is_p(j, ")") and s.is_id(s.match[j] - 1, "pub"):
                j = s.match[j] - 2
            elif s.is_id(j, "pub"):
                j -= 1
            while s.is_p(j, "]") and s.is_p(s.match[j] - 1, "#"):
                attr = re.sub(r"\s+", "", s.text(s.match[j], j))
                if attr == "[cfg(test)]":
                    return True
                j = s.match[j] - 2
        g = s.parent[g]
    return False


def _t7_generic_names(s: Source, rng):
    """(lifetimes, type/const params) declared in the `<`..`>` range `rng`."""
    lts, tys = [], []
    if rng is None:
        return lts, tys
    lt, gt = rng
    k, depth, start = lt + 1, 0, True
    while k < gt:
        tok = s.t[k]
        if start:
            if tok.kind == LIFETIME:
                lts.append(tok.text)
            elif s.is_id(k, "const") and s.is_id(k + 1):
                tys.append(s.t[k + 1].text)
            elif tok.kind == IDENT:
                tys.append(tok.text)
            start = False
        if tok.kind == PUNCT:
            if tok.text in ("(", "["):
                k = s.match[k]
            elif tok.text == "<":
                depth += 1
            elif tok.text == ">":
                depth -= 1
            elif tok.text == "," and depth == 0:
                start = True
        k += 1
    return lts, tys


def _t7_captures(s: Source, f: T7Fn, impl_gen, edition: str):
    """The `+ use<..>` suffix for the new return type, "" for none, None when unspellable.

    Edition 2024 captures every in-scope generic parameter and lifetime by
    default, and `View<S>: 'static` keeps a captured borrow from constraining
    callers, so nothing is emitted. Edition 2015-2021 captures no lifetime
    implicitly: when the fn has a lifetime parameter (its own or its impl's) or
    an elided reference parameter, emit `+ use<..>` listing every lifetime
    (`'_` for elided ones) and every type/const parameter in scope. An
    argument-position `impl Trait` cannot be named in that list, so such a fn
    is skipped with a note."""
    try:
        if int(edition) >= 2024:
            return ""
    except ValueError:
        return ""
    lts, tys = _t7_generic_names(s, impl_gen)
    l2, t2 = _t7_generic_names(s, f.gen)
    lts, tys = lts + l2, tys + t2
    close = s.match[f.params]
    elided = any((s.is_p(k, "&") and s.t[k + 1].kind != LIFETIME)
                 or (s.t[k].kind == LIFETIME and s.t[k].text == "'_")
                 for k in range(f.params + 1, close))
    if not lts and not elided:
        return ""
    if any(s.is_id(k, "impl") for k in range(f.params + 1, close)):
        return None
    items = lts + (["'_"] if elided else []) + tys
    return " + use<" + ", ".join(items) + ">"


def _t7_view_import(s: Source, f: T7Fn, notes: list):
    """Import edits that make a bare `View` resolve at the fn; None to skip."""
    pos = s.t[f.fn].start
    line = s.line_of(pos)
    if "View" in s.local_types:
        notes.append((line, "note: T7 skipped: a local type `View` is in scope"))
        return None
    if s.decl_for("View", pos):
        return []
    hit = s.decl_for("AnyView", pos)
    if hit:
        decl, leaf = hit
        site_mod = s.module_of(pos)
        if site_mod != s.module_of(s.t[decl.use_idx].start) and leaf.path[0] not in ("self", "super"):
            # A nested (typically `#[cfg(test)]`) module seeing AnyView through
            # `use super::*`: import View there, as T2 does for its builders.
            for g in s.decls:
                if not g.is_pub and ("super",) in g.globs and g.scope == site_mod:
                    return [("new", g.ordinal, leaf.path[:-1] + ("View",))]
        if decl.is_pub:
            return [("new", decl.ordinal, leaf.path[:-1] + ("View",))]
        return [("add", decl.ordinal, leaf.path, "View")]
    if s.glob_covers(pos):
        return []
    notes.append((line, "note: T7 skipped: cannot resolve an import for `View`"))
    return None


def _t7(s: Source, ctx: T7Ctx, cands: list, notes: list) -> None:
    fns = _t7_fns(s)
    if not fns:
        return
    values = _t7_value_names(s, {s.t[f.name].text for f in fns}) | set(ctx.values)
    for f in fns:
        name = s.t[f.name].text
        line = s.line_of(s.t[f.fn].start)
        if s.is_kept(line) or s.is_kept(s.line_of(s.t[f.arrow].start)):
            s.kept += 1
            continue
        kind, impl_gen = _t7_container(s, f.fn)
        shape = _t7_tail(s, f) if f.body is not None else None

        def exclude(bucket, reason):
            s.t7_excluded.append((line, bucket, name, reason))

        if kind in ("trait", "trait_impl"):
            if f.body is None or shape is not None:
                exclude("T7-trait", "trait method declaration" if kind == "trait"
                        else "trait impl method")
            continue
        if shape is None:
            continue
        if shape[0] == "arms":
            exclude("T7-arms", shape[1])
            continue
        if _t7_recursive(s, f, kind):
            exclude("T7-ref", "recursive")
            continue
        if (name if kind == "free" else "::" + name) in values:
            exclude("T7-ref", "used as a value")
            continue
        if ctx.library and _t7_is_pub(s, f.fn) and not _t7_in_test_mod(s, f.fn):
            exclude("T7-public", "pub fn in a library source")
            continue
        _, eo, ta, tb = shape
        uses = _t7_captures(s, f, impl_gen, ctx.edition)
        if uses is None:
            notes.append((line, "note: T7 skipped: an edition-2021 capture list cannot name "
                                "an `impl Trait` parameter"))
            continue
        a, b = f.ret
        prefix = s.src[s.t[a].start:s.t[f.av].start]
        imports = []
        if not prefix:
            imports = _t7_view_import(s, f, notes)
            if imports is None:
                continue
        ret = "impl " + prefix + "View" + s.src[s.t[f.av + 1].start:s.t[b].end] + uses
        edits = [Edit(s.t[a].start, s.t[b].end, ret),
                 Edit(s.t[ta].start, s.t[tb].end, s.inner_text(eo))]
        span = (s.t[f.fn].start, s.t[s.match[f.body]].end)
        cands.append(Candidate("T7", line, f"helper returns impl View: {name}", span, edits, imports))


def _t7_library(path: str) -> bool:
    """True for a library source: under `crates/` or `plugins/` (the first such
    component of the path, made relative to the working directory when it sits
    under it) and not under a `tests/`, `examples/` or `src/corpus/` directory."""
    full = _norm(path)
    cwd = _norm(os.getcwd())
    rel = os.path.relpath(full, cwd) if full.startswith(cwd + os.sep) else full
    parts = [p for p in rel.split(os.sep) if p]
    for i, p in enumerate(parts):
        if p in ("crates", "plugins"):
            rest = parts[i + 1:-1]
            if "tests" in rest or "examples" in rest:
                return False
            return not any(rest[j] == "src" and rest[j + 1] == "corpus" for j in range(len(rest) - 1))
    return False


_TOML_CACHE: dict = {}


def _manifest(d: str):
    """Parsed `d/Cargo.toml` ({} when unreadable), or None when absent."""
    p = os.path.join(d, "Cargo.toml")
    if p not in _TOML_CACHE:
        if not os.path.isfile(p):
            _TOML_CACHE[p] = None
        else:
            try:
                import tomllib
                with open(p, "rb") as fh:
                    _TOML_CACHE[p] = tomllib.load(fh)
            except (ImportError, OSError, ValueError):
                _TOML_CACHE[p] = {}
    return _TOML_CACHE[p]


def _manifest_dir(path: str):
    """Directory of the nearest `Cargo.toml` at or above `path`'s directory."""
    d = os.path.dirname(_norm(path))
    while True:
        if _manifest(d) is not None:
            return d
        up = os.path.dirname(d)
        if up == d:
            return None
        d = up


def crate_edition(path: str, default: str = "2024") -> str:
    """The Rust edition of the crate owning `path`.

    Reads the nearest `Cargo.toml`'s `[package] edition`; `edition.workspace =
    true` resolves through the nearest `Cargo.toml` (that one included) with a
    `[workspace]` table, via `[workspace.package] edition`. A package without
    an `edition` key is 2015 (Cargo's default); no manifest at all, or an
    unreadable one, gives `default`."""
    d = _manifest_dir(path)
    if d is None:
        return default
    m = _manifest(d) or {}
    pkg = m.get("package")
    if isinstance(pkg, dict):
        ed = pkg.get("edition")
        if isinstance(ed, str):
            return ed
        if ed is None:
            return "2015"
        if not (isinstance(ed, dict) and ed.get("workspace")):
            return default
    w = d
    while True:
        wm = _manifest(w)
        if wm and isinstance(wm.get("workspace"), dict):
            ed = wm["workspace"].get("package", {}).get("edition")
            return ed if isinstance(ed, str) else default
        up = os.path.dirname(w)
        if up == w:
            return default
        w = up


def package_values(path: str, excludes=(), cache=None) -> frozenset:
    """T7 fn names used as values anywhere in the package that owns `path`.

    The package is the nearest `Cargo.toml`'s directory, walked for `*.rs`
    (skipping `target/`, hidden directories, symlinks, nested packages and
    `--exclude`d paths); a file outside every package yields only itself."""
    d = _manifest_dir(path)
    cache = {} if cache is None else cache
    key = d or _norm(path)
    if key in cache:
        return cache[key]
    files = []
    if d is None:
        files = [path]
    else:
        for dp, dn, fn in os.walk(d):
            dn[:] = sorted(x for x in dn if x != "target" and not x.startswith(".")
                           and not os.path.isfile(os.path.join(dp, x, "Cargo.toml")))
            files += [os.path.join(dp, f) for f in sorted(fn)
                      if f.endswith(".rs") and not os.path.islink(os.path.join(dp, f))]
    sources = []
    for p in files:
        if excludes and _path_excluded(p, excludes):
            continue
        try:
            with open(p, encoding="utf-8", newline="") as fh:
                sources.append(Source(fh.read()))
        except (OSError, LexError, UnicodeDecodeError):
            continue
    names = {s.t[f.name].text for s in sources for f in _t7_fns(s)}
    values = set()
    for s in sources:
        values |= _t7_value_names(s, names)
    cache[key] = frozenset(values)
    return cache[key]


def t7_context(path: str, excludes=(), cache=None) -> T7Ctx:
    return T7Ctx(crate_edition(path), _t7_library(path), package_values(path, excludes, cache))


def select_outermost(cands: list[Candidate]) -> list[Candidate]:
    chosen, taken = [], []
    for c in sorted(cands, key=lambda c: (c.span[0], -(c.span[1] - c.span[0]))):
        if any(not (c.span[1] <= a or c.span[0] >= b) for a, b in taken):
            continue
        chosen.append(c)
        taken.append(c.span)
    return chosen


# ---------------------------------------------------------------------------
# Import edits
# ---------------------------------------------------------------------------

def _item_text(s: Source, item: Item) -> str:
    return s.text(item.start, item.end)


def _group_text(s: Source, items_text: list, multiline: bool, close_pos: int) -> str:
    if not multiline:
        return "{" + ", ".join(items_text) + "}"
    # Greedy-wrap at 100 columns, rustfmt-style; `cargo fmt` settles the order.
    ind = s.indent_of(close_pos)
    inner = ind + "    "
    lines, cur = [], ""
    for x in items_text:
        cand = f"{cur} {x}," if cur else f"{x},"
        if cur and len(inner) + len(cand) > 100:
            lines.append(cur)
            cand = f"{x},"
        cur = cand
    lines.append(cur)
    return "{\n" + "".join(f"{inner}{ln}\n" for ln in lines) + ind + "}"


def _render_item(s: Source, item: Item, drop: set, add: dict) -> list:
    """Re-render a use-tree item as the list of items it contributes to its parent group."""
    if item.group is None:
        text = _item_text(s, item)
        cut = text.rfind("::")
        prefix, last = (text[: cut + 2], text[cut + 2:]) if cut >= 0 else ("", text)
        names = [] if id(item) in drop else [last]
        names += [n for n in add.get(id(item), []) if n not in names]
        if not names:
            return []
        if len(names) == 1:
            return [prefix + names[0]]
        return [prefix + "{" + ", ".join(names) + "}"] if prefix else names
    grp = item.group
    pieces = []
    for sub in grp.items:
        pieces += _render_item(s, sub, drop, add)
    if not pieces:
        return []
    head = s.src[s.t[item.start].start:s.t[grp.open].start]
    if len(pieces) == 1 and pieces[0] != "self" and not pieces[0].startswith("self "):
        return [head + pieces[0]]
    multiline = "\n" in s.src[s.t[grp.open].start:s.t[grp.close].end]
    return [head + _group_text(s, pieces, multiline, s.t[grp.close].start)]


def import_edits(s: Source, adds: list, drops: set) -> list[Edit]:
    """Edits for `adds` and `drops` {(decl ordinal, leaf path, name)}.

    An add is ("add", decl ordinal, anchor leaf path, new name) — import the
    name next to the anchor — or ("new", decl ordinal, full path) — a new `use`
    statement right after that declaration."""
    edits = []
    by_decl: dict[int, tuple[set, dict]] = {}
    added = set()   # (scope, name): one import per name per scope, or E0252
    for add in adds:
        if add[0] == "new":
            _, ordn, full = add
            d = s.decls[ordn]
            if (d.scope, full[-1]) in added:
                continue
            added.add((d.scope, full[-1]))
            ind = s.indent_of(s.t[d.stmt_start].start)
            end = s.t[d.semi].end
            edits.append(Edit(end, end, f"\n{ind}use {'::'.join(full)};"))
            continue
        _, ordn, path, new = add
        if (s.decls[ordn].scope, new) in added:
            continue
        added.add((s.decls[ordn].scope, new))
        for leaf in s.decls[ordn].leaves:
            if leaf.path == path and not leaf.aliased:
                _, add_map = by_decl.setdefault(ordn, (set(), {}))
                names = add_map.setdefault(id(leaf.item), [])
                if new not in names:
                    names.append(new)
                break
    for ordn, path, name in drops:
        for leaf in s.decls[ordn].leaves:
            if leaf.path == path and leaf.name == name and not leaf.aliased:
                drop_ids, _ = by_decl.setdefault(ordn, (set(), {}))
                drop_ids.add(id(leaf.item))
    for ordn, (drop_ids, add_map) in by_decl.items():
        d = s.decls[ordn]
        root = d.root
        pieces = _render_item(s, root, drop_ids, add_map)
        if len(pieces) > 1:
            pieces = ["{" + ", ".join(pieces) + "}"]
        if not pieces:
            a = s.t[d.stmt_start].start
            b = s.t[d.semi].end
            # Swallow the whole line when the statement sits alone on it.
            ls = s.src.rfind("\n", 0, a) + 1
            le = s.src.find("\n", b)
            le = len(s.src) if le < 0 else le
            if not s.src[ls:a].strip() and not s.src[b:le].strip():
                a, b = ls, min(le + 1, len(s.src))
            edits.append(Edit(a, b, ""))
        else:
            edits.append(Edit(s.t[root.start].start, s.t[root.end].end, pieces[0]))
    return edits


def apply_edits(src: str, edits: list[Edit]) -> str:
    for e in sorted(edits, key=lambda e: e.start, reverse=True):
        src = src[: e.start] + e.text + src[e.end:]
    return src


# ---------------------------------------------------------------------------
# Driver
# ---------------------------------------------------------------------------

MAX_PASSES = 64


@dataclass
class Result:
    text: str
    before: int
    after: int
    candidates: list
    notes: list
    kept: int = 0
    excluded: list = field(default_factory=list)   # T7 exclusions of the original text


def rewrite(src: str, apis: Apis, t5: bool = False, t7: bool = False,
            ctx: "T7Ctx | None" = None) -> Result:
    """Rewrite `src` to a fixpoint; raises LexError on unparseable input.

    `t5` turns on the opt-in homogeneous list rule T5, `t7` the opt-in helper
    return rule T7 with the crate facts in `ctx` (default: edition 2024, not a
    library source, no value uses outside this text)."""
    original = Source(src)
    before = original.erasure_count()
    cands0, notes0 = find_candidates(original, apis, t5, t7, ctx)
    kept0 = original.kept
    excluded0 = list(original.t7_excluded)
    text = src
    for _ in range(MAX_PASSES):
        s = Source(text)
        cands, _ = find_candidates(s, apis, t5, t7, ctx)
        chosen = select_outermost(cands)
        if not chosen:
            break
        edits = [e for c in chosen for e in c.edits]
        adds = [imp for c in chosen for imp in c.imports]
        if adds:
            edits += import_edits(s, adds, set())
        text = apply_edits(text, edits)
    if text != src:
        old_counts = original.usage_counts(REMOVABLE_IMPORTS)
        final = Source(text)
        new_counts = final.usage_counts(REMOVABLE_IMPORTS)
        drops = set()
        origin = _align_decls(original, final)
        for d in final.decls:
            if d.is_pub or d.ordinal not in origin:
                continue
            for leaf in d.leaves:
                key = (d.ordinal, leaf.path, leaf.name)
                if leaf.aliased or leaf.name not in REMOVABLE_IMPORTS:
                    continue
                old = old_counts.get((origin[d.ordinal],) + key[1:], 0)
                if old > 0 and new_counts.get(key, 0) == 0:
                    drops.add(key)
        if drops:
            text = apply_edits(text, import_edits(final, [], drops))
    after = Source(text).erasure_count() if text != src else before
    return Result(text, before, after, cands0, sorted(set(notes0)), kept0, excluded0)


def _align_decls(original: Source, final: Source) -> dict:
    """{final decl ordinal: original decl ordinal}.

    Rewrites only add leaves to existing `use` declarations or insert new ones
    (`use frust::column;` in a test module, `use frust::View;`), so an in-order
    greedy match — the original's leaves a subset of the final's, same globs and
    visibility — pairs every surviving declaration and leaves the inserted ones
    unpaired."""
    out, i = {}, 0
    for d in final.decls:
        if i >= len(original.decls):
            break
        o = original.decls[i]
        if o.is_pub == d.is_pub and o.globs == d.globs and \
                {(lf.path, lf.name) for lf in o.leaves} <= {(lf.path, lf.name) for lf in d.leaves}:
            out[d.ordinal] = o.ordinal
            i += 1
    return out


def _norm(p):
    return os.path.normpath(os.path.abspath(p))


def _path_excluded(path, excluded):
    n = _norm(path)
    return any(n == x or n.startswith(x + os.sep) for x in excluded)


def _write_atomic(path: str, text: str) -> None:
    """Replace `path` (resolving symlinks) with `text`, never leaving a partial file."""
    real = os.path.realpath(path)
    tmp = tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", newline="", dir=os.path.dirname(real),
        prefix=".codemod-", suffix=".tmp", delete=False)
    try:
        with tmp:
            tmp.write(text)
            shutil.copymode(real, tmp.name)
            tmp.flush()
            os.fsync(tmp.fileno())
        os.replace(tmp.name, real)
    except BaseException:
        with contextlib.suppress(OSError):
            os.unlink(tmp.name)
        raise


def iter_rs(paths):
    for p in paths:
        if os.path.isdir(p):
            for dp, dn, fn in os.walk(p):
                dn[:] = sorted(d for d in dn if d != "target" and not d.startswith("."))
                for f in sorted(fn):
                    if f.endswith(".rs") and not os.path.islink(os.path.join(dp, f)):
                        yield os.path.join(dp, f)
        else:
            yield p


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("paths", nargs="+", help=".rs files or directories")
    ap.add_argument("--check", action="store_true", help="list remaining candidates; exit 1 if any")
    ap.add_argument("--write", action="store_true", help="rewrite files in place")
    ap.add_argument("--stats", action="store_true", help="print per-file erasure-call counts before/after")
    ap.add_argument("--strict", action="store_true",
                    help="--check exits 2 when any site was shadow-skipped (candidates still win: 1)")
    ap.add_argument("--t5", action="store_true",
                    help="opt in to rule T5: drop any() from vec![..] list arguments whose "
                         "elements all share one head (callee path + trailing method names); "
                         "T5 judges heads, not types: for a same-head list whose elements differ "
                         "in type keep any() and mark the vec![ line with `// erasure: keep <why>` "
                         "(or bind the vec to a local, or use the fluent builder)")
    ap.add_argument("--t7", action="store_true",
                    help="opt in to rule T7: a fn returning AnyView<S> whose body ends in one "
                         "erasure call returns impl View<S> instead (+ use<..> only in a "
                         "pre-2024 crate when the fn has a lifetime or reference parameter); "
                         "trait fns, multi-arm bodies, recursive fns or fns used as values, and "
                         "pub fns of crates/ and plugins/ library sources are listed as "
                         "T7-trait/T7-arms/T7-ref/T7-public exclusions, never rewritten and never "
                         "counted toward the exit code; `// erasure: keep <why>` on the signature "
                         "line (or alone above it) exempts a fn")
    ap.add_argument("--exclude", action="append", default=[], metavar="PATH",
                    help="skip PATH, a file or directory, by whole path components (repeatable)")
    ap.add_argument("--apis", default=DEFAULT_APIS, help="name list (default: erasing_apis.txt beside this script)")
    args = ap.parse_args(argv)
    check = args.check or not (args.write or args.stats)
    apis = load_apis(args.apis)

    excludes = [_norm(x) for x in args.exclude]
    remaining = 0
    skipped = 0
    shadow = 0
    refused = 0
    total_before = total_after = 0
    total_t5 = 0
    total_t7 = 0
    total_kept = 0
    excluded_t7 = 0
    total_buckets = dict.fromkeys(T7_BUCKETS, 0)
    pkg_cache: dict = {}
    for path in iter_rs(args.paths):
        if excludes and _path_excluded(path, excludes):
            continue
        if os.path.islink(path):
            print(f"{path}: skipped, refusing to follow a symlink", file=sys.stderr)
            skipped += 1
            refused += 1
            continue
        if not os.path.isfile(path):
            print(f"{path}: no such file", file=sys.stderr)
            return 2
        try:
            with open(path, encoding="utf-8", newline="") as fh:
                src = fh.read()
            ctx = t7_context(path, excludes, pkg_cache) if args.t7 else None
            res = rewrite(src, apis, args.t5, args.t7, ctx)
        except (LexError, UnicodeDecodeError) as exc:
            print(f"{path}: skipped, cannot tokenise: {exc}", file=sys.stderr)
            skipped += 1
            continue
        total_before += res.before
        total_after += res.after
        if args.write and res.text != src:
            _write_atomic(path, res.text)
        if check:
            if args.write:
                post = Source(res.text)
                cands, notes = find_candidates(post, apis, args.t5, args.t7, ctx)
                notes = sorted(set(notes))
                excluded = post.t7_excluded
            else:
                cands, notes, excluded = res.candidates, res.notes, res.excluded
            for c in sorted(cands, key=lambda c: (c.line, c.span[0])):
                print(f"{path}:{c.line}: {c.rule} {c.desc}")
            for line, bucket, name, reason in excluded:
                print(f"{path}:{line}: {bucket} excluded: {name} ({reason})")
            for line, msg in notes:
                print(f"{path}:{line}: {msg}")
            remaining += len(cands)
            shadow += len(notes)
            excluded_t7 += len(excluded)
        if args.stats:
            print(f"{path}: erasure calls before={res.before} after={res.after}")
            total_kept += res.kept
            if res.kept:
                print(f"{path}: kept={res.kept}")
            if args.t5:
                n_t5 = sum(1 for c in res.candidates if c.rule == "T5")
                total_t5 += n_t5
                if n_t5:
                    print(f"{path}: T5 candidates={n_t5}")
            if args.t7:
                n_t7 = sum(1 for c in res.candidates if c.rule == "T7")
                total_t7 += n_t7
                buckets = dict.fromkeys(T7_BUCKETS, 0)
                for _, bucket, _, _ in res.excluded:
                    buckets[bucket] += 1
                    total_buckets[bucket] += 1
                if n_t7:
                    print(f"{path}: T7 candidates={n_t7}")
                if res.excluded:
                    print(f"{path}: T7 excluded " + " ".join(f"{b}={n}" for b, n in buckets.items()))
    if args.stats:
        print(f"total: erasure calls before={total_before} after={total_after}")
        print(f"total: kept={total_kept}")
        if args.t5:
            print(f"total: T5 candidates={total_t5}")
        if args.t7:
            print(f"total: T7 candidates={total_t7}")
            print("total: T7 excluded " + " ".join(f"{b}={n}" for b, n in total_buckets.items()))
    if check:
        if args.t7:
            print(f"{excluded_t7} T7 exclusion(s)")
        print(f"{shadow} shadow skip(s)")
        print(f"{remaining} candidate(s)" if remaining else "no candidates")
        if skipped:
            print(f"{skipped} file(s) skipped")
        if remaining:
            return 1
        return 2 if skipped or (args.strict and shadow) else 0
    return 2 if refused else 0


if __name__ == "__main__":
    sys.exit(main())
