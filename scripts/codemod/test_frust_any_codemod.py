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


def run5(src: str) -> str:
    """Rewrite `src` with the opt-in T5 rule on."""
    return cm.rewrite(textwrap.dedent(src), APIS, t5=True).text


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

    # -- callee origin: the name list matches names, the callee must be frust's --

    def assert_noted(self, src, why):
        src = textwrap.dedent(src)
        self.assertEqual(run(src), src)
        rules, notes = check(src)
        self.assertEqual(rules, [], src)
        self.assertTrue(any(why in n for _, n in notes), notes)

    def test_app_fn_named_like_slot_api_noted(self):
        # muxr rail.rs: the app's own `panel` takes an erased body (E0308).
        self.assert_noted("""\
            fn panel<S>(width: f32, body: AnyView<S>) -> AnyView<S> { body }
            fn f() -> V { panel(1.0, any(text("a"))) }
            """, "a same-file fn `panel` may be the callee")

    def test_app_import_named_like_slot_api_noted(self):
        self.assert_noted("""\
            use crate::ui::panel::panel;
            fn f() -> V { panel(1.0, any(text("a"))) }
            """, "`panel` is imported from a non-frust path")
        self.assert_noted("""\
            use super::field;
            fn f() -> V { field(any(x)) }
            """, "`field` is imported from a non-frust path")

    def test_app_glob_import_noted(self):
        self.assert_noted("use crate::ui::*;\nfn f() -> V { panel(1.0, any(x)) }\n",
                          "`panel` may come from a non-frust glob import")
        self.assert_noted("use frust::*;\nuse crate::ui::*;\nfn f() -> V { card(any(x)) }\n",
                          "non-frust glob import")

    def test_type_name_callee_glob_origin(self):
        self.assert_noted("use crate::ui::*;\nfn f() -> V { ScrollView::new(any(x)) }\n",
                          "`ScrollView` may come from a non-frust glob import")
        self.assert_noted("use frust::*;\nuse crate::ui::*;\nfn f() -> V { ScrollView::new(any(x)) }\n",
                          "`ScrollView` may come from a non-frust glob import")
        for pre in ("use frust::*;\n", ""):
            src = pre + "fn f() -> V { ScrollView::new(any(x)) }\n"
            self.assertIn("ScrollView::new(x)", run(src), src)

    def test_qualified_app_path_untouched(self):
        for src in ("crate::ui::panel(any(x))", "super::card(any(x))", "self::ui::card(any(x))",
                    "ui::card(any(x))", "crate::nav::Route::new(\"/\", || any(home()))"):
            self.assertEqual(run(src), src, src)
            self.assertEqual(check(src), ([], []), src)

    def test_local_type_named_like_assoc_api_noted(self):
        self.assert_noted("""\
            struct Route;
            impl Route { fn new<F>(p: &str, f: F) -> Self { Route } }
            fn f() { Route::new("/", || any(home())); }
            """, "`Route` is a local type")
        self.assert_noted("struct ScrollView;\nfn f() { ScrollView::new(any(body)); }\n",
                          "`ScrollView` is a local type")
        self.assert_noted("use crate::nav::Route;\nfn f() { Route::new(\"/\", || any(home())); }\n",
                          "`Route` is imported from a non-frust path")

    def test_method_named_like_same_file_fn_noted(self):
        self.assert_noted("""\
            impl Bar { fn leading(self, v: AnyView<()>) -> Self { self } }
            fn f(b: Bar) -> Bar { b.leading(any(x)) }
            """, "a same-file fn `leading` may be the callee of `.leading`")

    def test_kept_app_callee_is_not_noted(self):
        src = "fn panel(b: AnyView<()>) {}\nfn f() { panel(any(x)); // erasure: keep app fn\n}\n"
        self.assertEqual(run(src), src)
        self.assertEqual(check(src), ([], []))

    def test_frust_origin_callee_rewrites(self):
        self.assertEqual(run("use frust_material::card;\nfn f() -> V { card(any(x)) }\n"),
                         "use frust_material::card;\nfn f() -> V { card(x) }\n")
        self.assertEqual(run("use frust::panel;\nfn f() -> V { panel(1.0, any(x)) }\n"),
                         "use frust::panel;\nfn f() -> V { panel(1.0, x) }\n")
        self.assertEqual(run("frust_shadcn::card(any(x))"), "frust_shadcn::card(x)")
        self.assertEqual(run("frust::Route::new(\"/\", || any(home()))"),
                         "frust::Route::new(\"/\", || home())")
        self.assertEqual(run("use frust::Route;\nRoute::new(\"/\", || any(home()))"),
                         "use frust::Route;\nRoute::new(\"/\", || home())")

    def test_frust_import_beats_same_named_method(self):
        # A method `card` does not shadow the imported free fn `card`.
        src = "use frust::card;\nimpl P { fn card(&self) {} }\nfn f() -> V { card(any(x)) }\n"
        self.assertEqual(run(src), src.replace("card(any(x))", "card(x)"))

    def test_test_module_super_glob_is_transparent(self):
        src = textwrap.dedent("""\
            use frust::*;
            #[cfg(test)]
            mod tests {
                use super::*;
                fn t() -> V { card(any(x)) }
            }
            """)
        self.assertIn("card(x)", run(src))


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

    # -- builder origin: only a frust-origin import binds column/row/stack --

    def assert_noted(self, src, why):
        src = textwrap.dedent(src)
        self.assertEqual(run(src), src)
        rules, notes = check(src)
        self.assertEqual(rules, [], src)
        self.assertTrue(any(why in n for _, n in notes), notes)

    def test_app_imported_builders_noted(self):
        # muxr: `use super::{column, row};` helpers took the bare `column()` (E0061).
        self.assert_noted("""\
            use frust::{Column, Row};
            use super::{column, row};
            fn f() -> V { Column(vec![a, Row(vec![b])]) }
            """, "T2 skipped: `column` is imported from a non-frust path")
        self.assert_noted("""\
            use frust::Row;
            use crate::ui::row as row;
            fn f() -> V { Row(vec![a]) }
            """, "T2 skipped: `row` is imported from a non-frust path")

    def test_app_imported_sugar_noted(self):
        self.assert_noted("use crate::ui::Column;\nfn f() -> V { Column(vec![a]) }\n",
                          "T2 skipped: `Column` is imported from a non-frust path")
        self.assert_noted("use self::ui::FlexView;\nfn f() -> V { FlexView::new(Axis::Vertical, vec![a]) }\n",
                          "T3 skipped: `FlexView` is imported from a non-frust path")
        for src in ("crate::ui::Column(vec![a])", "super::Row(vec![a])"):
            self.assertEqual(run(src), src, src)

    def test_mixed_globs_noted(self):
        self.assert_noted("use frust::*;\nuse crate::ui::*;\nfn f() -> V { Column(vec![a]) }\n",
                          "T2 skipped: `Column`/`column` may come from a non-frust glob import")

    def test_frust_imported_builder_still_rewrites(self):
        src = "use frust_widgets::{column, Column};\nfn f() -> V { Column(vec![a]) }\n"
        self.assertIn("column().child(a)", run(src))


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


