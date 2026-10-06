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
        [--apis FILE] [--exclude PREFIX]... PATH...

PATH is a `.rs` file or a directory (recursed for `*.rs`, skipping `target/`).
`--check` (the default) lists `file:line` of every remaining candidate and
exits 1 if there is one, 0 otherwise; skipped-site notes do not affect the exit
code. A file that cannot be tokenised or decoded is reported as skipped, counted,
and makes `--check` exit 2 when no candidate was found (candidates still win: 1).
`--exclude PREFIX` (repeatable; repository-relative or absolute, prefix match on
the normalised path) drops matching files from `--check`, `--write` and
`--stats`; they are neither read nor counted. `--write` rewrites in place (iterating to a fixpoint, so a second run is a
no-op). `--stats` prints per-file erasure-call counts before/after.
"""

from __future__ import annotations

import argparse
import bisect
import os
import re
import sys
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
        self.bare_any_ok = not self._defines_foreign_any()
        self.decls = self._parse_uses()
        self.local_types = self._local_type_names()
        self.local_fns = {t[k + 1].text for k in range(len(t) - 1)
                          if t[k].text == "fn" and t[k + 1].kind == IDENT}

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


def find_candidates(s: Source, apis: Apis):
    """Return (candidates, notes). Candidates may overlap; callers pick outermost."""
    t = s.t
    cands: list[Candidate] = []
    notes: list[tuple[int, str]] = []
    for k, tok in enumerate(t):
        if tok.kind != IDENT or not s.is_p(k + 1, "("):
            continue
        key, qual = _call_key(s, k)
        if key is None or s.forbidden(k):
            continue
        o = k + 1
        if key in apis.slot or key in apis.builder:
            if "::" not in key and not key.startswith(".") and _excluded(s, apis, key, qual, tok.start):
                continue
        if key in apis.slot:
            _t1(s, key, k, o, cands)
        if key in apis.builder:
            _t4(s, key, k, o, cands)
        if key in BUILDER_OF:
            _t2(s, key, k, o, qual, cands, notes)
        if key == "FlexView::new":
            _t3(s, k, o, qual, cands, notes)
    return cands, notes


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


def rewrite(src: str, apis: Apis) -> Result:
    """Rewrite `src` to a fixpoint; raises LexError on unparseable input."""
    original = Source(src)
    before = original.erasure_count()
    cands0, notes0 = find_candidates(original, apis)
    text = src
    for _ in range(MAX_PASSES):
        s = Source(text)
        cands, _ = find_candidates(s, apis)
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
        if len(final.decls) == len(original.decls):
            for d in final.decls:
                if d.is_pub:
                    continue
                for leaf in d.leaves:
                    key = (d.ordinal, leaf.path, leaf.name)
                    if leaf.aliased or leaf.name not in REMOVABLE_IMPORTS:
                        continue
                    if old_counts.get(key, 0) > 0 and new_counts.get(key, 0) == 0:
                        drops.add(key)
        if drops:
            text = apply_edits(text, import_edits(final, [], drops))
    after = Source(text).erasure_count() if text != src else before
    return Result(text, before, after, cands0, sorted(set(notes0)))


def _norm(p):
    return os.path.normpath(os.path.abspath(p))


def _path_excluded(path, prefixes):
    n = _norm(path)
    return any(n.startswith(x) for x in prefixes)


def iter_rs(paths):
    for p in paths:
        if os.path.isdir(p):
            for dp, dn, fn in os.walk(p):
                dn[:] = sorted(d for d in dn if d != "target" and not d.startswith("."))
                for f in sorted(fn):
                    if f.endswith(".rs"):
                        yield os.path.join(dp, f)
        else:
            yield p


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("paths", nargs="+", help=".rs files or directories")
    ap.add_argument("--check", action="store_true", help="list remaining candidates; exit 1 if any")
    ap.add_argument("--write", action="store_true", help="rewrite files in place")
    ap.add_argument("--stats", action="store_true", help="print per-file erasure-call counts before/after")
    ap.add_argument("--exclude", action="append", default=[], metavar="PREFIX",
                    help="skip files whose normalised path starts with PREFIX (repeatable)")
    ap.add_argument("--apis", default=DEFAULT_APIS, help="name list (default: erasing_apis.txt beside this script)")
    args = ap.parse_args(argv)
    check = args.check or not (args.write or args.stats)
    apis = load_apis(args.apis)

    excludes = [_norm(x) for x in args.exclude]
    remaining = 0
    skipped = 0
    total_before = total_after = 0
    for path in iter_rs(args.paths):
        if excludes and _path_excluded(path, excludes):
            continue
        if not os.path.isfile(path):
            print(f"{path}: no such file", file=sys.stderr)
            return 2
        try:
            with open(path, encoding="utf-8", newline="") as fh:
                src = fh.read()
            res = rewrite(src, apis)
        except (LexError, UnicodeDecodeError) as exc:
            print(f"{path}: skipped, cannot tokenise: {exc}", file=sys.stderr)
            skipped += 1
            continue
        total_before += res.before
        total_after += res.after
        if args.write and res.text != src:
            with open(path, "w", encoding="utf-8", newline="") as fh:
                fh.write(res.text)
        if check:
            if args.write:
                post = Source(res.text)
                cands, notes = find_candidates(post, apis)
                notes = sorted(set(notes))
            else:
                cands, notes = res.candidates, res.notes
            for c in sorted(cands, key=lambda c: (c.line, c.span[0])):
                print(f"{path}:{c.line}: {c.rule} {c.desc}")
            for line, msg in notes:
                print(f"{path}:{line}: {msg}")
            remaining += len(cands)
        if args.stats:
            print(f"{path}: erasure calls before={res.before} after={res.after}")
    if args.stats:
        print(f"total: erasure calls before={total_before} after={total_after}")
    if check:
        print(f"{remaining} candidate(s)" if remaining else "no candidates")
        if skipped:
            print(f"{skipped} file(s) skipped")
        return 1 if remaining else (2 if skipped else 0)
    return 0


if __name__ == "__main__":
    sys.exit(main())
