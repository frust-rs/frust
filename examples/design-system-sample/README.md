# `design-system-sample` — the out-of-tree design-system proof

A standalone Cargo workspace holding the first Frust design system written
**outside** the framework: a themed widget catalog plus its installer, built on
the `frust` facade's public API alone, with every built-in catalog
(Material/Cupertino/Glyph) compiled off.

| Crate | Is |
|-------|-----|
| `sample-design` | the design system — tokens, three widgets, one transition pattern, an `install()` |
| `sample-app` | a minimal desktop app that installs it and renders all three widgets |

## What this proves

Frust claims a third party can build a design system on the same seam its own
three catalogs use. Nothing demonstrated that end to end — the built-in
catalogs live *inside* `frust-widgets` and can reach anything it declares, so
they cannot detect a gap in the public surface. This workspace can, because a
missing symbol here is a compile error rather than a private-item access
somebody didn't notice.

Concretely, the build proves all of the following are reachable from a crate
whose **only** dependency is `frust`:

- **Token authoring** — `Theme::neutral()` as a design-language-free baseline,
  `ThemeBuilder`'s `map_colors_light`/`map_colors_dark`/`map_shape`/`map_motion`
  per-group editors, `DesignLanguage::Custom("sample")` as an identity tag, and
  `ThemeBuilder::extension` for a typed, no-lock-in token attachment
  (`SampleAccents`) resolved back out with `Theme::extension::<T>()`.
- **All three widget-authoring shapes**, each against `frust::authoring`:
  a **leaf** (`badge`, paint-only, shapes its own glyph run through
  `frust::authoring::text`), a **single-child container** (`panel` —
  `build_child`/`rebuild_child`/`teardown_child`, `route_event_single`,
  `visit_children!`, `semantics` forwarding via `ChildPod::semantics_child`),
  and an **interactive control** (`chip` — `erase_callback`/`ErasedCallback`,
  pointer capture, fire-on-up-inside, `PRESSED_OPACITY`).
- **Motion** — `frust::motion` is *not* catalog-gated, so a catalogs-off design
  system can still implement `TransitionPattern` (`reveal::SampleReveal`) and
  feed it to `pattern_switcher`. This was an open question before the sample;
  the fact that `reveal.rs` compiles at all is the answer.
- **Composing over the baseline set** — the chip's label is the framework's own
  `text` widget under `ThemeTextColor::OnPrimaryContainer`; the baseline widget
  set survives `default-features = false` and is meant to be built on.
- **Testing** — every widget is unit-tested against a hand-rolled recording
  `PaintScene` and a synthetic event pass (`src/testing.rs`), with no GPU,
  window, or bundled font. `LayoutCtx::with_resources`, `PaintCtx::new`,
  `EventCtx::new` and `BuildCtx::new` are all public.

### What it found

**`Widget::semantics` cannot be exercised from out of tree.** Every widget here
carries a semantics impl and the vocabulary to write one is fully public
(`SemanticsCtx::push_node`/`push_container`, `Role`, `Node`, `Action`,
`ChildPod::semantics_child`), but **nothing can drive a semantics pass**:
`SemanticsCtx::new` is `pub(crate)` in `frust-core`, and the only public
producer — `frust_core::RenderRoot::semantics()` — is not re-exported by the
`frust` facade. The framework's own catalogs are unaffected (they reach
`RenderRoot` through a `frust-core` dev-dependency, which
`crates/frust-widgets/tests/semantics_tree.rs` does). An external author's only
options today are to depend on `frust-core` directly — exactly the dependency
`docs/CODE_STANDARDS.md` tells app/design-system code not to declare — or to
ship semantics impls untested. Recorded here rather than worked around; closing
it is a facade change, not a sample change.

## The catalogs-off contract

`default-features = false` on the `frust` dependency lives on **this
workspace's** `[workspace.dependencies]` entry, not on either member's edge:
Cargo silently ignores that key when it is written on an *inherited* dependency
(defaults stay on) and hard-errors when the workspace entry left it
unspecified. See the manifests' own comments.

**Verify it:**

```bash
cargo tree -e features -i frust -p sample-app
```