class T5Test(unittest.TestCase):
    def test_homogeneous_list_dropped(self):
        self.assertEqual(run5("s.actions(vec![any(button(a)), any(button(b))])"),
                         "s.actions(vec![button(a), button(b)])")
        self.assertEqual(run5("carousel(vec![any(slide(1)), frust::any(slide(2))])"),
                         "carousel(vec![slide(1), slide(2)])")

    def test_heterogeneous_heads_untouched(self):
        for src in (
            "s.actions(vec![any(button(a)), any(text(b))])",
            "s.actions(vec![any(text(a)), any(frust::text(b))])",   # different callee path
        ):
            self.assertEqual(run5(src), src, src)

    def test_same_head_needs_same_trailing_chain(self):
        same = "s.children(vec![any(text(a).size(1)), any(text(b).size(2))])"
        self.assertEqual(run5(same), "s.children(vec![text(a).size(1), text(b).size(2)])")
        for src in (
            "s.children(vec![any(text(a)), any(text(b).size(2))])",
            "s.children(vec![any(text(a).size(1)), any(text(b).bold())])",
        ):
            self.assertEqual(run5(src), src, src)

    def test_single_element_dropped(self):
        self.assertEqual(run5("sidebar_menu(vec![any(sidebar_menu_item(x))])"),
                         "sidebar_menu(vec![sidebar_menu_item(x)])")

    def test_nested_calls_in_elements(self):
        src = "s.children(vec![any(card(f(a, g(1)), h())), any(card(b, vec![c]))])"
        self.assertEqual(run5(src), "s.children(vec![card(f(a, g(1)), h()), card(b, vec![c])])")

    def test_nested_lists_reach_fixpoint(self):
        src = "s.children(vec![any(column().children(vec![any(t(1)), any(t(2))]))])"
        self.assertEqual(run5(src), "s.children(vec![column().children(vec![t(1), t(2)])])")

    def test_trailing_comma_and_comments_preserved(self):
        src = """\
        s.children(vec![
            // first
            any(item(1)), /* between */
            any(item(
                2,
            )), // last
        ])
        """
        want = """\
        s.children(vec![
            // first
            item(1), /* between */
            item(
                2,
            ), // last
        ])
        """
        self.assertEqual(run5(src), textwrap.dedent(want))

    def test_left_alone(self):
        for src in (
            "s.children(vec![])",                            # empty list
            "s.children(vec![any(x), any(y)])",              # no head: plain variables
            "s.children(vec![any(t(1)), t(2)])",             # a non-erasure element
            "s.children(vec![any(t(1)); 3])",                # repeat form
            "s.children(vec![any::<S, _>(t(1)), any::<S, _>(t(2))])",  # turbofish erasure
            "s.children(vec![any(t!(1)), any(t!(2))])",      # macro element
            "s.children(vec![any(t(1)?), any(t(2)?)])",      # not a pure call chain
            "s.children(items)",                             # not a literal
            "s.unknown(vec![any(t(1)), any(t(2))])",         # not a list callee
            "s.children(Some(vec![any(t(1))]))",             # not a direct argument
        ):
            self.assertEqual(run5(src), src, src)

    def test_local_vec_not_a_list_argument_untouched(self):
        src = "fn f() { let v = vec![any(t(1)), any(t(2))]; s.children(v); }"
        self.assertEqual(run5(src), src)

    def test_authoring_and_sugar_forms(self):
        self.assertEqual(run5("s.children(vec![authoring::any(t(1)), AnyView::new(t(2))])"),
                         "s.children(vec![t(1), t(2)])")
        # An unresolved `Row` (no frust import) is not frust's sugar: left alone.
        src = "Row(vec![any(t(1)), any(t(2))])"
        self.assertEqual(run5(src), src)

    def test_t2_site_not_listed_twice(self):
        rules, _ = check_g("fn f() { Column(vec![any(t(1)), any(t(2))]) }")
        self.assertEqual(rules, ["T2"])
        res = cm.rewrite(GLOB + "fn f() { Column(vec![any(t(1)), any(t(2))]) }", APIS, t5=True)
        self.assertEqual([c.rule for c in res.candidates], ["T2"])

    def test_off_by_default(self):
        src = "s.actions(vec![any(button(a)), any(button(b))])"
        self.assertEqual(run(src), src)
        self.assertEqual(check(src), ([], []))

    def test_idempotent(self):
        src = textwrap.dedent("""\
            use frust::{any, text};
            fn f() -> V {
                s.children(vec![any(text("a")), any(text("b"))])
            }
            """)
        once = run5(src)
        self.assertEqual(run5(once), once)
        self.assertNotIn("any", once)   # the now-unused import is dropped too

    def test_comment_in_erasure_head_skipped_with_note(self):
        res = cm.rewrite("s.children(vec![any /*c*/ (t(1))])", APIS, t5=True)
        self.assertEqual(res.text, "s.children(vec![any /*c*/ (t(1))])")
        self.assertTrue(any("T5 skipped" in n for _, n in res.notes), res.notes)

    def test_excluded_list_api_untouched(self):
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as fh:
            fh.write("[list]\nlist_api\n[exclude]\nfrust_other::list_api\n")
        try:
            apis = cm.load_apis(fh.name)
        finally:
            os.unlink(fh.name)
        # Both qualifiers are frust-origin, so only the exclusion tells them apart.
        src = "frust_other::list_api(vec![any(t(1))]); frust_mine::list_api(vec![any(t(1))]);"
        self.assertEqual(cm.rewrite(src, apis, t5=True).text,
                         "frust_other::list_api(vec![any(t(1))]); frust_mine::list_api(vec![t(1)]);")


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


class T5DriverTest(DriverFixture, unittest.TestCase):
    FIXTURE = textwrap.dedent("""\
        use frust::{any, text};

        fn f() -> V {
            s.children(vec![
                any(text("a")),
                any(text("b")),
            ])
        }
        """)

    def test_check_lists_t5_only_when_opted_in(self):
        code, out = self.main("--check", self.dir.name)
        self.assertEqual(code, 0, out)
        self.assertIn("no candidates", out)
        code, out = self.main("--check", "--t5", self.dir.name)
        self.assertEqual(code, 1, out)
        self.assertIn("lib.rs:4: T5 homogeneous list argument of .children", out)

    def test_write_requires_flag_and_is_idempotent(self):
        self.main("--write", self.dir.name)
        with open(self.path, encoding="utf-8") as fh:
            self.assertEqual(fh.read(), self.FIXTURE)   # T5 off: untouched
        code, _ = self.main("--write", "--t5", self.dir.name)
        self.assertEqual(code, 0)
        with open(self.path, encoding="utf-8") as fh:
            once = fh.read()
        self.assertIn('text("a"),\n', once)
        self.assertNotIn("any", once)
        code, out = self.main("--check", "--t5", "--strict", self.dir.name)
        self.assertEqual(code, 0, out)
        self.main("--write", "--t5", self.dir.name)
        with open(self.path, encoding="utf-8") as fh:
            self.assertEqual(fh.read(), once)

    def test_stats_reports_t5_separately(self):
        code, out = self.main("--stats", self.path)
        self.assertEqual(code, 0)
        self.assertNotIn("T5", out)
        code, out = self.main("--stats", "--t5", self.path)
        self.assertEqual(code, 0)
        self.assertIn("lib.rs: T5 candidates=1", out)
        self.assertIn("total: T5 candidates=1", out)
        self.assertIn("erasure calls before=2 after=0", out)
        with open(self.path, encoding="utf-8") as fh:
            self.assertEqual(fh.read(), self.FIXTURE)   # --stats alone never writes


MIXED = "column().children(vec![any(component(Header { a: 1 })), any(component(Counter { b: 2 }))])"


def t5_rules(src: str):
    res = cm.rewrite(textwrap.dedent(src), APIS, t5=True)
    return [c.rule for c in res.candidates], res


class KeepMarkerTest(unittest.TestCase):
    def test_unmarked_mixed_type_list_is_flagged(self):
        rules, _ = t5_rules(MIXED)
        self.assertEqual(rules, ["T5"])

    def test_marker_same_line_suppresses_t5(self):
        src = MIXED + " // erasure: keep"
        rules, res = t5_rules(src)
        self.assertEqual(rules, [])
        self.assertEqual(res.kept, 1)
        self.assertEqual(res.text, src)

    def test_marker_line_above_suppresses_t5(self):
        src = "fn f() {\n    // erasure: keep\n    " + MIXED + ";\n}\n"
        rules, res = t5_rules(src)
        self.assertEqual(rules, [])
        self.assertEqual(res.text, src)

    def test_marker_with_trailing_reason(self):
        for marker in ("// erasure: keep component<C> differs per call",
                       "//erasure: keep\tmixed types", "//   erasure: keep"):
            rules, _ = t5_rules(f"{marker}\n{MIXED}")
            self.assertEqual(rules, [], marker)

    def test_marker_must_be_exact(self):
        for marker in ("// erasure: keeping", "// erasure:keep", "/// erasure: keep",
                       "// not erasure: keep", "/* erasure: keep */"):
            rules, _ = t5_rules(f"{marker}\n{MIXED}")
            self.assertEqual(rules, ["T5"], marker)

    def test_marker_on_unrelated_line_does_not_suppress(self):
        src = "// erasure: keep\nlet a = 1;\n" + MIXED
        self.assertEqual(t5_rules(src)[0], ["T5"])
        src = "let a = 1;\n" + MIXED + "\n// erasure: keep\n"
        self.assertEqual(t5_rules(src)[0], ["T5"])
        # A trailing marker on the previous code line belongs to that line.
        src = "let a = 1; // erasure: keep\n" + MIXED
        self.assertEqual(t5_rules(src)[0], ["T5"])

    def test_marker_suppresses_t1_slot_site(self):
        src = "b.child(any(text(1))); // erasure: keep\nb.child(any(text(2)));"
        res = cm.rewrite(src, APIS)
        self.assertEqual(len(res.candidates), 1)
        self.assertEqual(res.kept, 1)
        self.assertEqual(res.text, "b.child(any(text(1))); // erasure: keep\nb.child(text(2));")

    def test_marker_suppresses_t2_and_its_notes(self):
        src = "// erasure: keep\nColumn(vec![any(a()), b()])"
        res = cm.rewrite(GLOB + src, APIS)
        self.assertEqual(res.candidates, [])
        self.assertEqual(res.kept, 1)
        self.assertEqual(res.text, GLOB + src)

    def test_kept_file_is_idempotent(self):
        src = "// erasure: keep why\n" + MIXED + "\nx.child(any(text(1)));\n"
        once = cm.rewrite(src, APIS, t5=True).text
        self.assertEqual(once, "// erasure: keep why\n" + MIXED + "\nx.child(text(1));\n")
        self.assertEqual(cm.rewrite(once, APIS, t5=True).text, once)


class KeepDriverTest(DriverFixture, unittest.TestCase):
    FIXTURE = textwrap.dedent("""\
        use frust::{any, component};

        fn f() -> V {
            column().children(vec![ // erasure: keep component<C> differs
                any(component(A {})),
                any(component(B {})),
            ])
        }
        """)
    UNMARKED = FIXTURE.replace(" // erasure: keep component<C> differs", "")

    def test_check_strict_t5_exits_0_when_kept_and_1_without(self):
        code, out = self.main("--check", "--strict", "--t5", self.dir.name)
        self.assertEqual(code, 0, out)
        self.assertIn("no candidates", out)
        with open(self.path, "w", encoding="utf-8") as fh:
            fh.write(self.UNMARKED)
        code, out = self.main("--check", "--strict", "--t5", self.dir.name)
        self.assertEqual(code, 1, out)
        self.assertIn("T5 homogeneous list argument of .children", out)

    def test_stats_reports_kept(self):
        code, out = self.main("--stats", "--t5", self.path)
        self.assertEqual(code, 0)
        self.assertIn("lib.rs: kept=1", out)
        self.assertIn("total: kept=1", out)

    def test_write_leaves_kept_file_untouched(self):
        self.main("--write", "--t5", self.dir.name)
        with open(self.path, encoding="utf-8") as fh:
            self.assertEqual(fh.read(), self.FIXTURE)


