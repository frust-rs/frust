"""Unit tests for frust_any_codemod.py (stdlib unittest, inline fixtures).

Run from the repository root:

    python3 -m unittest scripts/codemod/test_frust_any_codemod.py
"""

import contextlib
import io
import os
import sys
import tempfile
import textwrap
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import frust_any_codemod as cm  # noqa: E402

APIS = cm.load_apis()


def run(src: str) -> str:
    return cm.rewrite(textwrap.dedent(src), APIS).text


def check(src: str):
    res = cm.rewrite(textwrap.dedent(src), APIS)
    return [c.rule for c in res.candidates], res.notes


GLOB = "use frust::*;\n"


def run_g(src: str) -> str:
    """Rewrite `src` with a glob import in scope, so builder chains resolve."""
    out = run(GLOB + textwrap.dedent(src))
    assert out.startswith(GLOB), out
    return out[len(GLOB):]


def check_g(src: str):
    return check(GLOB + textwrap.dedent(src))


class NameListTest(unittest.TestCase):
    def test_sections_load(self):
        self.assertIn(".child", APIS.slot)
        self.assertIn("inflexible", APIS.slot)
        self.assertIn("ScrollView::new", APIS.slot)
        self.assertIn(".push", APIS.builder)
        self.assertIn("Route::new", APIS.builder)
        self.assertIn(("frust_material", "list_view"), APIS.exclude)
        self.assertNotIn("breadcrumb_item", APIS.slot)  # returns V: not type-preserving
        self.assertNotIn(".children", APIS.slot)        # a list parameter

    def test_unknown_section_rejected(self):
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as fh:
            fh.write("[bogus]\nfoo\n")
        try:
            with self.assertRaises(ValueError):
                cm.load_apis(fh.name)
        finally:
            os.unlink(fh.name)


class LexerTest(unittest.TestCase):
    def kinds(self, src):
        return [(t.kind, t.text) for t in cm.lex(src) if t.kind not in cm.TRIVIA]

    def test_raw_and_byte_strings(self):
        toks = self.kinds('r#"a "quoted" any(x)"# br"b" b"c\\"d" c"e"')
        self.assertEqual([k for k, _ in toks], [cm.STR] * 4)

    def test_lifetime_vs_char(self):
        toks = self.kinds("fn f<'a>(x: &'a str) -> char { '(' } '\\'' b'x' '\\u{1F600}'")
        self.assertIn((cm.LIFETIME, "'a"), toks)
        self.assertIn((cm.CHAR, "'('"), toks)
        self.assertIn((cm.CHAR, "'\\''"), toks)
        self.assertIn((cm.CHAR, "b'x'"), toks)
        self.assertIn((cm.CHAR, "'\\u{1F600}'"), toks)

    def test_nested_block_comment(self):
        toks = cm.lex("a /* x /* any( */ y */ b")
        self.assertEqual([t.text for t in toks if t.kind == cm.IDENT], ["a", "b"])

    def test_char_paren_does_not_unbalance(self):
        cm.Source("fn f() -> char { if true { '(' } else { ')' } }")

    def test_unbalanced_raises(self):
        with self.assertRaises(cm.LexError):
            cm.Source("fn f() { (")