The output must contain **no `frust feature "..."` line at all** — only the two
crates that depend on it:

```
frust v0.1.0 (.../crates/frust)
├── sample-app v0.1.0 (.../sample-app)
│   └── sample-app feature "default" (command-line)
└── sample-design v0.1.0 (.../sample-design)
    └── sample-design feature "default"
        └── sample-app v0.1.0 (.../sample-app) (*)
```

The negative control this check used to have — a catalogs-on dependent
turning `glyph`/`material`/`cupertino`/`glyph-fonts` back on for `frust` via
Cargo's feature-additivity (`cargo tree -e features -i frust -p
frust-native-widgets`, from the framework repo root) — no longer exists to
demonstrate: `frust` carries no catalog cargo feature at all any more (the
three built-in catalogs left the tree as `frust-material`/`frust-cupertino`/
`frust-glyph`, ordinary sibling plugin crates each app opts into beside
`frust`, the same way this workspace's `sample-app` opts into
`sample-design`). Printing no catalog-name feature line is therefore not
this workspace's special property any more — it is true of *every* `frust`
dependent in the repo now, because there is no catalog-name feature left
anywhere to print.

### The rule this implies for design-system crates

**A design-system crate enables nothing on `frust` by existing.** The
Cargo-feature-additivity hazard this section used to warn about — one
`features = ["glyph"]` line in a design-system crate silently turning
Material/Cupertino/Glyph back on for *every* app that depends on it, with no
way to switch them off again — is retired along with the catalog feature
graph itself: `frust` has nothing named `glyph`/`material`/`cupertino`/
`glyph-fonts` left to enable. A design-system crate today is shaped exactly
like `sample-design`: a sibling crate an app depends on directly, built on
`frust::authoring` plus whatever `frust` re-exports by name — the same seam
the three built-in design systems (`frust-material`/`frust-cupertino`/
`frust-glyph`) use.

This workspace stays **excluded** from the framework's root workspace (root
`Cargo.toml`'s `[workspace] exclude`) for **out-of-tree realism and
`target/` isolation**, not to dodge a resolver hazard: a design-system crate
must resolve `frust` exactly the way a real third-party crate would, from
its own checkout with its own lockfile and its own `target/` dir, never as a
root-workspace member riding the framework's shared build.

## The install-timing contract

`sample_design::install()` calls `frust::set_default_theme(sample_theme())`,
and it is called from `frust::app!`'s `setup = { .. }` block. All three halves
of that are load-bearing:

- **`setup`, not `Component::init`** — `setup` runs before the shell builds its
  first frame and before any component initialises. `Component::init` has no
  kept ordering contract against the shell's own theme seeding, so a theme
  installed there may or may not be in place for frame one.
- **`set_default_theme`, not `set_app_theme`** — `set_app_theme` is the app-facing
  *override* and it pins brightness, so an app themed that way stops following
  platform light/dark. `set_default_theme` seeds the *base* the shell keeps
  re-deriving light/dark against. Precedence: `set_app_theme` override →
  `set_default_theme` base → the shell's own `Theme::neutral()` fallback.
- **Fonts ride the same seam** — a system with a bundled typeface pushes its
  bytes through `frust::register_app_fonts` in the same `install()`. Sample
  bundles none on purpose (a megabyte of TTF proves nothing the theme half
  doesn't); `frust::glyph_theme::install` is the built-in example of both calls
  together.

## Gate

This workspace is standalone — run its gate **from this directory**, not with
`-p` from the framework repo root:

```bash
cargo test \
  && cargo clippy --all-targets -- -D warnings \
  && cargo fmt --check
```

Plus the catalogs-off check above.

**Not yet run:** the desktop visual pass (`cargo run -p sample-app` — a window
showing the badge, chip and lift-and-fade switcher under the Custom-tagged
theme). It needs a display; like every other rendering change in this repo it
has no automated pixel-diff counterpart.

## Not a design system worth shipping

Three widgets, two hues, one pattern. It is scoped to be the smallest thing
that exercises every authoring shape — deliberately, so that a `frust create`
design-system template distilled from it stays small enough to read.