class T5ResolutionTest(unittest.TestCase):
    LIST = "(vec![any(t(1)), any(t(2))])"

    def test_foreign_and_local_row_not_flagged(self):
        for src in (
            "ui::Row" + self.LIST,
            "use ui::Row;\nRow" + self.LIST,
            "use frust::Row as Row;\nuse other::Column as Row;\nRow" + self.LIST,
            "struct Row;\nRow" + self.LIST,
            "Row" + self.LIST,
        ):
            self.assertEqual(t5_rules(src)[0], [], src)

    def test_frust_row_still_flagged(self):
        # Resolved frust sugar is still a candidate (T2 where it can build the chain).
        for src in (
            "frust::Row" + self.LIST,
            "use frust::Row;\nRow" + self.LIST,
            "use frust::*;\nRow" + self.LIST,
        ):
            self.assertNotEqual(t5_rules(src)[0], [], src)

    def test_children_method_unchanged(self):
        self.assertEqual(t5_rules("s.children" + self.LIST)[0], ["T5"])


class T5NegativeTest(unittest.TestCase):
    def test_not_a_head_is_left_alone(self):
        for src in (
            "s.children(vec![any(t.name::<T>(1)), any(t.name::<T>(2))])",
            "s.children(vec![any(move || t(1)), any(move || t(2))])",
            "s.children(vec![any(t(1).await), any(t(2).await)])",
        ):
            self.assertEqual(t5_rules(src)[0], [], src)

    def test_leading_path_separator_is_a_head(self):
        # `::path(..)` has a head: a same-head list is flagged, a mixed one is not.
        self.assertEqual(t5_rules("s.children(vec![any(::p::a(1)), any(::p::a(2))])")[0], ["T5"])
        self.assertEqual(t5_rules("s.children(vec![any(::p::a(1)), any(p::a(2))])")[0], [])

    def test_exclude_on_a_method_key_is_ignored(self):
        # `_excluded` is skipped for keys starting with `.`, so an [exclude]
        # entry cannot opt a `.children` site out; use `// erasure: keep`.
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as fh:
            fh.write("[list]\n.children\n[exclude]\nother::children\n.children\n")
        try:
            apis = cm.load_apis(fh.name)
        finally:
            os.unlink(fh.name)
        res = cm.rewrite("x.children(vec![any(t(1)), any(t(2))])", apis, t5=True)
        self.assertEqual(res.text, "x.children(vec![t(1), t(2)])")


class CommentBetweenRangesTest(unittest.TestCase):
    def test_t2_comment_between_elements_survives_rewrite(self):
        src = textwrap.dedent("""\
            Column(vec![
                any(a()),
                // between the two
                any(b()),
            ])""")
        out = run_g(src)
        self.assertIn("// between the two", out)
        self.assertIn(".child(a())", out)
        self.assertIn(".child(b())", out)
        self.assertNotIn("any", out)

    def test_t3_comment_between_children_survives_rewrite(self):
        src = "FlexView::new(Axis::Vertical, vec![\n    inflexible(any(a())),\n    /* gap */\n    flexible(1, b()),\n])"
        out = run_g(src)
        self.assertIn("/* gap */", out)
        self.assertIn(".child(a())", out)
        self.assertIn(".flex(1, b())", out)


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


def run7(src: str, **ctx) -> "cm.Result":
    """Rewrite `src` with the opt-in T7 rule on and the given T7Ctx fields."""
    return cm.rewrite(textwrap.dedent(src), APIS, t7=True, ctx=cm.T7Ctx(**ctx))


def buckets(res) -> list:
    return [(bucket, name) for _, bucket, name, _ in res.excluded]