class T1Test(unittest.TestCase):
    def test_method_slot(self):
        self.assertEqual(run("x.child(any(text(\"a\")));"), "x.child(text(\"a\"));")

    def test_free_fn_slot(self):
        self.assertEqual(run("let c = container(any(text(\"a\")));"),
                         "let c = container(text(\"a\"));")

    def test_qualified_free_fn_and_erasure_spellings(self):
        src = "a(frust_material::fab(frust::any(i), f)); b(Padding(e, AnyView::new(v)));"
        self.assertEqual(run(src), "a(frust_material::fab(i, f)); b(Padding(e, v));")

    def test_associated_fn_entry(self):
        self.assertEqual(run("ScrollView::new(any(body))"), "ScrollView::new(body)")
        # A `Type::name` call never matches a free-fn entry.
        self.assertEqual(run("Foo::card(any(x))"), "Foo::card(any(x))")

    def test_multi_arg_and_option_slot(self):
        src = "flexible(1, any(a)); keyed(k, any(b)); s.flex(2, any(c)); s.overlay(Some(any(d)));"
        self.assertEqual(run(src), "flexible(1, a); keyed(k, b); s.flex(2, c); s.overlay(Some(d));")

    def test_nested_any(self):
        self.assertEqual(run("s.child(any(any(x)))"), "s.child(x)")

    def test_trailing_comma_inside_any(self):
        src = """\
        s.child(any(
            text("long"),
        ))
        """
        self.assertEqual(run(src), 's.child(text("long"))\n')

    def test_untouched_forms(self):
        for src in (
            "s.child(any(x).boxed())",         # not the whole argument
            "s.child(any::<S, _>(x))",         # turbofish carries the State type
            "s.unknown(any(x))",               # not in the name list
            "s.children(vec![any(a), any(b)])",  # list parameter
            "card(vec![any(a), any(b)])",      # list form of a dual-use name
            "s.child(x.iter().any(|v| v))",    # iterator .any()
            "s.child(any(a, b))",              # not a one-argument erasure
            "s.child(std::any(x))",            # not a frust path
        ):
            self.assertEqual(run(src), src, src)

    def test_line_comment_inside_any_is_not_swallowed(self):
        src = "s.child(any(x // keep\n)).end()"
        out = run(src)
        self.assertIn("x // keep\n", out)
        self.assertTrue(out.rstrip().endswith(").end()"))

    def test_foreign_any_fn_disables_bare_any(self):
        src = "fn any(x: i32) -> bool { x > 0 }\nfn f() { s.child(any(3)); }"
        self.assertEqual(run(src), src)

    def test_attributes_cfg_and_macros_untouched(self):
        for src in (
            '#[cfg(any(test, feature = "x"))]\nfn f() {}',
            '#[cfg_attr(any(test), allow(dead_code))]\nfn f() {}',
            'let on = cfg!(any(unix, windows));',
            'assert!(s.child(any(x)).is_ok());',
            'let t = format!("{}", s.child(any(x)));',
            'let s = r#"s.child(any(x))"#;',
            'let s = "s.child(any(x))";',
        ):
            self.assertEqual(run(src), src, src)

    def test_inside_vec_macro_is_rewritten(self):
        self.assertEqual(run("vec![inflexible(any(a))]"), "vec![inflexible(a)]")

    def test_method_named_like_free_fn_untouched(self):
        # `card` is a free-fn entry; `.card(..)` is a different (method) call.
        self.assertEqual(run("s.card(any(x))"), "s.card(any(x))")


class T4Test(unittest.TestCase):
    def test_closure_expression_body(self):
        self.assertEqual(run("navigator(&c, || any(home()))"), "navigator(&c, || home())")

    def test_closure_block_body(self):
        self.assertEqual(run("nav.push(move || { any(page(1)) })"), "nav.push(move || { page(1) })")

    def test_closure_with_params_and_assoc_entry(self):
        self.assertEqual(run('Route::new("/", |p| any(home(p)))'), 'Route::new("/", |p| home(p))')
        self.assertEqual(run("ListView::builder(9, 4.0, |i| any(row_at(i)))"),
                         "ListView::builder(9, 4.0, |i| row_at(i))")

    def test_left_alone(self):
        for src in (
            "nav.push(|| { let p = page(); any(p) })",       # multi-statement
            "nav.push(|| if a { any(x) } else { any(y) })",  # branching
            "nav.push(|| -> AnyView<S> { any(x) })",         # explicit return type
            "list_view(9, 4.0, |i| any::<(), _>(row(i)))",   # turbofish
            "frust_material::list_view(9, 4.0, |i| any(r(i)))",  # excluded wrapper
        ):
            self.assertEqual(run(src), src, src)

    def test_excluded_through_import(self):
        src = "use frust_material::list_view;\nfn f() { list_view(9, 4.0, |i| any(r(i))); }\n"
        self.assertEqual(run(src), src)


