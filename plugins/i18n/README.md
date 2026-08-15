# frust-i18n

A platform-independent internationalization/localization plugin for frust apps, built on the
[Fluent Project](https://projectfluent.org/) — locale-aware message resolution, compile-time
Fluent bundle loading (`locales!`), system-locale detection, and (behind the `formatting`
feature) ICU4X-backed number/date/currency formatting.

**Internationalization (i18n)** is what this crate does for you: message lookup, plural/select
rules, and number/date/currency shaping, all driven by a locale tag. **Localization (l10n)** is
the part only you can do — writing the actual translated `.ftl` files. This README covers the
i18n half end to end; §3 covers the l10n workflow (authoring locale files) it expects from you.

Like `frust-native-widgets` (see `docs/PLUGINS_CODE_STANDARDS.md`'s Plugin Conventions), this is
**one crate spanning both plugin tiers** rather than a platform-core/facade-glue split: the
default-on `frust-api` feature gates the reactive `I18n` handle (the facade-glue half). With it
off, `cargo check -p frust-i18n --no-default-features` still resolves to the platform-plugin
charter line — no `frust` facade crate anywhere in the tree, just message lookup, negotiation,
and detection. See §7 for the full feature matrix.

Like every frust **platform plugin**, this crate is added to your app's own `Cargo.toml`
alongside `frust` (the pubspec model) — the `frust` facade does not re-export it.

### Capability × platform

| Capability | Feature gate | Android | iOS | Desktop |
|---|---|---|---|---|
| Message lookup (`t`/`t_args`, dynamic) | always on | yes | yes | yes |
| Typed keys (`keys::…`, compile-checked) | always on (via `locales!`) | yes | yes | yes |
| Reactive locale handle (`I18n`, `provide_i18n`/`use_i18n`) | `frust-api` (default) | yes | yes | yes |
| System-locale detection (`system_locales`) | always on | yes (`LocaleList` via JNI) | yes (`NSLocale.preferredLanguages`) | yes (`sys-locale`) |
| Number/percent/currency/date/time formatting (`fmt::`) | `formatting` (default) | yes | yes | yes |

Every capability above is available on every platform — unlike `camera`/`haptics`/`iap`, this
plugin has no per-platform capability gap, only Cargo feature gates.

---

## 1. What it is

`frust-i18n` resolves Fluent messages (`.ftl` files) against a negotiated locale chain, offers
both a dynamic (`t("key")`) and a compile-checked typed (`keys::key(...)`) way to call one, and
(with `frust-api`, default-on) wraps that in a reactive handle that re-negotiates and wakes the
app on a locale switch. Detection (`system_locales`) reads the platform's configured locale
list; formatting (`fmt::`, `formatting` feature, default-on) adds ICU4X-backed
decimal/percent/currency/date/time rendering, reachable both directly and from inside an `.ftl`
message via `NUMBER()`/`DATETIME()`.

---

## 2. Install

### 2a. The frust TUI Add Plugin dialog (recommended)

`frust tui` → Add Plugin → `i18n` applies every step in §2b in one shot, idempotently:

- adds the `frust-i18n` Cargo dependency;
- seeds a starter `locales/en/main.ftl` at your project root (only if that exact path doesn't
  already exist);
- appends `frust_i18n::locales!("locales");` to `src/lib.rs`, **after** the generated
  `frust::app!(...)` invocation (that line, and the ordering, is a hard rule — see §2b step 3).

No manifest permission, plist key, Gradle module, or Swift package is needed — like
`frust-database`, this crate reaches no OS capability at all beyond a read: message resolution
and formatting are pure Rust, and `src/detect`'s platform reads (Android JNI `LocaleList`, iOS
`NSLocale`) need no init step beyond what every plugin already gets on Android
(`frust_plugin::android`'s platform handle) and nothing extra on Apple.

### 2b. By hand — the manual equivalent

1. Cargo dependency:

   ```toml
   # app Cargo.toml — [dependencies]
   frust-i18n = { path = "<frust>/plugins/i18n" }  # crates.io later
   ```

   `<frust>` is the path to your frust checkout — derive it from the `frust = { path = "…" }`
   line the scaffold already wrote.

2. Create `locales/en/main.ftl` at your project root (alongside `Cargo.toml`):

   ```ftl
   # English locale — add sibling directories (de/, fr/, ...) with the same file names.
   # Syntax: https://projectfluent.org/fluent/guide/
   hello = Hello, { $name }!
   ```

   This is exactly the file the Add Plugin dialog seeds — §3 covers the directory convention
   once you add more locales or files.

3. Append one line to `src/lib.rs`, **after** the generated `frust::app!(...)` invocation
   (never edit that invocation itself — the scaffold's own header comment says the same):

   ```rust
   frust_i18n::locales!("locales");
   ```

   This is the macro that reads `locales/`, validates every `.ftl` file at compile time, and
   generates `locale_set()`/`engine()`/`keys` in the module it's invoked from (here, the crate
   root — see §4b).

That's the whole install: no manifest, plist, Gradle module, or Swift package, matching
exactly what §2a's dialog applies.

---

## 3. Authoring locales

### Directory convention

```
locales/
  en/
    main.ftl
    forms/
      cart.ftl        # nested dirs are walked; registered as "forms/cart.ftl"
  de/
    main.ftl
```

- One directory per locale, directly under the path you pass to `locales!` — named by its
  BCP-47 identifier (`en`, `en-US`, `zh-Hans-CN`). A directory name that isn't a well-formed
  BCP-47 identifier is a **compile error** naming the offending directory.
- Any number of `.ftl` files per locale, in any nested layout. Everything not ending `.ftl`,
  and anything starting with `.` (editor/OS droppings), is skipped.
- File names are registered for diagnostics only — the Fluent message **id** is the actual
  lookup key, and must be unique across a locale's own files (a duplicate id keeps its
  earliest definition, matching `fluent-bundle`'s own concatenation rule).

### Fluent syntax

Full guide: <https://projectfluent.org/fluent/guide/>. The two shapes you'll reach for most:

```ftl
# Interpolation
greeting = Hello, { $name }!

# Plural/select — CLDR plural categories (`one`/`other` in English; more
# categories in some languages), `*` marks the required default arm
cart-items =
    { $count ->
        [one] one item
       *[other] { $count } items
    }
```

### Fallback semantics

`locales!`'s `fallback:` argument (default `en`, if an `en/` directory exists) names the
locale that **closes every negotiated chain** — negotiation always ends there, so it has to be
the locale whose message set is complete. It's also the locale the typed `keys::` surface
(§4b) is generated from.

A non-fallback locale missing a message the fallback has is **not an error** — that message is
served from the fallback at runtime, and the gap is reported as a **build-log warning**:

```
warning: frust-i18n: locales!: `de` is missing 2 of `en`'s messages, which stay
fallback-served: only-en, save
```

Capped at 8 named keys before summarizing the rest as `(and N more)`. This is proc-macro
`eprintln!` output reaching the build log — an **unchanged, cached expansion does not
re-print it**, so a clean incremental rebuild won't re-surface a gap you already saw once;
touch the `.ftl` file (or force a rebuild of the invoking crate) to see it again.

With no `en/` directory and no explicit `fallback:` argument, expansion fails asking you to
name one: `locales!("locales", fallback: "de")`.

### What a broken `.ftl` looks like, and how to fix it

Every `.ftl` file is parsed **while the macro expands** — a malformed message is a
*compilation* failure, not something discovered when the screen holding it opens:

```
error: `locales!`: malformed Fluent syntax in `locales/en/main.ftl`
         locales/en/main.ftl:3:2: Expected a token starting with "="
 --> src/lib.rs:1:22
  |
1 | frust_i18n::locales!("locales");
  |                      ^^^^^^^^^
```

The message names the file, then a `<file>:<line>:<column>` position (1-based, columns
counted in characters) for each parser complaint. Every file in the tree is checked even
after one fails, so a fresh batch of translations reports every mistake in one build rather
than one at a time. There is no partial/best-effort load — fix the syntax at the named
position and rebuild; a malformed file blocks compilation entirely rather than silently
dropping the one broken message.

A directory name that isn't BCP-47 (`en_US.UTF-8` instead of `en-US`) and a `fallback:` that
names a directory that doesn't exist are compile errors the same way, each naming what was
wrong.

---

## 4. API

### 4a. Dynamic lookup — `t` / `t_args`

The reactive `I18n` handle (§4c) and the headless `ChainResolver` (`Engine::with_chain`) both
expose:

```rust
i18n.t("greeting");                                    // no arguments
i18n.t_args("greeting", Some(&frust_i18n::args!("name" => "Ada")));
```

A missing key never panics or propagates an error here — it returns the key itself and logs a
`log::warn!` (see §8). `args!` is shorthand for building a `FluentArgs`:

```rust
let args = frust_i18n::args!("name" => "Ada", "count" => 3);
assert_eq!(args.iter().count(), 2);
```

### 4b. Typed keys — `keys::`

`locales!` generates one function per message **with a value** in the fallback locale (a
message with only attributes — `save = \n    .tooltip = Save this document` — contributes no
function; attributes stay reachable only through the dynamic `key.attribute` lookup). A
typo'd key is a **missing function** — a compile error, not a runtime miss:

Assuming `frust_i18n::locales!("locales");` was invoked at the crate root (§2), `locale_set()`/
`engine()`/`keys` are items of that same module:

```rust
use frust_i18n::Resolve;

// Any `&impl Resolve` works as the first argument — the reactive `I18n` handle (§4c),
// `Engine::with_chain(...)`, or your own implementor.
let chain = engine().negotiate(&requested);
let resolver = engine().with_chain(&chain);

keys::greeting(&resolver, "Ada");    // "Hello, Ada!" — an interpolated arg is wrapped in
                                      // invisible bidi isolation marks by default; see §8
                                      // before comparing exactly
keys::cart_items(&resolver, 1);      // "one item" (the `[one]` arm has no interpolation,
                                      // so no marks either)
keys::cart_items(&resolver, 5);      // "5 items" — `$count` is isolated too
```

Each generated function's parameters are typed from the message's own `$variable`
references — direct references only; a variable reached indirectly through `{ other-message
}` is not part of the signature. A resolution failure softens the same way `t`/`t_args` does:
a warn-level log plus the key rendered as its own text.

### 4c. Reactive — `I18n`, `provide_i18n`/`use_i18n`

Only with the default-on `frust-api` feature. `I18n` pairs an `Engine` with the app's active
locale as a tracked `RwSignal` — reading `.locale()` (or calling `t`/`t_args`/a typed key
through it) inside a live rebuild subscribes to `set_locale`, so a locale switch wakes the
shell's next frame through the normal signal-write path:

```rust
use frust_i18n::{I18n, Locale, active_locale, expect_i18n, provide_i18n, use_i18n};

// Call where a reactive Owner is ambient — app setup, or a root Component::init.
let requested: Vec<Locale> = frust_i18n::system_locales().unwrap_or_default();
let i18n = I18n::new(locale_set(), &requested).expect("locale set is non-empty");
provide_i18n(i18n.clone());

// From any descendant Component's build:
let greeting = expect_i18n().t("greeting");

// From a settings screen — re-negotiates and wakes the app:
i18n.set_locale("de".parse().expect("valid BCP-47 tag"));
```

`I18n` is cheap to `Clone` — every clone shares the same engine, signal, and negotiated
chain. `use_i18n()` returns `None` if nothing's been provided; `expect_i18n()` panics with a
named message instead; `active_locale()` is `use_i18n().map(|i| i.locale())` — a convenience
for a rebuild that only needs the locale, not the whole handle.

**Message locale vs. format locale.** `.locale()` is a negotiated *message* locale — it can
only ever be one of the locales your app actually shipped `.ftl` catalogs for (e.g. `en`),
because negotiation collapses a regional request like `en-GB` down to whatever bundle is
available. ICU4X formatting (§4e) has no such constraint — it ships full CLDR data for every
region regardless of which message bundles you compiled in. Use `.format_locale()` for every
`fmt::` call instead: it recovers the best *requested* tag whose language matches the
negotiated message locale (`en-GB` requested over an `en`-only bundle set still returns
`en-GB`), falls back to the first requested tag if none share the message locale's language,
and falls back to `.locale()` itself if nothing was requested at all:

```rust
let i18n = I18n::new(locale_set(), &["en-GB".parse()?])?;
i18n.locale();          // "en" — the bundle set only ships `en`, never a region
i18n.format_locale();   // "en-GB" — the request survives for CLDR formatting
```

`format_locale()` is a tracked read too — same `set_locale`-subscribes contract as `.locale()`.

### 4d. Detection — `system_locales`

```rust
let requested: Vec<frust_i18n::Locale> = frust_i18n::system_locales()?;
```

Returns the platform's configured locale list, most-preferred first — Android's
`LocaleList`, iOS's `NSLocale.preferredLanguages`, or (macOS/Linux/Windows) `sys-locale`.
**Re-queries the OS on every call — never cached** (see §6: this is what makes a
"check-again-after-the-setting-changed" flow possible, since there's no live notification).
An unparseable individual tag is skipped and logged, not fatal; an empty result (nothing
parsed at all) is `I18nError::Detection`.

### 4e. Formatting — `fmt::`

Only with the default-on `formatting` feature (ICU4X-backed, CLDR data baked into the
binary — no provider to configure, no async load):

```rust
use frust_i18n::fmt::{self, CivilDate, DateLength};

let de: frust_i18n::Locale = "de-DE".parse()?;
let en: frust_i18n::Locale = "en-US".parse()?;

fmt::decimal(&de, 1234.56);                                    // "1.234,56"
fmt::percent(&en, 75.0);                                       // "75%"
fmt::currency(&de, 9.99, "EUR")?;               // "9,99\u{a0}€" — U+00A0, CLDR's own space
fmt::date(&de, CivilDate { year: 2024, month: 1, day: 31 }, DateLength::Medium)?; // "31.01.2024"
```

Pulling the locale from a live `I18n` handle (§4c) instead of a literal? Pass
`i18n.format_locale()`, never `i18n.locale()` — `fmt::` calls want the caller's actual
requested region, not the negotiated message locale:

```rust
fmt::decimal(&i18n.format_locale(), 1234.56);
```

`decimal`/`percent` degrade to Rust's own rendering (with a log) rather than failing — a
localized-looking number beats no number. `currency`/`date`/`time`/`datetime` return
`Result<String, I18nError>`, since a bad currency code or an impossible date is a caller
error worth surfacing.

The same formatters are reachable from inside a `.ftl` message, once you register them —
call `with_icu_functions` **last**, after every other `LocaleSet` builder call (registering
your own `NUMBER`/`DATETIME` too fails `Engine::new` with a duplicate-function-id error):

```rust
let engine = frust_i18n::Engine::new(
    frust_i18n::fmt::with_icu_functions(locale_set()),
)?;
```

```ftl
total = You owe { NUMBER($amount, style: "currency", currency: "USD") }
placed = Order placed { DATETIME($when, dateStyle: "medium") }
```

Honored named options: `NUMBER`'s `style` (`decimal`/`percent`/`currency`) and `currency`;
`DATETIME`'s `dateStyle`/`timeStyle`. Anything else (ECMA-402-style
`minimumFractionDigits`, `useGrouping`, …) is ignored at `debug` log level, never a warning or
an error — these run on a UI's formatting path, potentially every frame.

---

## 5. Persistence recipe — restoring a user's chosen locale

`I18n` holds the active locale in memory only; nothing in this crate persists a choice across
a kill/relaunch. Pair it with `frust-shared-preferences` — no new machinery, just this crate's
existing API plus that plugin's:

Assumes `frust_i18n::locales!("locales");` was invoked at the crate root, as in §2 — so
`locale_set()` below is that module's generated function, unqualified:

```rust
use frust_i18n::{I18n, Locale, provide_i18n};
use frust_shared_preferences::SharedPreferences;

const LOCALE_KEY: &str = "locale";

pub struct AppState {
    i18n: I18n,
    prefs: Option<SharedPreferences>,
    // ...
}

impl AppState {
    fn new() -> Self {
        let prefs = SharedPreferences::standard().ok();

        // A previously-saved choice wins; otherwise fall back to the system's list.
        let requested: Vec<Locale> = prefs
            .as_ref()
            .and_then(|p| p.get_string(LOCALE_KEY))
            .and_then(|saved| saved.parse::<Locale>().ok())
            .map(|locale| vec![locale])
            .or_else(|| frust_i18n::system_locales().ok())
            .unwrap_or_default();

        let i18n = I18n::new(locale_set(), &requested)
            .expect("the compiled-in locale set is never empty");
        provide_i18n(i18n.clone());

        Self { i18n, prefs }
    }

    /// Call from a settings screen when the user picks a locale explicitly.
    fn set_locale(&self, locale: Locale) {
        self.i18n.set_locale(locale.clone());
        if let Some(prefs) = &self.prefs {
            // Best-effort: a save failure loses only the *next* launch's default,
            // never today's already-switched UI.
            let _ = prefs.set_string(LOCALE_KEY, locale.to_string());
        }
    }
}
```

This is the same "degrade to unavailable rather than panic" shape `templates/app/src/lib.rs`'s
own notes/draft persistence uses — a `None` preferences handle (an old scaffold, or a backend
that hasn't landed on this target yet) just means the choice doesn't survive this run, never a
crash. `SharedPreferences`'s calls are plain synchronous reads/writes (no `spawn_blocking`
needed) — see that crate's own doc for why.

---

## 6. Platform notes

- **Android** — detection reads `Resources.getSystem().getConfiguration().getLocales()`
  (`android.os.LocaleList`) through a scoped JNI attach via `frust_plugin::android`, no
  Kotlin helper class involved. Every call re-queries this fresh (§4d) — a configuration
  change (the user switches system language while your app is backgrounded) is visible on
  the very next `system_locales()` call, not live-pushed.
- **iOS** — detection reads `NSLocale.preferredLanguages` directly through
  `objc2-foundation` — no platform-handle init step (`objc2` reaches the ObjC runtime
  globally). macOS shares the desktop backend below instead of this one.
- **Desktop (macOS/Linux/Windows)** — detection uses the `sys-locale` crate
  (`get_locales()`, falling back to `get_locale()`, falling back to a `LANG`/`LC_*`
  POSIX-style env read only if both come back empty).

**Not reactive: a live system-locale change is not observed automatically.**
`system_locales()` re-queries the OS on every call (§4d), but nothing in this crate
subscribes to a platform notification and re-negotiates `I18n` for you while the app keeps
running. An app that wants to react to a live change polls `system_locales()` itself (e.g.
on resume) and calls `I18n::set_locale`. This is a v1 scope line, not a bug — see
`docs/LIMITATIONS.md` for this framework's accepted-gap register.

---

## 7. Feature flags

| Feature | Default | Gates |
|---|---|---|
| `frust-api` | on | The reactive `I18n` handle, `provide_i18n`/`use_i18n`/`expect_i18n`/`active_locale` (§4c) — pulls in the `frust` facade. |
| `formatting` | on | `fmt::` — ICU4X decimal/percent/currency/date/time formatting plus the `NUMBER`/`DATETIME` Fluent functions (§4e) — pulls in `icu_decimal`/`icu_datetime`/`icu_plurals`/`icu_experimental` and their support crates. |

Independently toggleable — all four combinations (default, `--no-default-features`,
`--features formatting` alone, `--features frust-api` alone) compile.
`cargo check -p frust-i18n --no-default-features` resolves to this tier's platform-plugin
charter line: **no `frust` facade crate anywhere in the tree**. What's left is message lookup
(`t`/`t_args`/`keys::`), locale negotiation, and detection (`system_locales`) — no reactive
glue, no ICU formatting. A headless service that only needs message lookup reaches for
exactly this.

---

## 8. Troubleshooting

- **A message renders as its own key** (`greeting` instead of the translated text) — no
  bundle in the negotiated chain has that message. This never panics: `t`/`t_args` and every
  `keys::` function soften a resolution failure to the key itself plus a `log::warn!`. Check
  the warn-level log for which locale(s) were tried, and whether the message id is a typo
  against the `.ftl` source — a typo'd **typed** key is instead a compile error (§3/§4b), so
  this only bites the dynamic `t`/`t_args` path.
- **`fmt::currency` accepts a currency code you expected to be rejected** — a
  well-formed-but-unassigned ISO 4217 code (e.g. `ZZZ`) is deliberately **not** an error:
  CLDR's own fallback renders it with a generic symbol, and this crate has no ISO 4217
  registry to validate against. Only a code that isn't three ASCII letters is
  `I18nError::Format`. Validate against your own product's currency list first if you need to
  reject invalid-but-well-formed codes.
- **Interpolated text has invisible characters around it, or an exact-match test fails
  unexpectedly** — bidi isolation is **on by default**: every interpolated placeable is
  wrapped in `U+2068`/`U+2069` (FSI/PDI, "first strong isolate"/"pop directional isolate")
  so a right-to-left argument can't reorder the text around it. This is invisible in a
  rendered UI but shows up as extra characters in a raw string comparison (e.g. `"Hello,
  \u{2068}Ada\u{2069}!"`). Turn it off with `LocaleSet::with_isolating(false)` only where the
  consumer can't handle the marks (a plain-text export, an exact-match test) — leave it on
  for anything that renders to a screen, since it's the isolation protection you'd be
  trading away, not just the marks.

---

## 9. Manual test checklist

A device gate for `frust-i18n`, against an app depending on the plugin per §2 above (the same
list `docs/PLUGINS_DEVELOPMENT.md`'s i18n section tracks — keep both in sync):

- **Add Plugin scaffold builds clean.** Apply §2 to a fresh scaffold on all three targets with
  zero hand edits.
- **Compile-time validation fires.** In turn: break a `.ftl` file, rename a locale directory
  to a non-BCP-47 name, and delete the fallback directory — confirm each is a *build* failure
  naming the file/line (or directory), not a runtime surprise. Revert, then delete one
  message from a non-fallback `.ftl` and confirm it logs the `warning: frust-i18n:
  locales!:` build-log line instead of failing.
- **Negotiation + fallback, on device.** With at least two locales shipped, negotiate against
  a chain that only partially matches; confirm a message only the fallback has still
  resolves, and a plural/select message (`cart-items`-shaped) picks the right category at a
  boundary count (1 vs. not-1) in at least two locales.
- **Detection reflects the platform, Android + iOS.** Change the system/app language in
  device Settings, relaunch, and confirm `system_locales()` (and an `I18n` built/re-negotiated
  from it) reflects the new list — never live without a relaunch or an explicit re-poll (§6).
- **Formatting matches the locale.** With `formatting` on, confirm `fmt::currency` renders a
  zero-decimal currency (JPY) and a two-decimal one (USD/EUR) correctly, and
  `fmt::date`/`fmt::time` differ visibly between at least two locales (symbol placement,
  separator, hour cycle).
- **Persistence survives a kill/relaunch.** Following §5's recipe: pick a non-default locale,
  kill the app, relaunch, and confirm it reopens in the saved locale rather than re-detecting
  the system one.
- **`--no-default-features` still resolves.** `cargo check -p frust-i18n
  --no-default-features` and `cargo tree -p frust-i18n --no-default-features -e normal`
  confirm no `frust`/ICU4X crate remains in the graph (§7).
- **Macro diagnostics suite.** `cargo test -p frust-i18n --no-default-features --test
  macro_diagnostics -- --ignored` — the `#[ignore]`d trybuild suite pinning every compile-fail
  message named above; not part of the default `cargo test` pass, run it explicitly when
  touching the macro.

---

## 10. Binary size

The binary size impact of `frust-i18n` at the shipped profile (optimized release build), measured
with the same build-twice-and-diff procedure `frust-database` §6 uses for its Turso delta — this
revision also follows §6's *location* instruction (probe out-of-repo), which the previous revision
of this table did not. **These figures supersede the previous table below**; see *Why the previous
table was wrong* at the end of this section.

| Feature set | Binary size | Notes |
|---|---:|---|
| Baseline (no i18n) | 319,152 bytes (0.30 MB) | Same `frust` dependency `frust-i18n` itself uses (default features), linked but never called — a trivial `println!` only |
| Messages only | 534,448 bytes (0.51 MB) | `frust-i18n` with `frust-api`, no `formatting` — message lookup, typed keys, reactive `I18n` handle, all actually called |
| Full default | 1,760,112 bytes (1.68 MB) | `frust-i18n` with `frust-api` + `formatting` (ICU4X) — every formatter actually called |

**Per-feature deltas:**

| Feature | Cost | Notes |
|---|---:|---|
| Messages + reactive | **+215,296 bytes** (+0.21 MB) | `frust-api` feature: locale negotiation, dynamic `t`/typed `keys::`, reactive `I18n` binding |
| Formatting | **+1,225,664 bytes** (+1.17 MB) | `formatting` feature: ICU4X decimal/percent/currency/date/time/plurals — baked CLDR data, not just code (see *Evidence* below) |

### Measurement procedure

Reproducible on a feature-flag change or ICU4X pin bump — re-run this exact procedure and update
the table above, the raw figures, date, and toolchain version.

1. Scaffold a probe crate **outside this repo** (a scratch/temp directory — never a subdirectory
   of this checkout, per `frust-database` §6) with:
   - `Cargo.toml` `path`-dependencies on `frust` (the identical dependency line `frust-i18n`'s own
     optional `frust` edge uses — no `default-features` override) plus an *optional*
     `frust-i18n` (`default-features = false`) and an *optional* `frust-reactive` (only to stand
     up an ambient reactive `Owner` headlessly — the same pattern
     `plugins/i18n/tests/reactive.rs` uses).
   - Two probe-owned Cargo features gating those optional deps additively:
     `messages = ["dep:frust-i18n", "dep:frust-reactive", "frust-i18n/frust-api"]` and
     `full = ["dep:frust-i18n", "dep:frust-reactive", "frust-i18n/frust-api", "frust-i18n/formatting"]`.
     With neither active, `frust-i18n` is not even in the build — the honest "no i18n" baseline.
   - `[profile.release]` matching the ship floor exactly (`lto = "fat"`, `codegen-units = 1`,
     `strip = "symbols"`, `panic = "abort"`).
   - `locales/en/main.ftl` with a plain interpolated message, a plural/select message, and a
     message containing `NUMBER()`/`DATETIME()` placeables.
   - `src/main.rs` whose `main()` **calls and `println!`s every result** from `fmt::decimal`,
     `fmt::percent`, `fmt::currency` (a 2-decimal code like `EUR` and a 0-decimal one like `JPY`),
     `fmt::date`, `fmt::time`, `fmt::datetime`, a dynamic `t`/`t_args` call, a typed `keys::`
     call, and the `NUMBER()`/`DATETIME()` message resolved through an
     ICU-function-registered `Engine` (`fmt::with_icu_functions`, called last, over the
     macro-generated `locale_set()`) — every numeric/date input derived at runtime from
     `std::env::args().len()`, never a literal, so nothing here is const-foldable. **This is the
     step the previous revision of this table got wrong**: a probe that only *lists* the
     dependency, or feeds it compile-time-constant inputs whose result is never read, lets
     `lto = "fat"` + `strip = "symbols"` dead-strip the unreferenced computation (and the ICU4X
     data statics behind it) entirely before it ever reaches the linked binary — measuring the
     cost of nothing.

2. `export CARGO_TARGET_DIR=<probe-dir>/target-<name>` before each build, so the three builds
   neither share nor pollute each other's target directory (nor this repo's own).

3. Build three times with the same release profile:
   - Baseline: `cargo build --release` (`frust-i18n` is `optional = true` and no feature turns
     it on, so it is not compiled at all — not merely feature-gated off)
   - Messages only: `cargo build --release --no-default-features --features messages`
   - Full: `cargo build --release --no-default-features --features full`

4. Record the exact byte count of each resulting binary (`wc -c target-<name>/release/<bin>`),
   both deltas, the date, the exact rustc/cargo version, the build machine's CPU/OS, and the
   target triple.

### Evidence the formatting delta is measuring real data, not another dead-stripped no-op

Three independent checks, all against this run's binaries:

- **The full build's stdout is locale-shaped, not degraded.** Every `fmt::` call above printed a
  real, locale-correct string (`"1.235,56"` decimal, `"￥1,236"` zero-decimal JPY, `"11:30:00 AM"`
  12-hour English time, …) rather than the non-finite/failure fallback path — the formatters ran.
- **Pre-link dependency graph.** `target-full/release/deps/` contains
  `libicu_datetime_data-*.rlib` (~14.2 MB) and `libicu_experimental_data-*.rlib` (~69.5 MB, the
  currency-data crate) — both entirely absent from `target-messages/release/deps/`. The crates
  carrying baked CLDR data are only ever compiled into the `full` build's dependency graph.
- **Where the linked delta actually lands.** Splitting the Mach-O `__TEXT` segment's sections
  (`otool -l`) between the two final binaries: the two `__const` (read-only data) sections grew by
  **1,040,240 bytes**, the `__text` (code) section by only **177,692 bytes** — the growth is
  concentrated in read-only data, exactly where baked static tables live, not in formatter code
  size. (Both binaries are `strip = "symbols"`, so `nm` retains no local symbol names to grep by
  name on this platform — the section-size split is the available per-binary evidence instead.)

**Plausibility.** ~1.17 MB, not the tens-of-MB the raw `icu_experimental_data`/`icu_datetime_data`
rlib sizes might suggest, because ICU4X's `compiled_data` feature bakes each data *marker* (one
static per formatter capability × locale set) separately — only the markers `fmt::decimal`/
`percent`/`currency`/`date`/`time`/`datetime` actually reference get linked; `icu_experimental`'s
other formatters (units, person names, list, relative time, …) this crate never calls contribute
nothing. A low-single-digit-MB delta for exactly this formatter combination is the range this
task's own acceptance bar called plausible, and is consistent with ICU4X's own published
baked-data sizing for a similarly-scoped formatter set.

**2026-08-15 raw figures** (macOS (darwin-25), release/ship profile, rustc 1.97.1, target
`aarch64-apple-darwin`, build machine: Apple M4):

| Build | Binary size | Bytes |
|---|---:|---:|
| Baseline (no i18n) | 0.30 MB | 319,152 |
| Messages only (frust-api) | 0.51 MB | 534,448 |
| Full (frust-api + formatting) | 1.68 MB | 1,760,112 |
| **Messages delta** | **+0.21 MB** | **+215,296** |
| **Formatting delta** | **+1.17 MB** | **+1,225,664** |

The probe was scaffolded, built, and deleted entirely outside this repository (a scratch/temp
directory, never a subdirectory of this checkout, per `frust-database` §6), with a separate
`CARGO_TARGET_DIR` per build; all three `target-*/` directories were removed afterward — no
artifact or tracked residue remains from this run.

### Why the previous table was wrong

The table this one replaced reported a **`+49,680`-byte** `formatting` delta and a
**`+50,144`-byte** `frust-api` delta — both almost certainly measuring nothing. Its own
302,432-byte ("0.29 MB") *baseline* figure was the tell: a build linking a real `frust` app
(vello/wgpu and all) cannot plausibly be 0.29 MB, and it wasn't really measuring one — that
baseline's `frust` dependency was declared but never called into, so `lto = "fat"` +
`strip = "symbols"` dead-stripped it down to almost nothing, exactly as it must have dead-stripped
the unreferenced ICU4X data statics in the `formatting` build (whose result also never reached a
`println!`, or did so only from compile-time-constant inputs). This revision's probe fixes both:
every call's result is `println!`-ed, every input is runtime-derived, and — per the *Evidence*
section above — the size jump lands in read-only data, not in dead space. The `messages` delta
moved for the identical reason: this probe's `messages`-feature build actually resolves a dynamic
`t`/`t_args` call, a typed `keys::` call, and reads a reactive handle's `.locale()`, none of which
the previous probe's build exercised either.

One correction from the previous revision beyond the numbers: the probe now lives fully
**out-of-repo**, matching `frust-database` §6 exactly, so the previous revision's own
deviation-disclosure paragraph (an in-repo `.tmp/i18n-probe/`) no longer applies and is dropped.