class T7Test(unittest.TestCase):
    def test_simple_fn_rewritten(self):
        res = run7("""\
            use frust::{AnyView, View, any, text};

            fn title(s: &State) -> AnyView<State> {
                any(text(s.name.clone()))
            }
            """)
        self.assertEqual([c.rule for c in res.candidates], ["T7"])
        self.assertEqual(res.candidates[0].desc, "helper returns impl View: title")
        self.assertEqual(res.text, textwrap.dedent("""\
            use frust::{View, text};

            fn title(s: &State) -> impl View<State> + use<> {
                text(s.name.clone())
            }
            """))

    def test_let_prefixed_body_rewritten(self):
        res = run7("""\
            use frust::{AnyView, View, any, column, text};
            fn body<S: 'static>(n: u32) -> AnyView<S>
            where
                S: Clone,
            {
                let label = format!("{n}");
                let _unused = 1;
                any(column().child(text(label)))
            }
            """)
        self.assertIn("fn body<S: 'static>(n: u32) -> impl View<S>\nwhere", res.text)
        self.assertIn("    column().child(text(label))\n}", res.text)
        self.assertNotIn("any(", res.text)

    def test_return_tail_rewritten(self):
        res = run7("""\
            use frust::{AnyView, View, any, text};
            fn r() -> AnyView<S> {
                let t = text("a");
                return any(t);
            }
            """)
        self.assertIn("fn r() -> impl View<S> {\n    let t = text(\"a\");\n    return t;\n}", res.text)

    def test_path_qualified_anyview(self):
        res = run7("""\
            fn a() -> frust::AnyView<S> { frust::any(x()) }
            fn b() -> frust::authoring::AnyView<S> { AnyView::new(y()) }
            fn c() -> authoring::AnyView<S> { authoring::any(z()) }
            fn d() -> other::AnyView<S> { any(w()) }
            """)
        self.assertEqual(res.text, textwrap.dedent("""\
            fn a() -> impl frust::View<S> { x() }
            fn b() -> impl frust::authoring::View<S> { y() }
            fn c() -> impl authoring::View<S> { z() }
            fn d() -> other::AnyView<S> { any(w()) }
            """))

    def test_view_import_added_when_missing(self):
        res = run7("""\
            use frust::{AnyView, any, text};
            fn a() -> AnyView<S> { any(text("a")) }
            fn b() -> AnyView<S> { any(text("b")) }
            fn keep(v: AnyView<S>) {}
            """)
        self.assertTrue(res.text.startswith("use frust::{AnyView, View, text};\n"), res.text)
        self.assertEqual(res.text.count("impl View<S>"), 2)
        res = run7("use frust::AnyView;\nuse frust::any;\nfn a() -> AnyView<S> { any(t()) }\n")
        self.assertEqual(res.text, "use frust::View;\nfn a() -> impl View<S> { t() }\n")

    def test_view_import_in_nested_test_module(self):
        res = run7("""\
            use frust::{AnyView, any};
            fn keep(v: AnyView<S>) {}
            #[cfg(test)]
            mod tests {
                use super::*;
                fn fixture() -> AnyView<S> { any(t()) }
            }
            """)
        self.assertIn("    use super::*;\n    use frust::View;\n", res.text)
        self.assertTrue(res.text.startswith("use frust::AnyView;\n"), res.text)

    def test_edition_2021_lists_captures(self):
        src = """\
            use frust::{AnyView, View, any};
            fn row<'a, S: 'static>(label: &'a str) -> AnyView<S> { any(t(label)) }
            fn plain<S: 'static>(n: u8) -> AnyView<S> { any(t(n)) }
            impl<S: 'static> Card<S> {
                fn view(&self) -> AnyView<S> { any(t(1)) }
            }
            """
        res = run7(src, edition="2021")
        self.assertIn("fn row<'a, S: 'static>(label: &'a str) -> impl View<S> + use<S> {", res.text)
        self.assertIn("fn plain<S: 'static>(n: u8) -> impl View<S> {", res.text)  # borrows nothing
        self.assertIn("fn view(&self) -> impl View<S> + use<S> {", res.text)
        res = run7("use frust::{AnyView, View, any};\n"
                   "fn f(a: &u8, b: impl Into<u8>) -> AnyView<S> { any(t()) }\n", edition="2018")
        self.assertEqual(res.candidates, [])
        self.assertIn("a `use<..>` capture list cannot name an argument-position `impl Trait` parameter",
                      res.notes[0][1])

    def test_edition_2024_borrowing_helpers_emit_type_only_use(self):
        res = run7("""\
            use frust::{AnyView, View, any};
            impl<S: 'static> Card<S> {
                fn view(&self) -> AnyView<S> { any(t(1)) }
            }
            fn free<S: 'static>(st: &State) -> AnyView<S> { any(t(st)) }
            fn row<'a, S: 'static>(label: &'a str) -> AnyView<S> { any(t(label)) }
            fn plain<S: 'static>(n: u8) -> AnyView<S> { any(t(n)) }
            """, edition="2024")
        self.assertIn("fn view(&self) -> impl View<S> + use<S> { t(1) }", res.text)
        self.assertIn("fn free<S: 'static>(st: &State) -> impl View<S> + use<S> { t(st) }", res.text)
        self.assertIn("-> impl View<S> + use<S> { t(label) }", res.text)
        self.assertIn("fn plain<S: 'static>(n: u8) -> impl View<S> { t(n) }", res.text)
        self.assertNotIn("'a, S>", res.text)
        res = run7("use frust::{AnyView, View, any};\n"
                   "fn f(a: &u8, b: impl Into<u8>) -> AnyView<S> { any(t()) }\n", edition="2024")
        self.assertEqual(res.candidates, [])
        self.assertIn("a `use<..>` capture list cannot name an argument-position `impl Trait` parameter",
                      res.notes[0][1])

    def test_let_and_return_collapsed(self):
        res = run7("""\
            use frust::{AnyView, View, any, column};
            fn body<S: 'static>() -> AnyView<S> {
                let v = column(a());
                any(v)
            }
            """)
        self.assertIn("fn body<S: 'static>() -> impl View<S> {\n    column(a())\n}", res.text)
        self.assertNotIn("let v", res.text)

    def test_let_and_return_skipped_before_2024(self):
        src = """\
            use frust::{AnyView, View, any, column};
            fn body<S: 'static>() -> AnyView<S> {
                let v = column(a());
                any(v)
            }
            fn plain<S: 'static>() -> AnyView<S> { any(column(a())) }
            """
        for ed in ("2015", "2018", "2021"):
            res = run7(src, edition=ed)
            self.assertIn("let v = column(a());", res.text, ed)
            self.assertIn("fn body<S: 'static>() -> AnyView<S>", res.text, ed)
            self.assertIn("fn plain<S: 'static>() -> impl View<S> {", res.text, ed)
            self.assertEqual(len(res.notes), 1, ed)
            self.assertIn("a pre-2024 crate keeps tail-expression temporaries alive past locals; "
                          "collapse `let x = ..; any(x)` by hand", res.notes[0][1])
        res = run7(src, edition="2024")
        self.assertIn("fn body<S: 'static>() -> impl View<S> {\n    column(a())\n}", res.text)
        self.assertEqual(res.notes, [])

    def test_t7_help_is_edition_independent(self):
        with open(cm.__file__, encoding="utf-8") as fh:
            ap_src = fh.read()
        i = ap_src.index('"--t7"')
        help_text = ap_src[i:ap_src.index('"--t7-visible"', i)]
        self.assertNotIn("pre-2024", help_text)
        self.assertIn("any edition", help_text)

    def test_let_and_return_leaves_other_shapes(self):
        for stmt, tail in (("let mut v = column(a());", "any(v)"),
                           ("let v: Foo = column(a());", "any(v)"),
                           ("let w = column(a());", "any(v)"),
                           ("#[allow(x)] let v = column(a());", "any(v)")):
            res = run7(f"""\
                use frust::{{AnyView, View, any, column}};
                fn body<S: 'static>() -> AnyView<S> {{
                    {stmt}
                    {tail}
                }}
                """)
            self.assertIn(stmt, res.text, stmt)

    def test_cross_file_method_uses_match_the_impl_type(self):
        src = """\
            use frust::{AnyView, View, any};
            impl Card { fn body(&self) -> AnyView<S> { any(t()) } }
            """
        use = lambda qual, ity=None: cm.T7Use("/o.rs", ("src", "o"), "::body", qual, (), ity)
        run = lambda u: run7(src, path="/a.rs", module=("src",), uses=(u,))
        self.assertEqual(buckets(run(use(("Card",)))), [("T7-ref", "body")])
        self.assertEqual(buckets(run(use(("Self",), "Card"))), [("T7-ref", "body")])
        self.assertEqual(buckets(run(use(("Other",)))), [])
        self.assertEqual(buckets(run(use(("Self",), "Other"))), [])

    def test_private_helper_only_reached_from_descendant_modules(self):
        src = "use frust::{AnyView, View, any};\nfn h() -> AnyView<S> { any(t()) }\n"
        sib = cm.T7Use("/o.rs", ("src", "o"), "h", ("crate",), (), None)
        kid = cm.T7Use("/k.rs", ("src", "a", "k"), "h", ("super",), (), None)
        run = lambda u: run7(src, path="/a.rs", module=("src", "a"), uses=(u,))
        self.assertEqual(buckets(run(sib)), [])        # `crate::h` names the root's h
        self.assertEqual(buckets(run(kid)), [("T7-ref", "h")])

    def test_recursion_excluded(self):
        res = run7("""\
            use frust::{AnyView, View, any};
            fn tree(n: u8) -> AnyView<S> {
                let kids = tree(n - 1);
                any(node(kids))
            }
            impl X {
                fn walk(&self) -> AnyView<S> { let _ = Self::walk(self); any(n()) }
                fn view(&self) -> AnyView<S> { any(view(self.x)) }
            }
            """)
        self.assertEqual(buckets(res), [("T7-ref", "tree"), ("T7-ref", "walk")])
        self.assertEqual([c.desc for c in res.candidates], ["helper returns impl View: view"])

    def test_fn_used_as_value_excluded(self):
        res = run7("""\
            use frust::{AnyView, View, any};
            fn a() -> AnyView<S> { any(x()) }
            fn b() -> AnyView<S> { any(x()) }
            impl X { fn c(&self) -> AnyView<S> { any(x()) } }
            fn d() -> AnyView<S> { any(x()) }
            fn e(a: u8) -> AnyView<S> { any(x()) }
            fn user() {
                let f: fn() -> AnyView<S> = a;
                let g = Box::new(views::b);
                items.map(Self::c);
                let r = Row { render: d };
                let e = 1;
                use_it(e);
            }
            """)
        self.assertEqual(buckets(res), [("T7-ref", n) for n in "abcd"])
        self.assertEqual([c.desc for c in res.candidates], ["helper returns impl View: e"])
        # A value use elsewhere in the package (from the driver's index) excludes too.
        res = run7("use frust::{AnyView, View, any};\nfn a() -> AnyView<S> { any(x()) }\n",
                   values=frozenset({"a"}))
        self.assertEqual(buckets(res), [("T7-ref", "a")])

    def test_helper_taken_by_reference_excluded(self):
        res = run7("""\
            use frust::{AnyView, View, any};
            fn free_h() -> AnyView<S> { any(x()) }
            fn plain() -> AnyView<S> { any(x()) }
            fn mutref() -> AnyView<S> { any(x()) }
            fn bigger() -> AnyView<S> { any(x()) }
            impl X {
                fn m(&self) -> AnyView<S> { any(x()) }
                fn assoc() -> AnyView<S> { any(x()) }
                fn user(&self, prev: &X) {
                    take(&free_h());
                    take(&self.m());
                    take(&Self::assoc());
                    take(&prev.m());
                    take(&mut mutref());
                    take((a, bigger()));
                    take(&vec![bigger()]);
                    take(&bigger().len());
                    let v = plain();
                }
            }
            """)
        self.assertEqual(buckets(res), [("T7-ref", "free_h"), ("T7-ref", "mutref"),
                                        ("T7-ref", "m"), ("T7-ref", "assoc")])
        self.assertEqual([c.desc for c in res.candidates],
                         ["helper returns impl View: plain", "helper returns impl View: bigger"])
        self.assertTrue(all(e[3] == "taken by reference" for e in res.excluded
                            if e[1] == "T7-ref"))

    def test_let_bound_result_borrowed_excluded(self):
        res = run7("""\
            use frust::{AnyView, View, any};
            impl T {
                fn content(&self) -> AnyView<S> { any(x()) }
                fn moved(&self) -> AnyView<S> { any(x()) }
                fn field(&self) -> AnyView<S> { any(x()) }
                fn mutb(&self) -> AnyView<S> { any(x()) }
                fn build(&self, slot: &mut Slot, ctx: &mut Ctx) {
                    let content = self.content();
                    slot.rebuild(None, Some(&content), ctx);
                }
                fn other(&self, kids: &mut Vec<AnyView<S>>) {
                    let moved = self.moved();
                    kids.push(moved);
                    let field = self.field();
                    take(&field.len());
                    let mut mutb = self.mutb();
                    take(&mut mutb);
                }
            }
            """)
        self.assertEqual(buckets(res), [("T7-ref", "content"), ("T7-ref", "mutb")])
        self.assertEqual([c.desc for c in res.candidates],
                         ["helper returns impl View: moved", "helper returns impl View: field"])
        self.assertTrue(all(e[3] == "taken by reference" for e in res.excluded
                            if e[1] == "T7-ref"))

    def test_beui_shape_taken_by_reference_not_rewritten(self):
        src = """\
            use frust::{AnyView, View, any, text};
            impl Block {
                fn primary_view(&self) -> AnyView<State> {
                    any(text(self.label.clone()))
                }
                fn build(&self, ctx: &mut Ctx) {
                    build_child(&self.primary_view(), ctx);
                }
                fn rebuild(&self, prev: &Self, ctx: &mut Ctx) {
                    rebuild_child(&prev.primary_view(), &self.primary_view(), ctx);
                }
                fn teardown(&self, ctx: &mut Ctx) {
                    teardown_child(&self.primary_view(), ctx);
                }
            }
            """
        res = run7(src)
        self.assertEqual(res.candidates, [])
        self.assertEqual(buckets(res), [("T7-ref", "primary_view")])
        self.assertEqual(res.text, textwrap.dedent(src))

    def test_bindings_and_other_items_are_not_value_uses(self):
        res = run7("""\
            use frust::{AnyView, View, any};
            fn label() -> AnyView<S> { any(x()) }
            fn row(&self) -> AnyView<S> { any(x()) }
            fn pod() -> AnyView<S> { any(x()) }
            fn user(v: Vec<(u8, u8)>) {
                for (i, label) in v { use_it(label); }
                v.iter().map(|(a, label)| label);
                match q { (_, Some(pod)) => use_it(pod), _ => {} }
                let row: u8 = 1;
                use_it(row);
                kinds.map(Kind::label);
                visit_children!(pod);
                label.len();
            }
            """)
        self.assertEqual(res.excluded, [])
        self.assertEqual(len(res.candidates), 3)

    def test_trait_fns_excluded(self):
        res = run7("""\
            use frust::{AnyView, View, any};
            trait Screen<S> {
                fn view(&self) -> AnyView<S>;
                fn chrome(&self) -> AnyView<S> { any(bar()) }
            }
            impl<S> Screen<S> for Home where F: for<'a> Fn(&'a u8) {
                fn view(&self) -> AnyView<S> { any(page()) }
            }
            """)
        self.assertEqual([(b, n, r) for _, b, n, r in res.excluded], [
            ("T7-trait", "view", "trait method declaration"),
            ("T7-trait", "chrome", "trait method declaration"),
            ("T7-trait", "view", "trait impl method"),
        ])
        self.assertEqual(res.candidates, [])
        self.assertNotIn("impl View", res.text)

    def test_multi_arm_bodies_listed_as_arms(self):
        res = run7("""\
            use frust::{AnyView, View, any};
            fn a(c: bool) -> AnyView<S> {
                let x = 1;
                if c { any(p()) } else if x > 0 { any(q()) } else { any(r()) }
            }
            fn b(k: u8) -> AnyView<S> { match k { 0 => any(p()), _ => any(q()) } }
            fn c(k: bool) -> AnyView<S> {
                if k {
                    return any(p());
                }
                any(q())
            }
            """)
        self.assertEqual([(b, n, r) for _, b, n, r in res.excluded], [
            ("T7-arms", "a", "if/else tail"), ("T7-arms", "b", "match tail"),
            ("T7-arms", "c", "early return")])
        self.assertEqual(res.candidates, [])

    def test_other_tails_ignored(self):
        res = run7("""\
            use frust::{AnyView, View, any};
            fn a() -> AnyView<S> { let v = vec![any(p())]; any(column().children(v)) }
            fn b() -> AnyView<S> { let mut acc = Vec::new(); for i in 0..3 { acc.push(i); } any(r(acc)) }
            fn c() -> AnyView<S> { loop { break any(p()); } }
            fn d() -> AnyView<S> { other(any(p())) }
            fn e() -> AnyView<S> { any::<S, _>(p()) }
            fn f() -> AnyView<S> { let v = any(p()); v }
            async fn g() -> AnyView<S> { any(p()) }
            """)
        # a and b end in one erasure call (accumulated statements are fine);
        # c-g have no single-erasure tail and are neither rewritten nor listed.
        self.assertEqual([c.desc.rsplit(" ", 1)[1] for c in res.candidates], ["a", "b"])
        self.assertEqual(res.excluded, [])

    def test_pub_library_fn_listed_public(self):
        src = """\
            use frust::{AnyView, View, any};
            pub fn api() -> AnyView<S> { any(p()) }
            pub(crate) fn internal() -> AnyView<S> { any(p()) }
            fn private() -> AnyView<S> { any(p()) }
            impl W { pub fn method(&self) -> AnyView<S> { any(p()) } }
            #[cfg(test)]
            mod tests {
                pub fn fixture() -> AnyView<S> { any(p()) }
            }
            """
        res = run7(src, library=True, visible=True)
        self.assertEqual(buckets(res), [("T7-public", "api"), ("T7-public", "method")])
        self.assertEqual([c.desc.rsplit(" ", 1)[1] for c in res.candidates],
                         ["internal", "private", "fixture"])
        res = run7(src, library=False, visible=True)   # an app, a tests/ or an examples/ source
        self.assertEqual(res.excluded, [])
        self.assertEqual(len(res.candidates), 5)

    def test_visible_helper_listed_unless_opted_in(self):
        src = """\
            use frust::{AnyView, View, any};
            pub(crate) fn a() -> AnyView<S> { any(p()) }
            pub(super) fn b() -> AnyView<S> { any(p()) }
            pub(in crate::x) fn c() -> AnyView<S> { any(p()) }
            pub fn d() -> AnyView<S> { any(p()) }
            fn e() -> AnyView<S> { any(p()) }
            """
        res = run7(src)
        self.assertEqual([n for _, n in buckets(res)], ["a", "b", "c", "d"])
        self.assertEqual({b for b, _ in buckets(res)}, {"T7-visible"})
        self.assertEqual([c.desc.rsplit(" ", 1)[1] for c in res.candidates], ["e"])
        self.assertIn("pub(crate) fn a() -> AnyView<S>", res.text)
        self.assertIn("fn e() -> impl View<S>", res.text)
        res = run7(src, visible=True)
        self.assertEqual(res.excluded, [])
        self.assertIn("pub(crate) fn a() -> impl View<S>", res.text)
        self.assertEqual(len(res.candidates), 5)
        res = run7("use frust::{AnyView, View, any};\n"
                   "pub(crate) fn a() -> AnyView<S> { any(p()) }\n", library=True)
        self.assertEqual(buckets(res), [("T7-visible", "a")])

    def test_closure_local_return_is_not_an_early_return(self):
        src = """\
            use frust::{AnyView, View, any};
            fn a() -> AnyView<S> {
                let f = move |_app| {
                    if x { return; }
                };
                let g = || { return 1; };
                any(button().on_click(f))
            }
            fn b() -> AnyView<S> {
                if x { return any(p()); }
                any(q())
            }
            fn c() -> AnyView<S> {
                let f = |_| { return; };
                if x { return any(p()); }
                any(q())
            }
            """
        res = run7(src)
        self.assertEqual([c.desc.rsplit(" ", 1)[1] for c in res.candidates], ["a"])
        self.assertEqual(buckets(res), [("T7-arms", "b"), ("T7-arms", "c")])

    def test_rustfmt_moved_marker_is_fn_scoped(self):
        src = """\
            use frust::{AnyView, View, any};
            fn a() -> AnyView<S> {
                // erasure: keep stored in a Vec<AnyView<S>> by callers
                any(p())
            }
            fn b() -> AnyView<S> {
                let x = 1;
                // erasure: keep too late
                any(p())
            }
            """
        res = run7(src)
        self.assertEqual(res.kept, 1)
        self.assertEqual([c.desc.rsplit(" ", 1)[1] for c in res.candidates], ["b"])
        self.assertIn("fn a() -> AnyView<S>", res.text)

    def test_keep_marker_honoured(self):
        src = """\
            use frust::{AnyView, View, any};
            // erasure: keep stored in a Vec<AnyView<S>> by callers
            fn a() -> AnyView<S> { any(p()) }
            fn b() -> AnyView<S> { // erasure: keep
                any(p())
            }
            fn c(
                x: u8,
            ) -> AnyView<S> { // erasure: keep
                any(p())
            }
            fn d() -> AnyView<S> { any(p()) }
            """
        res = run7(src)
        self.assertEqual(res.kept, 3)
        self.assertEqual([c.desc for c in res.candidates], ["helper returns impl View: d"])
        self.assertEqual(res.text.count("-> AnyView<S>"), 3)

    def test_off_by_default_and_idempotent(self):
        src = "use frust::{AnyView, View, any};\nfn a() -> AnyView<S> { any(p()) }\n"
        self.assertEqual(cm.rewrite(src, APIS).text, src)
        self.assertEqual(cm.rewrite(src, APIS, t5=True).text, src)
        once = run7(src).text
        self.assertEqual(once, "use frust::View;\nfn a() -> impl View<S> { p() }\n")
        again = run7(once)
        self.assertEqual(again.text, once)
        self.assertEqual(again.candidates, [])

    def test_inner_erasures_follow_in_later_passes(self):
        res = run7("""\
            use frust::{AnyView, View, any, column, text};
            fn a() -> AnyView<S> { any(column().child(any(text("x")))) }
            """)
        self.assertIn("fn a() -> impl View<S> { column().child(text(\"x\")) }", res.text)