class T2Test(unittest.TestCase):
    def test_single_line(self):
        self.assertEqual(run_g("framed(Column(vec![any(a), b]))"), "framed(column().child(a).child(b))")

    def test_row_and_stack(self):
        self.assertEqual(run_g("Row(vec![a]); Stack(vec![b, any(c)]);"),
                         "row().child(a); stack().child(b).child(c);")

    def test_multi_line_with_comments_and_trailing_call(self):
        src = """\
        let v = Column(vec![
            // header
            any(text("a")), // first
            /* middle */ any(Row(vec![text("b")])),
            text("c"),
        ])
        .cross_axis(c);
        """
        want = """\
        let v = column()
            // header
            .child(text("a")) // first
            /* middle */ .child(row().child(text("b")))
            .child(text("c"))
        .cross_axis(c);
        """
        self.assertEqual(run_g(src), textwrap.dedent(want))

    def test_trailing_line_comment_before_close(self):
        src = "let v = Column(vec![\n    a,\n    // end\n]);\n"
        out = run_g(src)
        self.assertEqual(out, "let v = column()\n    .child(a)\n    // end\n;\n")

    def test_non_literal_and_repeat_untouched(self):
        for src in ("Column(children)", "Column(vec![x; 3])", "Column::<S, _>(vec![a])",
                    "Column(items.iter().map(f).collect())"):
            self.assertEqual(run_g(src), src, src)

    def test_turbofish_elements_split_correctly(self):
        self.assertEqual(run_g("Column(vec![any::<(), _>(a), any::<(), _>(b)])"),
                         "column().child(any::<(), _>(a)).child(any::<(), _>(b))")

    def test_qualified_container(self):
        self.assertEqual(run_g("frust::Column(vec![a])"), "frust::column().child(a)")

    def test_local_struct_named_row_untouched(self):
        src = "struct Row(Vec<u8>);\nfn f() -> Row { Row(vec![1, 2]) }\n"
        self.assertEqual(run_g(src), src)

    def test_shadowing_binding_skips_with_note(self):
        src = "fn f(row: u32) -> V { Row(vec![a]) }\n"
        self.assertEqual(run_g(src), src)
        rules, notes = check_g(src)
        self.assertEqual(rules, [])
        self.assertTrue(any("local binding `row`" in n for _, n in notes))

    def test_free_fn_shadow_skips_with_note(self):
        src = "fn column(x: u32) -> u32 { x }\nfn f() -> V { Column(vec![a, b]) }\n"
        self.assertEqual(run_g(src), src)
        rules, notes = check_g(src)
        self.assertEqual(rules, [])
        self.assertTrue(any("local fn `column`" in n for _, n in notes))

    def test_method_shadow_skips_with_note(self):
        src = ("use frust::{any, Column, View};\n"
               "impl P { fn column(&self) -> u32 { 1 } }\n"
               "fn f() -> V { Column(vec![any(a()), any(b())]) }\n")
        out = run(src)
        self.assertIn("Column(vec![", out)
        self.assertNotIn("column().child", out)
        self.assertIn("Column", out.split("\n")[0])
        _, notes = check(src)
        self.assertTrue(any("local fn `column`" in n for _, n in notes))

    def test_imported_builder_plus_method_rewrites(self):
        src = ("use frust::{column, Column};\n"
               "impl P { fn column(&self) -> u32 { 1 } }\n"
               "fn f() -> V { Column(vec![a, b]) }\n")
        out = run(src)
        self.assertIn("column().child(a).child(b)", out)

    def test_let_initialiser_is_not_shadowed(self):
        src = "fn f() -> V { let column = Column(vec![a]); framed(column) }\n"
        self.assertEqual(run_g(src), "fn f() -> V { let column = column().child(a); framed(column) }\n")


class T3Test(unittest.TestCase):
    def test_all_child_forms(self):
        src = """\
        let r = FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(any(a)),
                flexible(2, b),
                helper(),
                existing,
            ],
        )
        .cross_axis(c);
        """
        want = """\
        let r = row()
                .child(a)
                .flex(2, b)
                .push(helper())
                .push(existing)
        .cross_axis(c);
        """
        self.assertEqual(run_g(src), textwrap.dedent(want))

    def test_keyed_list_and_vertical(self):
        self.assertEqual(run_g("FlexView::new(Axis::Vertical, vec![keyed(1, any(a)), keyed(2, b)])"),
                         "column().keyed(1, a).keyed(2, b)")

    def test_inner_comments_kept(self):
        self.assertEqual(run_g("FlexView::new(Axis::Vertical, vec![flexible(1 /* n */, x)])"),
                         "column().flex(1 /* n */, x)")

    def test_mixed_keyed_left_alone_and_noted(self):
        src = "FlexView::new(Axis::Vertical, vec![keyed(1, a), inflexible(b)])"
        self.assertEqual(run_g(src), src)
        rules, notes = check_g(src)
        self.assertNotIn("T3", rules)
        self.assertTrue(any("mixes keyed" in n for _, n in notes))

    def test_non_literal_axis_or_list_untouched(self):
        for src in ("FlexView::new(axis, vec![inflexible(a)])",
                    "FlexView::new(Axis::Vertical, children)"):
            self.assertEqual(run_g(src), src, src)


class ImportTest(unittest.TestCase):
    def test_builder_added_and_unused_imports_dropped(self):
        src = """\
        use frust::{Column, any, text};

        fn f() -> V {
            Column(vec![any(text("a")), any(text("b"))])
        }
        """
        want = """\
        use frust::{column, text};

        fn f() -> V {
            column().child(text("a")).child(text("b"))
        }
        """
        self.assertEqual(run(src), textwrap.dedent(want))

    def test_still_used_imports_kept(self):
        src = """\
        use frust::{AnyView, Column, any, text};

        fn f() -> AnyView<()> {
            any(Column(vec![any(text("a"))]))
        }
        fn g() -> V { Column(items) }
        """
        want = """\
        use frust::{AnyView, Column, column, any, text};

        fn f() -> AnyView<()> {
            any(column().child(text("a")))
        }
        fn g() -> V { Column(items) }
        """
        self.assertEqual(run(src), textwrap.dedent(want))

    def test_flexview_imports(self):
        src = """\
        use frust_widgets::{Axis, FlexView, inflexible, text};
        use frust_widgets::Row;

        fn f() -> V {
            FlexView::new(Axis::Horizontal, vec![inflexible(text("a"))])
        }
        fn g() -> V { Row(vec![text("b")]) }
        """
        out = run(src)
        self.assertIn("use frust_widgets::{row, text};", out)
        self.assertNotIn("use frust_widgets::Row;", out)
        self.assertEqual(out.count("row"), 3)   # one import, two calls: never imported twice

    def test_single_path_import_promoted_to_group(self):
        src = "use frust::Stack;\nfn f() -> V { Stack(vec![a]) }\nfn g() -> V { Stack(xs) }\n"
        self.assertEqual(run(src), "use frust::{Stack, stack};\nfn f() -> V { stack().child(a) }\n"
                                   "fn g() -> V { Stack(xs) }\n")

    def test_test_module_gets_its_own_import(self):
        src = """\
        use frust::{Column, text};

        pub fn f() -> V { Column(xs) }

        #[cfg(test)]
        mod tests {
            use super::*;

            fn t() -> V { Column(vec![text("a")]) }
        }
        """
        out = run(src)
        self.assertIn("use frust::{Column, text};", out)
        self.assertIn("    use super::*;\n    use frust::column;\n", out)

    def test_pub_use_never_edited(self):
        src = "pub use frust::{Column, any};\nfn f() -> V { Column(vec![any(a)]) }\n"
        out = run(src)
        self.assertTrue(out.startswith("pub use frust::{Column, any};"))

    def test_unresolvable_builder_noted(self):
        src = "fn f() -> V { Column(vec![a]) }\n"
        self.assertEqual(run(src), src)
        _, notes = check(src)
        self.assertTrue(any("cannot resolve" in n for _, n in notes))

    def test_glob_import_resolves(self):
        src = "use frust::*;\nfn f() -> V { Column(vec![any(a)]) }\n"
        self.assertEqual(run(src), "use frust::*;\nfn f() -> V { column().child(a) }\n")


class DriverFixture:
    FIXTURE = textwrap.dedent("""\
        use frust::{Column, any, text};

        fn f() -> V {
            Column(vec![
                any(text("a")),
                any(text("b")),
            ])
        }
        """)

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.path = os.path.join(self.dir.name, "src", "lib.rs")
        os.makedirs(os.path.dirname(self.path))
        with open(self.path, "w", encoding="utf-8") as fh:
            fh.write(self.FIXTURE)
        # A file under target/ must be skipped by directory recursion.
        os.makedirs(os.path.join(self.dir.name, "target"))
        with open(os.path.join(self.dir.name, "target", "gen.rs"), "w", encoding="utf-8") as fh:
            fh.write("fn g() -> V { Column(vec![any(a)]) }\n")

    def tearDown(self):
        self.dir.cleanup()

    def main(self, *argv):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = cm.main(list(argv))
        return code, out.getvalue()