class T7LibraryPathTest(unittest.TestCase):
    def test_library_classification(self):
        for path, lib in (("crates/frust-widgets/src/a.rs", True),
                          ("plugins/shadcn/src/card.rs", True),
                          ("crates/frust-widgets/tests/a.rs", False),
                          ("plugins/material/examples/demo/src/main.rs", False),
                          ("crates/frust-testing/src/corpus/scroll.rs", False),
                          ("examples/huddle/src/main.rs", False),
                          ("benchmarks/frust_bench/src/lib.rs", False)):
            self.assertEqual(cm._t7_library(path), lib, path)


class T7DriverTest(DriverFixture, unittest.TestCase):
    LIB = textwrap.dedent("""\
        use frust::{AnyView, View, any};

        pub fn api() -> AnyView<S> {
            any(p())
        }

        fn helper(s: &S) -> AnyView<S> {
            any(p())
        }
        """)
    TEST = textwrap.dedent("""\
        use frust::{AnyView, View, any};

        pub fn fixture() -> AnyView<S> {
            any(p())
        }
        """)

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        root = self.dir.name
        self.write("Cargo.toml", '[workspace]\nmembers = ["crates/foo"]\n\n'
                                 '[workspace.package]\nedition = "2021"\n')
        self.write("crates/foo/Cargo.toml", '[package]\nname = "foo"\nedition.workspace = true\n')
        self.lib = self.write("crates/foo/src/lib.rs", self.LIB)
        self.test = self.write("crates/foo/tests/t.rs", self.TEST)
        self.crates = os.path.join(root, "crates")

    def write(self, rel, text):
        path = os.path.join(self.dir.name, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text)
        return path

    def read(self, path):
        with open(path, encoding="utf-8") as fh:
            return fh.read()

    def test_check_exit_codes_with_and_without_t7(self):
        code, out = self.main("--check", "--strict", self.crates)
        self.assertEqual(code, 0, out)
        self.assertNotIn("T7", out)
        code, out = self.main("--check", "--t7", self.crates)
        self.assertEqual(code, 1, out)
        self.assertIn("lib.rs:7: T7 helper returns impl View: helper", out)
        self.assertIn("lib.rs:3: T7-public excluded: api (pub fn in a library source)", out)
        self.assertIn("t.rs:3: T7-visible excluded: fixture (visible outside its module", out)
        self.assertNotIn("T7 helper returns impl View: fixture", out)
        self.assertIn("2 T7 exclusion(s)", out)
        code, out = self.main("--check", "--t7", "--t7-visible", self.crates)
        self.assertEqual(code, 1, out)
        self.assertIn("t.rs:3: T7 helper returns impl View: fixture", out)
        self.assertIn("1 T7 exclusion(s)", out)
        code, _ = self.main("--write", "--t7", "--t7-visible", self.crates)
        self.assertEqual(code, 0)
        # Exclusions stay listed but never fail the check, even under --strict.
        code, out = self.main("--check", "--strict", "--t7", self.crates)
        self.assertEqual(code, 0, out)
        self.assertIn("T7-public excluded: api", out)
        self.assertIn("no candidates", out)

    def test_write_resolves_workspace_edition_and_is_idempotent(self):
        self.main("--write", self.crates)
        self.assertEqual(self.read(self.lib), self.LIB)            # T7 off: untouched
        self.main("--write", "--t7", self.crates)
        once = self.read(self.lib)
        self.assertIn("fn helper(s: &S) -> impl View<S> + use<> {\n    p()\n}", once)
        self.assertIn("pub fn api() -> AnyView<S> {", once)
        self.assertEqual(self.read(self.test), self.TEST)          # T7-visible: untouched
        self.main("--write", "--t7", "--t7-visible", self.crates)
        self.assertIn("pub fn fixture() -> impl View<S> {", self.read(self.test))
        self.assertIn("pub fn api() -> AnyView<S> {", self.read(self.lib))
        self.main("--write", "--t7", "--t7-visible", self.crates)
        self.assertEqual(self.read(self.lib), once)

    def test_edition_2024_and_resolution_rules(self):
        self.write("Cargo.toml", '[workspace]\n\n[workspace.package]\nedition = "2024"\n')
        self.main("--write", "--t7", self.crates)
        self.assertIn("fn helper(s: &S) -> impl View<S> + use<> {", self.read(self.lib))
        cm._TOML_CACHE.clear()
        self.assertEqual(cm.crate_edition(self.lib), "2024")
        self.write("crates/foo/Cargo.toml", '[package]\nname = "foo"\nedition = "2021"\n')
        cm._TOML_CACHE.clear()
        self.assertEqual(cm.crate_edition(self.lib), "2021")
        self.write("crates/foo/Cargo.toml", '[package]\nname = "foo"\n')
        cm._TOML_CACHE.clear()
        self.assertEqual(cm.crate_edition(self.lib), "2015")
        self.assertEqual(cm.crate_edition("/nonexistent-dir/x.rs", "2024"), "2024")

    def test_value_use_in_another_file_of_the_package(self):
        self.write("crates/foo/src/table.rs", "pub fn rows() { let f = crate::helper; f(); }\n")
        code, out = self.main("--check", "--t7", self.lib)
        self.assertIn("lib.rs:7: T7-ref excluded: helper (used as a value)", out)
        self.assertEqual(code, 0, out)
        # An --exclude'd file is never read, so its uses do not count.
        code, out = self.main("--check", "--t7", self.lib, "--exclude",
                              os.path.join(self.crates, "foo", "src", "table.rs"))
        self.assertEqual(code, 1, out)

    def test_unrelated_same_named_value_use_does_not_exclude(self):
        self.write("crates/foo/src/table.rs", "fn rows() { let f = helper; f(); }\n")
        code, out = self.main("--check", "--t7", self.lib)
        self.assertEqual(code, 1, out)
        self.assertIn("lib.rs:7: T7 helper returns impl View: helper", out)
        self.assertNotIn("used as a value", out)

    def test_child_module_super_path_value_use_excludes(self):
        self.write("crates/foo/src/table.rs", "fn rows() { let f = super::helper; f(); }\n")
        code, out = self.main("--check", "--t7", self.lib)
        self.assertEqual(code, 0, out)
        self.assertIn("lib.rs:7: T7-ref excluded: helper (used as a value)", out)

    def test_import_from_the_helper_module_excludes(self):
        self.write("crates/foo/src/table.rs",
                   "use crate::helper;\nfn rows() { let f = helper; f(); }\n")
        code, out = self.main("--check", "--t7", self.lib)
        self.assertEqual(code, 0, out)
        self.assertIn("lib.rs:7: T7-ref excluded: helper (used as a value)", out)
        self.write("crates/foo/src/table.rs",
                   "use crate::other::helper;\nfn rows() { let f = helper; f(); }\n")
        code, out = self.main("--check", "--t7", self.lib)
        self.assertEqual(code, 1, out)

    def test_stats_reports_t7_and_buckets_separately(self):
        code, out = self.main("--stats", self.crates)
        self.assertNotIn("T7", out)
        code, out = self.main("--stats", "--t7", self.crates)
        self.assertEqual(code, 0)
        self.assertIn("lib.rs: T7 candidates=1", out)
        self.assertIn("lib.rs: T7 excluded T7-trait=0 T7-arms=0 T7-ref=0 T7-public=1 T7-visible=0", out)
        self.assertIn("total: T7 candidates=1", out)
        self.assertIn("total: T7 excluded T7-trait=0 T7-arms=0 T7-ref=0 T7-public=1 T7-visible=1", out)
        code, out = self.main("--stats", "--t7", "--t7-visible", self.crates)
        self.assertIn("total: T7 candidates=2", out)
        self.assertIn("total: T7 excluded T7-trait=0 T7-arms=0 T7-ref=0 T7-public=1 T7-visible=0", out)
        self.assertEqual(self.read(self.lib), self.LIB)            # --stats never writes