class DriverTest(DriverFixture, unittest.TestCase):
    def test_check_exit_codes_and_idempotence(self):
        code, out = self.main("--check", self.dir.name)
        self.assertEqual(code, 1)
        self.assertIn("lib.rs:4: T2", out)
        self.assertNotIn("gen.rs", out)

        code, _ = self.main("--write", self.dir.name)
        self.assertEqual(code, 0)
        with open(self.path, encoding="utf-8") as fh:
            once = fh.read()
        self.assertIn(".child(text(\"a\"))", once)

        code, out = self.main("--check", self.dir.name)
        self.assertEqual(code, 0, out)

        self.main("--write", self.dir.name)
        with open(self.path, encoding="utf-8") as fh:
            self.assertEqual(fh.read(), once)   # a second --write is a no-op

    def test_exclude_prefix(self):
        code, out = self.main("--check", self.dir.name, "--exclude", os.path.join(self.dir.name, "src"))
        self.assertEqual(code, 0, out)
        self.assertNotIn("lib.rs", out)
        code, _ = self.main("--write", self.dir.name, "--exclude", self.path)
        with open(self.path, encoding="utf-8") as fh:
            self.assertEqual(fh.read(), self.FIXTURE)   # excluded: never written
        code, out = self.main("--stats", self.dir.name, "--exclude", self.path)
        self.assertNotIn("lib.rs", out)

    def test_skipped_files_exit_2(self):
        os.remove(self.path)
        bad = os.path.join(self.dir.name, "src", "bad.rs")
        with open(bad, "wb") as fh:
            fh.write(b"fn f() { // \xff\xfe\n}\n")
        code, out = self.main("--check", self.dir.name)
        self.assertEqual(code, 2)
        self.assertIn("1 file(s) skipped", out)
        lex = os.path.join(self.dir.name, "src", "lex.rs")
        with open(lex, "w", encoding="utf-8") as fh:
            fh.write("fn f() { (\n")
        code, out = self.main("--check", self.dir.name)
        self.assertEqual(code, 2)
        self.assertIn("2 file(s) skipped", out)
        code, _ = self.main("--check", self.dir.name, "--exclude", bad, "--exclude", lex)
        self.assertEqual(code, 0)

    def test_candidates_beat_skipped(self):
        with open(os.path.join(self.dir.name, "src", "bad.rs"), "wb") as fh:
            fh.write(b"\xff")
        code, _ = self.main("--check", self.dir.name)
        self.assertEqual(code, 1)

    def test_stats(self):
        code, out = self.main("--stats", self.path)
        self.assertEqual(code, 0)
        self.assertIn("erasure calls before=2 after=0", out)
        with open(self.path, encoding="utf-8") as fh:
            self.assertEqual(fh.read(), self.FIXTURE)   # --stats alone never writes

    def test_idempotent_on_every_fixture_kind(self):
        src = textwrap.dedent("""\
            use frust::{Axis, Column, FlexView, any, inflexible, text};
            fn f() -> V {
                FlexView::new(Axis::Vertical, vec![
                    inflexible(any(Column(vec![any(any(text("a")))]))),
                ])
            }
            """)
        once = cm.rewrite(src, APIS).text
        self.assertEqual(cm.rewrite(once, APIS).text, once)
        self.assertNotIn("any(", once)