def run6(src: str, t5: bool = False) -> "cm.Result":
    """Rewrite `src` with the opt-in T6 rule on."""
    return cm.rewrite(textwrap.dedent(src), APIS, t5=t5, t6=True)


def buckets6(res):
    return [(b, n) for _, b, n, _ in res.excluded6]


class T6Test(unittest.TestCase):
    def test_heterogeneous_elements_become_a_tuple(self):
        res = run6("""
            fn f() {
                sidebar_menu(vec![any(text("a")), any(icon(1)), AnyView::new(b)])
            }
            """)
        self.assertIn("sidebar_menu((text(\"a\"), icon(1), b))", res.text)
        self.assertEqual([c.rule for c in res.candidates], ["T6"])

    def test_method_seq_api_and_description(self):
        res = run6("fn f() { x.children(vec![any(a()), any(b())]) }\n")
        self.assertEqual(res.text, "fn f() { x.children((a(), b())) }\n")
        self.assertEqual(res.candidates[0].desc, "erased list argument of .children -> tuple")

    def test_single_element_gets_trailing_comma(self):
        self.assertEqual(run6("fn f() { x.children(vec![any(a())]) }\n").text,
                         "fn f() { x.children((a(),)) }\n")

    def test_single_element_with_trailing_comma_keeps_it_once(self):
        self.assertEqual(run6("fn f() { x.children(vec![any(a()),]) }\n").text,
                         "fn f() { x.children((a(),)) }\n")

    def test_trailing_comma_and_comments_preserved(self):
        res = run6("""
            fn f() {
                x.children(vec![
                    // first
                    any(a()), // one
                    /* two */ any(b()),
                ])
            }
            """)
        self.assertEqual(res.text, textwrap.dedent("""
            fn f() {
                x.children((
                    // first
                    a(), // one
                    /* two */ b(),
                ))
            }
            """))

    def test_keyed_element_listed_not_rewritten(self):
        src = "fn f() { x.children(vec![any(a()), keyed(2, b())]) }\n"
        res = run6(src)
        self.assertEqual(res.text, src)
        self.assertEqual(buckets6(res), [("T6-keyed", ".children")])
        self.assertEqual(res.candidates, [])

    def test_more_than_twelve_listed(self):
        items = ", ".join(f"any(a{i}())" for i in range(13))
        src = f"fn f() {{ x.children(vec![{items}]) }}\n"
        res = run6(src)
        self.assertEqual(res.text, src)
        self.assertEqual(buckets6(res), [("T6-arity", ".children")])
        twelve = ", ".join(f"any(a{i}())" for i in range(12))
        self.assertEqual([c.rule for c in run6(f"fn f() {{ x.children(vec![{twelve}]) }}\n").candidates], ["T6"])

    def test_partially_erased_listed(self):
        src = "fn f() { x.children(vec![any(a()), b()]) }\n"
        res = run6(src)
        self.assertEqual(res.text, src)
        self.assertEqual(buckets6(res), [("T6-mixed", ".children")])

    def test_list_without_erasure_is_not_listed(self):
        res = run6("fn f() { x.children(vec![a(), b()]) }\n")
        self.assertEqual(res.excluded6, [])
        self.assertEqual(res.candidates, [])

    def test_let_bound_list_untouched(self):
        src = "fn f() { let v = vec![any(a()), any(b())]; x.children(v) }\n"
        res = run6(src)
        self.assertEqual(res.text, src)
        self.assertEqual(res.excluded6, [])

    def test_turbofish_element_untouched_and_unlisted(self):
        src = "fn f() { x.children(vec![any::<S, _>(a()), any(b())]) }\n"
        res = run6(src)
        self.assertEqual(res.text, src)
        self.assertEqual(res.excluded6, [])

    def test_callee_not_in_seq_untouched(self):
        src = "fn f() { other(vec![any(a()), any(b())]) }\n"
        self.assertEqual(run6(src).text, src)

    def test_idempotent(self):
        once = run6("fn f() { x.children(vec![any(a()), any(b())]) }\n").text
        res = run6(once)
        self.assertEqual(res.text, once)
        self.assertEqual(res.candidates, [])

    def test_off_by_default(self):
        src = "fn f() { x.children(vec![any(a()), any(b())]) }\n"
        self.assertEqual(cm.rewrite(src, APIS).text, src)

    def test_keep_marker_honoured(self):
        src = "fn f() {\n    // erasure: keep\n    x.children(vec![any(a()), any(b())])\n}\n"
        res = run6(src)
        self.assertEqual(res.text, src)
        self.assertEqual(res.kept, 1)

    def test_t6_wins_over_t5_for_homogeneous_list(self):
        src = "fn f() { x.children(vec![any(text(a)), any(text(b))]) }\n"
        res = cm.rewrite(src, APIS, t5=True, t6=True)
        self.assertEqual(res.text, "fn f() { x.children((text(a), text(b))) }\n")
        self.assertEqual([c.rule for c in res.candidates], ["T6"])
        self.assertEqual(cm.rewrite(src, APIS, t5=True).text,
                         "fn f() { x.children(vec![text(a), text(b)]) }\n")

    def test_t5_still_applies_when_t6_lists_a_list(self):
        items = ", ".join(f"any(text({i}))" for i in range(13))
        res = cm.rewrite(f"fn f() {{ x.children(vec![{items}]) }}\n", APIS, t5=True, t6=True)
        self.assertEqual([c.rule for c in res.candidates], ["T5"])
        self.assertEqual(buckets6(res), [("T6-arity", ".children")])

    def test_t2_sugar_still_owns_column(self):
        res = run6("use frust::*;\nfn f() { Column(vec![any(a()), any(b())]) }\n")
        self.assertEqual([c.rule for c in res.candidates], ["T2"])

    def test_any_import_dropped_when_unused(self):
        res = run6("use frust::any;\nfn f() { x.children(vec![any(a()), any(b())]) }\n")
        self.assertNotIn("use frust::any", res.text)