class HygieneTest(DriverFixture, unittest.TestCase):
    """Driver hygiene: atomic write, symlinks, --exclude, --strict (reuses DriverTest's fixture)."""

    SHADOWED = textwrap.dedent("""\
        use frust::{Column, text};

        fn f() -> V {
            let column = 1;
            Column(vec![text("a")])
        }
        """)

    def test_write_is_complete_and_keeps_mode(self):
        os.chmod(self.path, 0o640)
        code, _ = self.main("--write", self.dir.name)
        self.assertEqual(code, 0)
        self.assertEqual(os.stat(self.path).st_mode & 0o777, 0o640)
        with open(self.path, encoding="utf-8") as fh:
            self.assertEqual(fh.read(), cm.rewrite(self.FIXTURE, APIS).text)
        leftovers = [f for f in os.listdir(os.path.dirname(self.path)) if f.endswith(".tmp")]
        self.assertEqual(leftovers, [])

    def test_directory_walk_skips_symlinks(self):
        os.symlink(self.path, os.path.join(self.dir.name, "link.rs"))
        code, out = self.main("--check", self.dir.name)
        self.assertNotIn("link.rs", out)
        self.assertEqual(out.count("T2"), 1)
        code, _ = self.main("--write", self.dir.name)
        self.assertTrue(os.path.islink(os.path.join(self.dir.name, "link.rs")))

    def test_explicit_symlink_refused_and_never_written(self):
        link = os.path.join(self.dir.name, "link.rs")
        os.symlink(self.path, link)
        code, _ = self.main("--write", link)
        self.assertEqual(code, 2)
        self.assertTrue(os.path.islink(link))
        with open(self.path, encoding="utf-8") as fh:
            self.assertEqual(fh.read(), self.FIXTURE)

    def test_exclude_matches_whole_components(self):
        sibling = os.path.join(self.dir.name, "src2")
        os.makedirs(sibling)
        with open(os.path.join(sibling, "lib.rs"), "w", encoding="utf-8") as fh:
            fh.write(self.FIXTURE)
        code, out = self.main("--check", self.dir.name, "--exclude", os.path.join(self.dir.name, "src"))
        self.assertEqual(code, 1)
        self.assertIn("src2", out)
        self.assertNotIn(os.path.join("src", "lib.rs"), out)

    def write_shadowed(self):
        with open(self.path, "w", encoding="utf-8") as fh:
            fh.write(self.SHADOWED)

    def test_shadow_count_always_reported_and_strict_exits_2(self):
        self.write_shadowed()
        code, out = self.main("--check", self.dir.name)
        self.assertEqual(code, 0, out)
        self.assertIn("1 shadow skip(s)", out)
        self.assertTrue(out.rstrip().endswith("no candidates"))
        code, out = self.main("--check", "--strict", self.dir.name)
        self.assertEqual(code, 2, out)

    def test_strict_clean_tree_exits_0_and_candidates_win(self):
        code, out = self.main("--check", "--strict", self.dir.name)
        self.assertEqual(code, 1, out)          # candidate beats strict
        self.main("--write", self.dir.name)
        code, out = self.main("--check", "--strict", self.dir.name)
        self.assertEqual(code, 0, out)
        self.assertIn("0 shadow skip(s)", out)
        self.write_shadowed()
        with open(os.path.join(self.dir.name, "src", "b.rs"), "w", encoding="utf-8") as fh:
            fh.write(self.FIXTURE)
        code, _ = self.main("--check", "--strict", self.dir.name)
        self.assertEqual(code, 1)


class CommentGapTest(unittest.TestCase):
    def assert_skipped(self, src):
        text = GLOB + textwrap.dedent(src)
        res = cm.rewrite(text, APIS)
        self.assertEqual(res.text, text)
        self.assertTrue(any("comment inside the call head" in n for _, n in res.notes), res.notes)

    def test_t2_comment_in_head_skipped(self):
        self.assert_skipped("fn f() -> V { Column /*c*/ (vec![text(1)]) }\n")

    def test_t1_comment_in_erasure_gap_skipped(self):
        self.assert_skipped("fn f() -> V { column().child(any /*c*/ (x)) }\n")

    def test_comment_inside_argument_survives(self):
        out = run_g("fn f() -> V { column().child(any(x /*c*/)) }\n")
        self.assertIn("/*c*/", out)
        self.assertNotIn("any(", out)

    def test_t4_comment_in_gap_skipped(self):
        self.assert_skipped("fn f() -> V { Route::new(|| any /*c*/ (x)) }\n")


class CoverageGapTest(unittest.TestCase):
    def test_closure_param_annotated_anyview_is_known_blind_spot(self):
        # Documented blind spot: the tool rewrites; the compiler rejects the result.
        out = run_g("fn f() { let g = |v: AnyView<S>| column().child(any(v)); }\n")
        self.assertIn(".child(v)", out)

    def test_multiline_flexview_new(self):
        out = run_g("""\
            fn f() -> V {
                FlexView::new(
                    Axis::Vertical,
                    vec![
                        inflexible(any(a)),
                        flexible(1, any(b)),
                    ],
                )
            }
            """)
        self.assertIn("column()", out)
        self.assertIn(".child(a)", out)
        self.assertIn(".flex(1, b)", out)
        self.assertNotIn("FlexView", out)

    def test_entry_before_any_section_rejected(self):
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as fh:
            fh.write("foo\n[slot]\nbar\n")
        try:
            with self.assertRaises(ValueError):
                cm.load_apis(fh.name)
        finally:
            os.unlink(fh.name)

    def test_list_section_is_informational(self):
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as fh:
            fh.write("[list]\nlist_api\n[slot]\nslot_api\n")
        try:
            apis = cm.load_apis(fh.name)
        finally:
            os.unlink(fh.name)
        self.assertEqual(apis.lists, {"list_api"})
        self.assertEqual(apis.slot, {"slot_api"})
        self.assertNotIn("list_api", apis.slot | apis.builder)
        res = cm.rewrite("fn f() { list_api(any(x)); }\n", apis)
        self.assertEqual(res.candidates, [])


if __name__ == "__main__":
    unittest.main()