class T6DriverTest(DriverFixture, unittest.TestCase):
    FIXTURE = "fn f() { x.children(vec![any(a()), any(b())]) }\nfn g() { x.children(vec![any(a()), c()]) }\n"

    def test_check_exit_codes(self):
        code, out = self.main("--check", "--t6", self.path)
        self.assertEqual(code, 1)
        self.assertIn("lib.rs:1: T6 erased list argument of .children -> tuple", out)
        self.assertIn("lib.rs:2: T6-mixed excluded: .children", out)
        self.assertIn("1 T6 exclusion(s)", out)
        self.assertEqual(self.main("--check", self.path)[0], 0)

    def test_exclusions_alone_exit_zero(self):
        with open(self.path, "w", encoding="utf-8") as fh:
            fh.write("fn g() { x.children(vec![any(a()), c()]) }\n")
        code, out = self.main("--check", "--t6", self.path)
        self.assertEqual(code, 0)
        self.assertIn("T6-mixed excluded", out)

    def test_missing_seq_section_exits_two(self):
        apis = os.path.join(self.dir.name, "apis.txt")
        with open(apis, "w", encoding="utf-8") as fh:
            fh.write("[slot]\n.child\n")
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            code, _ = self.main("--check", "--t6", "--apis", apis, self.path)
        self.assertEqual(code, 2)
        self.assertIn("[seq]", err.getvalue())
        self.assertEqual(self.main("--check", "--apis", apis, self.path)[0], 0)

    def test_write_then_check_clean(self):
        code, _ = self.main("--write", "--t6", self.path)
        self.assertEqual(code, 0)
        self.assertIn("x.children((a(), b()))", self.read(self.path))
        self.assertEqual(self.main("--check", "--t6", self.path)[0], 0)

    def test_stats_counts(self):
        code, out = self.main("--stats", "--t6", self.path)
        self.assertEqual(code, 0)
        self.assertIn("lib.rs: T6 candidates=1", out)
        self.assertIn("lib.rs: T6 excluded T6-keyed=0 T6-arity=0 T6-mixed=1", out)
        self.assertIn("total: T6 candidates=1", out)
        self.assertIn("total: T6 excluded T6-keyed=0 T6-arity=0 T6-mixed=1", out)

    def read(self, path):
        with open(path, encoding="utf-8") as fh:
            return fh.read()


def unmarked(src: str) -> list:
    return cm.unmarked_erasures(cm.Source(textwrap.dedent(src)))


class UnmarkedBodyMarkerTest(unittest.TestCase):
    def test_first_body_comment_covers_the_whole_fn(self):
        self.assertEqual(unmarked("""\
            fn f(k: u8) -> AnyView<S> {
                // erasure: keep arms differ
                match k {
                    0 => any(a()),
                    _ => any(b()),
                }
            }
            """), [])

    def test_later_or_reasonless_body_marker_is_not_fn_scoped(self):
        self.assertEqual(unmarked("""\
            fn f(k: u8) -> AnyView<S> {
                let x = 1;
                // erasure: keep late
                foo(any(a()));
                foo(any(b()));
            }
            """), [5])
        self.assertEqual(unmarked("""\
            fn f(k: u8) -> AnyView<S> {
                // erasure: keep
                foo(any(a()));
                foo(any(b()));
            }
            """), [3, 4])


class UnmarkedRuleTest(unittest.TestCase):
    def test_unmarked_call_listed(self):
        self.assertEqual(unmarked("""\
            fn f() {
                let v = any(text("a"));
            }
            """), [2])

    def test_same_line_marker(self):
        self.assertEqual(unmarked("""\
            fn f() {
                let v = any(a); // erasure: keep dyn
            }
            """), [])

    def test_marker_alone_above(self):
        self.assertEqual(unmarked("""\
            fn f() {
                // erasure: keep dyn
                foo(any(a));
                foo(any(b));
            }
            """), [4])

    def test_fn_signature_marker_covers_match_arms(self):
        src = """\
            // erasure: keep arms differ
            fn f(k: u8) -> AnyView<S> {
                match k {
                    0 => any(a()),
                    1 => any(b()),
                    _ => any(c()),
                }
            }
            """
        self.assertEqual(unmarked(src), [])
        bare = src.split("\n", 1)[1]
        self.assertEqual(unmarked(bare), [3, 4, 5])

    def test_fn_arrow_line_marker(self):
        self.assertEqual(unmarked("""\
            fn f(
                a: u8,
            ) -> AnyView<S> { // erasure: keep why
                any(a())
            }
            fn g() -> AnyView<S> {
                any(b())
            }
            """), [7])

    def test_vec_start_line_marker_covers_elements(self):
        src = """\
            fn f() {
                x.children(vec![ // erasure: keep mixed
                    any(a()),
                    any(b()),
                ]);
                x.children(vec![
                    any(c()),
                ]);
            }
            """
        self.assertEqual(unmarked(src), [7])

    def test_let_and_match_start_line_markers(self):
        self.assertEqual(unmarked("""\
            fn f(k: u8) {
                // erasure: keep why
                let v = if k == 0 {
                    any(a())
                } else {
                    any(b())
                };
                // erasure: keep why
                match k {
                    0 => any(c()),
                    _ => any(d()),
                }
                let w = any(e());
            }
            """), [13])

    def test_arrow_in_generics_marker_does_not_cover_body(self):
        self.assertEqual(unmarked("""\
            fn f<F: Fn() -> u32>(g: F) -> AnyView<S> { // erasure: keep generic arrow
                any(a())
            }
            """), [])
        self.assertEqual(unmarked("""\
            fn f<F: Fn() -> u32>(g: F) -> AnyView<S> {
                let n = g(); // erasure: keep n
                any(a())
            }
            """), [3])
        self.assertEqual(unmarked("""\
            fn f<F: Fn() -> u32>(
                g: F,
            ) -> AnyView<S> {
                any(a())
            }
            """), [4])

    def test_generic_arrow_line_marker_not_binding(self):
        self.assertEqual(unmarked("""\
            fn f<
                F: Fn() -> u32, // erasure: keep generic arrow
            >(g: F) -> AnyView<S> {
                any(a())
            }
            """), [4])

    def test_where_clause_arrow_marker_does_not_cover_body(self):
        self.assertEqual(unmarked("""\
            fn f<F, V>(g: F) -> AnyView<S>
            where
                F: Fn(u32) -> V, // erasure: keep where arrow
            {
                any(a())
            }
            """), [5])

    def test_real_return_arrow_covers_when_generics_contain_arrow(self):
        self.assertEqual(unmarked("""\
            fn f<F: Fn() -> u32>(
                g: F,
            ) -> AnyView<S> { // erasure: keep dyn
                any(a())
            }
            """), [])

    def test_bare_marker_covers_nothing(self):
        self.assertEqual(unmarked("""\
            fn f() {
                let a = any(x()); // erasure: keep
                // erasure: keep
                foo(any(b()));
            }
            """), [2, 4])
        self.assertEqual(unmarked("""\
            // erasure: keep
            fn f() {
                any(a())
            }
            fn g() -> AnyView<S> { // erasure: keep
                any(b())
            }
            """), [3, 6])
        self.assertEqual(unmarked("""\
            fn f(k: u8) {
                x.children(vec![ // erasure: keep
                    any(a()),
                ]);
                // erasure: keep
                let v = any(b());
                // erasure: keep
                match k {
                    _ => any(c()),
                }
            }
            """), [3, 6, 9])

    def test_whitespace_only_reason_is_bare(self):
        self.assertEqual(unmarked("fn f() { let a = any(x()); } // erasure: keep   \n"), [1])

    def test_marker_above_blank_or_attribute_line_does_not_cover_fn(self):
        self.assertEqual(unmarked("""\
            // erasure: keep dyn

            fn f() {
                any(a())
            }
            """), [4])
        self.assertEqual(unmarked("""\
            // erasure: keep dyn
            #[inline]
            fn f() {
                any(a())
            }
            """), [4])

    def test_kept_statement_first_line_does_not_cover_following_statement(self):
        self.assertEqual(unmarked("""\
            fn f(k: u8) {
                // erasure: keep one
                let v = any(a());
                foo(any(b()));
            }
            """), [4])
        self.assertEqual(unmarked("""\
            fn f(k: u8) {
                // erasure: keep arms
                match k {
                    _ => any(a()),
                }
                foo(any(b()));
            }
            """), [6])
        self.assertEqual(unmarked("""\
            fn f() {
                x.children(vec![ // erasure: keep mixed
                    any(a()),
                ]);
                foo(any(b()));
            }
            """), [5])

    def test_closure_body_marker_does_not_leak_to_outer_fn(self):
        self.assertEqual(unmarked("""\
            fn f() {
                let c = |k: u8| -> AnyView<S> { // erasure: keep closure
                    any(a())
                };
                any(b())
            }
            """), [5])
        self.assertEqual(unmarked("""\
            fn f() {
                // erasure: keep inner
                fn inner() -> AnyView<S> {
                    any(a())
                }
                any(b())
            }
            """), [6])

    def test_qualified_paths_detected(self):
        self.assertEqual(unmarked("""\
            fn f() {
                let a = frust::any(x());
                let b = authoring::any(y());
                let c = frust::authoring::any(z());
            }
            """), [2, 3, 4])

    def test_anyview_new_ignored(self):
        self.assertEqual(unmarked("fn f() { let a = AnyView::new(x()); }\n"), [])

    def test_non_calls_ignored(self):
        self.assertEqual(unmarked("""\
            #[cfg(any(unix, windows))]
            fn f(v: Vec<u8>) -> bool {
                // any(x) in a comment
                let s = "any(x)";
                let c = cfg!(any(unix, windows));
                v.iter().any(|b| *b > 1)
            }
            """), [])

    def test_nested_any_reports_each_line_once(self):
        self.assertEqual(unmarked("""\
            fn f() {
                any(
                    any(x()),
                );
            }
            """), [2, 3])

    def test_foreign_path_not_erasure(self):
        self.assertEqual(unmarked("fn f() { other::any(x()); }\n"), [])


class UnmarkedDriverTest(DriverFixture, unittest.TestCase):
    FIXTURE = "fn f() {\n    let a = any(x());\n    // erasure: keep why\n    let b = any(y());\n}\n"

    def test_listed_and_exit_one(self):
        code, out = self.main("--check", "--unmarked", self.path)
        self.assertEqual(code, 1)
        self.assertIn("lib.rs:2: U unmarked erasure", out)
        self.assertNotIn("lib.rs:4:", out)
        self.assertIn("1 candidate(s)", out)

    def test_clean_exits_zero(self):
        with open(self.path, "w", encoding="utf-8") as fh:
            fh.write("fn f() {\n    // erasure: keep why\n    let a = any(x());\n}\n")
        code, out = self.main("--check", "--unmarked", self.path)
        self.assertEqual(code, 0)
        self.assertNotIn(" U ", out)

    def test_flag_absent_no_u_output(self):
        code, out = self.main("--check", self.path)
        self.assertEqual(code, 0)
        self.assertNotIn("U unmarked", out)

    def test_write_changes_nothing(self):
        before = self.read(self.path)
        self.main("--write", "--unmarked", self.path)
        self.assertEqual(self.read(self.path), before)

    def test_combines_with_other_rules(self):
        with open(self.path, "w", encoding="utf-8") as fh:
            fh.write("use frust::{Column, any, text};\n"
                     "fn f() -> V {\n    Column(vec![any(text(\"a\"))])\n}\n")
        code, out = self.main("--check", "--strict", "--t5", "--t6", "--t7", "--unmarked", self.path)
        self.assertEqual(code, 1)
        self.assertIn("lib.rs:3: T2", out)
        self.assertIn("lib.rs:3: U unmarked erasure", out)

    def test_stats_unmarked(self):
        code, out = self.main("--stats", "--unmarked", self.path)
        self.assertEqual(code, 0)
        self.assertIn("lib.rs: unmarked=1", out)
        self.assertIn("total: unmarked=1", out)

    def read(self, path):
        with open(path, encoding="utf-8") as fh:
            return fh.read()


if __name__ == "__main__":
    unittest.main()


class ModuleScopedShadowTest(unittest.TestCase):
    def test_nested_mod_fn_does_not_block_parent_but_blocks_inside(self):
        src = ("fn outer() -> V { Column(vec![a, b]) }\n"
               "#[cfg(test)]\nmod tests {\n"
               "    fn column(x: u32) -> u32 { x }\n"
               "    fn inner() -> V { Column(vec![a, b]) }\n}\n")
        res = cm.rewrite(GLOB + src, APIS)
        lines = [c.line for c in res.candidates]
        self.assertEqual(lines, [2])
        self.assertTrue(any("local fn `column`" in n for _, n in res.notes))

    def test_parent_fn_still_shadows_nested_mod(self):
        src = ("fn column(x: u32) -> u32 { x }\n"
               "mod tests {\n    fn inner() -> V { Column(vec![a, b]) }\n}\n")
        res = cm.rewrite(GLOB + src, APIS)
        self.assertEqual(res.candidates, [])

    def test_nested_mod_fn_does_not_block_origin_gate(self):
        s = cm.Source("fn a() { child(1); }\nmod m { fn child() {} fn b() { child(2); } }\n")
        self.assertFalse(s.fn_shadows("child", s.src.index("child(1)")))
        self.assertTrue(s.fn_shadows("child", s.src.index("child(2)")))


class AccumulatorMarkerTest(unittest.TestCase):
    def test_marked_accumulator_covers_pushes(self):
        self.assertEqual(unmarked("""\
            fn f() {
                let mut kids: Vec<AnyView<S>> = Vec::new(); // erasure: keep accumulator
                kids.push(any(a()));
                kids.push(any(b()));
                kids.insert(0, any(c()));
                kids.extend([any(d())]);
            }
            """), [])

    def test_marker_alone_above_and_untyped_initializer(self):
        self.assertEqual(unmarked("""\
            fn f() {
                // erasure: keep accumulator
                let mut kids = Vec::<AnyView<S>>::new();
                kids.push(any(a()));
            }
            """), [])

    def test_unmarked_accumulator_reports_each_push(self):
        self.assertEqual(unmarked("""\
            fn f() {
                let mut kids: Vec<AnyView<S>> = Vec::new();
                kids.push(any(a()));
                kids.push(any(b()));
            }
            """), [3, 4])

    def test_bare_marker_covers_nothing(self):
        self.assertEqual(unmarked("""\
            fn f() {
                let mut kids: Vec<AnyView<S>> = Vec::new(); // erasure: keep
                kids.push(any(a()));
            }
            """), [3])

    def test_other_vec_and_non_anyview_not_covered(self):
        self.assertEqual(unmarked("""\
            fn f() {
                let mut kids: Vec<AnyView<S>> = Vec::new(); // erasure: keep accumulator
                let mut other: Vec<AnyView<S>> = Vec::new();
                let mut nums: Vec<u8> = Vec::new(); // erasure: keep not a view vec
                other.push(any(a()));
                nums.push(any(b()));
            }
            """), [5, 6])

    def test_other_fn_and_rebinding_not_covered(self):
        self.assertEqual(unmarked("""\
            fn f() {
                let mut kids: Vec<AnyView<S>> = Vec::new(); // erasure: keep accumulator
                kids.push(any(a()));
                let mut kids: Vec<AnyView<S>> = Vec::new();
                kids.push(any(b()));
            }
            fn g(kids: &mut Vec<AnyView<S>>) {
                kids.push(any(c()));
            }
            """), [5, 8])


class InCallMarkerTest(unittest.TestCase):
    def test_own_line_marker_inside_parens_covers_call(self):
        src = """\
            fn f() {
                x.push(any(f(
                    // erasure: keep why
                    a,
                    b,
                )));
            }
            """
        self.assertEqual(unmarked(src), [])

    def test_marker_after_closing_paren_covers_only_next_line(self):
        src = """\
            fn f() {
                x.push(any(f(
                    a,
                )));
                // erasure: keep why
                y.push(any(g()));
                z.push(any(h()));
            }
            """
        self.assertEqual(unmarked(src), [2, 7])

    def test_marker_in_closure_body_does_not_escape(self):
        src = """\
            fn f() {
                x.push(any(w(|| {
                    // erasure: keep why
                    1
                })));
            }
            """
        self.assertEqual(unmarked(src), [2])

    def test_bare_in_call_marker_covers_nothing_under_u(self):
        src = """\
            fn f() {
                x.push(any(f(
                    // erasure: keep
                    a,
                )));
            }
            """
        self.assertEqual(unmarked(src), [2])

    def test_t_rule_keep_exemption(self):
        src = GLOB + textwrap.dedent("""\
            fn f() {
                s.child(any(f(
                    // erasure: keep why
                    a,
                )));
            }
            """)
        res = cm.rewrite(src, APIS)
        self.assertEqual(res.candidates, [])
        plain = cm.rewrite(src.replace("// erasure: keep why", "// note"), APIS)
        self.assertEqual(len(plain.candidates), 1)

    def test_rustfmt_stable_placements(self):
        import shutil
        import subprocess
        rustfmt = shutil.which("rustfmt")
        if rustfmt is None:
            self.skipTest("rustfmt not on PATH")
        src = textwrap.dedent("""\
            fn a() {
                x.push(any(f(a, b))); // erasure: keep why
            }
            fn b() {
                let mut kids: Vec<AnyView<S>> = Vec::new(); // erasure: keep accumulator
                kids.push(any(g()));
            }
            fn c() {
                // erasure: keep why
                y.push(any(h()));
            }
            fn d() {
                y.push(any(f( // erasure: keep why
                    aaaaaaaaaaaaaaaaaaaa, bbbbbbbbbbbbbbbbbbbbbbbb, cccccccccccccccccccccc, dddddddddddddddddd)));
            }
            """)
        self.assertEqual(unmarked(src), [])
        out = subprocess.run([rustfmt, "--edition", "2024", "--emit", "stdout"], input=src,
                             capture_output=True, text=True)
        if out.returncode != 0:
            self.skipTest("rustfmt failed: " + out.stderr[:200])
        self.assertEqual(unmarked(out.stdout), [], out.stdout)
