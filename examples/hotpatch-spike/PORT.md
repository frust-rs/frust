# PORT.md: a frust-owned patch builder

Card p2-03c (tsk_000001a1169831d4W8yKhj8T), plan fplan_000001a10e032a8eKL91PGxC, written 2026-10-07 on
`spike/hotpatch` @ 085509c5. Per Ed's Phase 2 direction this document **assumes** the builder is
frust-owned. It does not argue GO/NO-GO for owning it. It sizes that builder from dx 0.7.10, maps it
onto frust-drive / frust-cli / frust-tui, designs the five Phase-1 requirements, and ends with a
card list a follow-up plan can adopt as written.

**Citation conventions.**

- `dx:` paths are DioxusLabs/dioxus at tag **v0.7.10** (commit 57d6794, shallow clone outside the
  repo), relative to `packages/`. Example: `dx:cli/src/build/link.rs:612`.
- Bare paths are this repository on `spike/hotpatch` @ 085509c5.
- "lines" means lines in the file, comments included. "code" means lines that are neither blank nor
  comment-only (`//`, `///`). Both were counted by a script over the exact ranges cited, not
  estimated.

RESULTS.md holds the measurements this document builds on: rows A-I, D2, and the two Phase 2
sections.

---

## 0. Plan-research claims this document refutes or qualifies

| Claim (source) | Finding |
|---|---|
| Research rsa_000001a10e039348SfXC5tKB sized the builder files at `link.rs 1536, patch.rs 1789, request.rs 3211` lines. | Those counts are from dx **main f951996**. At **v0.7.10**, the tag the runtime and the spike use, they are `link.rs` **1409**, `patch.rs` **1655** and `request.rs` **3135**. `android.rs` 1387 and `apple.rs` 2009 match. *Qualified:* the counts were for another revision. |
| "The dx builder that makes patches is ~3-4k native lines" (plan background). | Measured over the exact hot-patch ranges (section 1): the native builder core is **3,153 lines / 2,023 code**, plus host+app transport (310 lines / 249 code) and platform glue (163 / 139, overlapping). Whole files are much larger because most of `request.rs`/`builder.rs` is bundling. *Confirmed, with the refinement that only ~2.0k lines are code.* |
| "dx 0.7.10 already replays rustc for modified workspace crates" (plan background). Also dx's own comment: "the final tip might include itself as a lib (lib.rs + main.rs) which gets covered here" (`dx:cli/src/build/link.rs:146-147`). | The replay **excludes the tip package by package name** (`dx:cli/src/build/link.rs:616-623`), and the tip's lib target lives in that package. dx's own comment is contradicted by its code and by row H. *Refuted for the tip package's lib.* |
| "Android namespace and W^X rules may refuse dlopen of app-written code" (plan risk register). | p2-02b refutes it for loading: on Android 14 (`untrusted_app`), memfd, plain `dlopen` from `files/` and from `cache/` all load and return 42 (RESULTS.md, "Phase 2: Android load probe"). Relocation against the base library remains unproven. |
| "Changing a State struct's layout under a patch is undefined behaviour or a crash" (plan risk register). | Already split by Phase 1: a type-identity change is a silent no-op (row D); a layout change under the same type path is UB (row D2). PORT.md adds a third case: a `build` **return-type** change is a D2-class hazard by mechanism (requirement (c)). Row D3 is not measured yet. |
| (implicit in every hot-patch design so far) frust-hotpatch's ASLR anchor `dlsym(RTLD_DEFAULT, "main")` (`crates/frust-hotpatch/src/patch.rs:159-164`) works on every target. | On Android the app's code is the lib's **cdylib** (`crates/frust-drive/templates/app/Cargo.toml.tmpl:14-15`), entered through JNI, and it defines no `main`. dx's Android tip is different: a `[[bin]]` linked as `libmain.so` that does contain `main` (`dx:cli/src/build/request.rs:2546-2550`). On frust's Android the anchor cannot be the app library's own `main`, so an app-owned anchor is required (section 2.f). |

---

## 1. Inventory: dx 0.7.10's hot-patch builder

### 1.1 Summary table

| # | Component | Where (dx v0.7.10) | Lines / code (native only) | External deps it uses | frust decision |
|---|---|---|---|---|---|
| 1 | rustc-arg capture (`RUSTC_WORKSPACE_WRAPPER`) | `cli/src/rustcwrapper.rs:1-170` | 170 / 109 | serde, serde_json | **Replace**: same mechanism, re-keyed per crate type (requirement (b)) |
| 1b | Linker interception (no-link / proxy) | `cli/src/cli/link.rs:1-288` (wasm arms excluded) | 285 / 193 | target-lexicon, object (dummy object), anyhow | **Keep** the design; drop target-lexicon (frust-drive already handles triples) |
| 1c | Wrapper env, thin rustc command, arg-set load, scope dirs, fingerprint bust | `cli/src/build/request.rs:1186-1270, 1551-1601, 1634-1646, 1913-1929, 2406-2436, 2569-2612, 3081-3086`; `cli/src/build/link.rs:1351-1408` | 305 / 191 | sha2 (scope hash), cargo_metadata, dunce | **Keep** the logic; replace sha2 with a fixed FNV hash and dunce with `crates/frust-drive/src/host_path.rs` |
| 2 | Fat vs Thin `BuildMode`, fat flags, cache fill | `cli/src/build/request.rs:276-323, 958-961, 1787-1825, 2276-2289` | 105 / 33 | none | **Keep** |
| 2b | Workspace replay (order, args, rlib lookup, dependents, cumulative set, file→crate) | `cli/src/build/link.rs:514-723, 1279-1349`; `cli/src/build/request.rs:2853-2904, 3095-3134`; `cli/src/build/builder.rs:347-456`; `cli/src/serve/runner.rs:1281-1326` | 524 / 367 | krates, tokio | **Replace**: include the tip package's lib (requirement (a)); find rlibs from rustc artifact notifications (requirement (b)); use frust-drive's sync `ProcessRunner` instead of tokio |
| 3 | Thin link (orchestration, per-flavor args, patch naming) | `cli/src/build/link.rs:39-335, 337-512, 725-756` (wasm arm 345-387 excluded) | 459 / 249 | itertools, tokio | **Keep** Darwin + Gnu; MSVC in stage 3 |
| 3b | Fat link (fat archive + force_load/whole-archive, `main` export) | `cli/src/build/link.rs:758-1142` (wasm 948-956 and Swift 1031-1049 excluded) | 357 / 233 | ar 0.9.0, uuid (cache key) | **Keep**; export the frust anchor instead of `main` |
| 3c | Linker flavor + selection | `cli/src/build/link.rs:1186-1277` | 92 / 53 | none | **Keep** |
| 4 | Patch cache, ELF/Mach-O | `cli/src/build/patch.rs:1-113, 263-347` (wasm fields excluded) | 183 / 138 | object 0.37 | **Keep** |
| 4b | Patch cache, PE (PDB) | `cli/src/build/patch.rs:115-189` | 75 / 59 | pdb 0.8.0 | **Keep, stage 3** |
| 4c | Jump table, ELF/Mach-O (+ `create_jump_table`, `main_sentinel`) | `cli/src/build/patch.rs:394-443, 1642-1655`; `cli/src/build/link.rs:1144-1184` (wasm 1163-1181 excluded) | 86 / 60 | object, rayon | **Keep**; drop rayon unless measured necessary |
| 4d | Jump table, PE | `cli/src/build/patch.rs:349-392` | 44 / 39 | pdb | **Keep, stage 3** |
| 4e | Undefined-symbol stub object (all native formats) | `cli/src/build/patch.rs:841-1308` | 468 / 299, of which PE-only arms 105 / 62 (`:969-1004, 1014-1082`) | object, ar | **Keep** (ELF/Mach-O first) |
| 4w | wasm patching | `cli/src/build/patch.rs:190-262, 445-839, 1310-1640` | 799 / 518 | walrus, wasmparser | **Drop** (web is out, section 5) |
| 5 | Transport, host side | `cli/src/serve/server.rs:302-319, 521-545`; `cli/src/serve/runner.rs:748-815, 868-893`; `cli/src/serve/mod.rs:106-122`; `cli/src/build/builder.rs:813-821, 866-901` | 199 / 172 | axum (ws), tokio-tungstenite, tokio | **Replace** with frust-devtools-protocol methods (section 4) |
| 5b | Transport, app side | `devtools/src/lib.rs:57-111` | 55 / 41 | tungstenite, dioxus-cli-config | **Replace** (already bypassed: `examples/hotpatch-spike/runner/src/main.rs:17-50`) |
| 5c | Wire types | `devtools-types/src/lib.rs:1-56` | 56 / 36 | dioxus-core (template type), subsecond-types | **Drop**, except the `JumpTable` shape that frust-hotpatch already mirrors (`crates/frust-hotpatch/src/jump_table.rs:14-33`) |
| 6 | Android hot-patch bits (export-dynamic, linker = NDK clang, adb push/reverse/root) | `cli/src/build/request.rs:767-781, 825-833`; `cli/src/build/android.rs:1188-1202`; `cli/src/build/builder.rs:967-993, 1427-1432, 1456-1465` | 82 / 68 | none beyond adb | **Replace** (section 3.3) |
| 6b | Apple hot-patch bits (sim build version in stub, `-isysroot`, simctl env) | `cli/src/build/patch.rs:897-923`; `cli/src/build/link.rs:405-410, 501-509`; `cli/src/build/builder.rs:1046-1084` | 81 / 71 (overlaps rows 3 and 4e) | none | **Keep** for the simulator (stage 4) |

**Totals (native, transport excluded):** rows 1-4e give **3,153 lines / 2,023 code**. The PE subset
(rows 4b, 4d and the PE stub arms) is **224 / 160** of that. Transport adds **310 / 249**.
Everything `apple.rs` contributes besides the rows above is bundling and code signing
(`dx:cli/src/build/apple.rs:282-365` codesigns the *app*; no patch is signed). The 2009 lines of
`apple.rs` are therefore not part of the port.

### 1.2 rustc-arg capture

- **Mechanism.** Fat and base builds run `cargo rustc` with `RUSTC_WORKSPACE_WRAPPER=<dx>` and
  `DX_RUSTC=<scope dir>` (`dx:cli/src/build/request.rs:1634-1646`). Cargo calls that wrapper only for
  workspace members; dx's comment says so (`:1606-1609`). Each invocation is persisted as JSON
  `{args, envs}` (`dx:cli/src/rustcwrapper.rs:53-70`), then real `rustc` runs (`:81-93`). A link
  step is detected by `.o`/`-flavor` arguments, including inside `@` response files (`:138-170`),
  and handed to `LinkAction` (`:73-77`).
- **Keying (the requirement (b) bug).** The file name is `{crate_name}.{lib|bin}.json`. Only the
  **first** `--crate-type` is read (`dx:cli/src/rustcwrapper.rs:110-115`), and any value other than
  `lib`/`rlib`, which includes `cdylib`, becomes `bin` (`:123-127`).
- **Linker interception.** `-Clinker=<dx>` is passed for fat and thin builds
  (`dx:cli/src/build/request.rs:1745-1753`), and the `DX_LINK*` env vars select `LinkAction`
  (`dx:cli/src/cli/link.rs:51-109`). With no real linker configured (fat/thin), dx writes the linker
  args to a file and emits an empty object, so rustc's post-link steps succeed (`:178-238`). With a
  real linker (Android), dx proxies to it and captures the args (`:149-177`).
- **Scope.** Captures live under `<target>/dx/.captured-args/<tip>-<triple>-<profile>-<hash16>`. The
  hash covers profile, features, rustflags and rustc version
  (`dx:cli/src/build/request.rs:2569-2577`, `dx:cli/src/build/link.rs:1360-1407`). Fat builds bust
  cargo fingerprints for the tip, and for any workspace dep without cached args, to force a fresh
  capture (`dx:cli/src/build/request.rs:1213-1262`).

### 1.3 Fat vs Thin and workspace replay

- `BuildMode::{Base, Fat, Thin{changed_files, aslr_reference, workspace_rustc_args,
  modified_crates, cache}}` (`dx:cli/src/build/request.rs:294-323`). `build()` sends Thin to
  `compile_workspace_hotpatch` and Base/Fat to `cargo_build` (`:958-1011`).
- Fat and Thin both add `-Csave-temps=true -Clink-dead-code` (`:1795-1802`). These are `cargo rustc`
  args, so they reach only the final target (`:1712-1715`).
- Fat linking (`dx:cli/src/build/link.rs:788-1142`) packs every workspace rlib's `.rcgu.o` into one
  `libdeps-<hash>.a` (`:810-936`). It force-loads that archive (Darwin `-force_load` `:966-973`,
  Gnu `--whole-archive` `:957-965`, MSVC `/WHOLEARCHIVE` `:974-983`) and exports `main` as the ASLR
  anchor (`:991-1008`). Then the patch cache is built from the final exe
  (`dx:cli/src/build/request.rs:2276-2289`).
- On a file change, `order_changed_crates` maps each file to the workspace member with the longest
  directory prefix (`dx:cli/src/serve/runner.rs:1281-1326`). `patch_rebuild` adds the tip package
  and the dependents cascade to the **cumulative** `modified_crates` set (cascade stops at the tip,
  `dx:cli/src/build/builder.rs:413-429`).
- The thin build replays every modified crate **except the tip package**
  (`dx:cli/src/build/link.rs:612-662`, exclusion at `:619-623`). It compiles each with its captured
  args, stripping `-Clinker` (`:518-584`), then recompiles the tip bin with plain `rustc`
  (`dx:cli/src/build/link.rs:161`, command at `dx:cli/src/build/request.rs:1571-1601`). Finally it
  links the tip's fresh `.rcgu.o` files, the replayed rlibs and a stub object into
  `lib<exe>-patch-<ms>.{dylib,so,dll}` (`dx:cli/src/build/link.rs:187-317`, naming `:737-756`).
- Replayed rlibs are found from `--out-dir` and `-C extra-filename` (`dx:cli/src/build/link.rs:1285-1318`),
  falling back to a `lib<crate>-*.rlib` glob (`:1320-1348`).
- Quirk worth keeping: dx deletes the fat exe's `deps/` copy after each thin link, because later
  `dlopen`s otherwise fail with "missing symbols that never existed" (`:307-317`).

### 1.4 Stub generation and jump table, per object format

- **Patch cache.** Fat build parses the base binary's symbol table once.
  - ELF/Mach-O: `object` (`dx:cli/src/build/patch.rs:263-290`), plus the TLS init image and
    Mach-O `$tlv$init` sizes (`:292-332`).
  - PE: the `.pdb` public/data symbols (`:120-188`).
- **Stub object** (`create_undefined_symbol_stub`, `dx:cli/src/build/patch.rs:852-1260`). Collects
  symbols that are undefined in the patch inputs and not defined by them (`:858-870`,
  `:1262-1308`). Each one is resolved against the cache and slid by the ASLR offset (`:925-966`):
  - **text** → a jump thunk to the absolute base address. Thunks exist for x86-64, x86, aarch64 and
    arm32 on SysV (`:1083-1120`) and Windows (`:1015-1082`).
  - **TLS** → a fresh TLS symbol seeded with the base's init bytes (`:1163-1227`). Each patch gets
    its own copy, so TLS values reset on patch (`:1180-1181`).
  - **data** → an absolute symbol at the base address (`:1229-1255`).
  - **PE `__imp_`** → a data pointer (`:983-1004`).
  - Mach-O gets a platform build version (macOS / iOS / iOS simulator, `:897-920`). Darwin names
    lose their leading `_` (`:959-964`).
- **Jump table** (`create_native_jump_table`, `dx:cli/src/build/patch.rs:403-443`). Every symbol of
  the patch whose **name** exists in the base maps base address → patch address (`:420-424`). The
  base and patch anchors (`_main` on Apple, `main` elsewhere, `:1646-1655`) become `aslr_reference`
  and `new_base_address`. PE does the same over the PDB (`:349-392`).
- **Consequence for requirement (c):** identity is purely by symbol name. Row D (new name → no
  entry → silent no-op) and row D2 (same name → entry → UB) both follow directly.

### 1.5 ASLR reference handling

- The app reports its runtime `main` address in the websocket URL query
  (`dx:devtools/src/lib.rs:91-96`). The server stores it per connection
  (`dx:cli/src/serve/server.rs:521-536`, `dx:cli/src/serve/runner.rs:868-885`).
- No patch is built until it arrives (`dx:cli/src/build/builder.rs:380-395`).
- The stub hard-codes absolute addresses for that process (`dx:cli/src/build/patch.rs:925-939`). The
  runtime rebases the table by `aslr_reference()` and the patch's own `main`
  (`crates/frust-hotpatch/src/patch.rs:114-135`).
- dx notes the approach is fragile on Android when the app restarts mid-build
  (`dx:cli/src/build/link.rs:216-218`). It also notes a PIC alternative needing no host coordination
  (`:358-363`), which is not implemented for native targets.

### 1.6 Android and Apple specifics (hot-patch bits only)

- **Android.**
  - dx's Android tip is a bin linked as `lib<name>.so` (`dx:cli/src/build/request.rs:2546-2550`),
    linked with `--export-dynamic` so `main` is resolvable (`:767-781`).
  - The linker is the NDK clang `"<triple><api>-clang"` (`:825-833`,
    `dx:cli/src/build/android.rs:1188-1202`).
  - Patches are `adb push`ed to `/data/local/tmp/dx/` (`dx:cli/src/build/builder.rs:967-993`,
    `dx:cli-config/src/lib.rs:312-314`) and the jump table's `lib` path is rewritten to that location
    (`dx:cli/src/build/builder.rs:870-875`).
  - The app connects back through `adb reverse` (`:1456-1465`), after an optional `adb root`
    (`:1427-1432`).
- **Apple.** Only the simulator is patched.
  - The stub carries `PLATFORM_IOSSIMULATOR` (`dx:cli/src/build/patch.rs:909-919`).
  - The thin link forwards `-isysroot` for iOS (`dx:cli/src/build/link.rs:405-410, 501-509`).
  - The simulator app gets the devserver env through `SIMCTL_CHILD_*` (`dx:cli/src/build/builder.rs:1059-1071`).
  - Nothing signs a patch (codesign covers the bundle only, `dx:cli/src/build/apple.rs:282-365`).

### 1.7 Devserver transport

- **Messages.** `DevserverMsg::{HotReload(HotReloadMsg), HotPatchStart, ...}` and
  `HotReloadMsg{templates, assets, ms_elapsed, jump_table, for_build_id, for_pid}`
  (`dx:devtools-types/src/lib.rs:9-50`). The crate imports a `dioxus_core` template type (`:1`).
- **Delivery.** A websocket sent to **all** clients, filtered app-side by pid
  (`dx:cli/src/serve/server.rs:302-319`, `dx:devtools/src/lib.rs:76-86`).
- **No authentication.** "This doesn't use any form of security or protocol, so it's not safe to
  expose to the internet" (`dx:devtools/src/lib.rs:59`).
- **No acknowledgement.** dx never learns whether a patch took effect. That is why row D logs
  "Hot-patching ... took 220ms" for a no-op.

---

## 2. The Phase-1 requirements: cause and design

### (a) Replay the tip package's own lib target

**Cause.**
- `workspace_hotpatch_replay_order` filters out the tip by **package** name
  (`dx:cli/src/build/link.rs:616-623`).
- `patch_rebuild` puts the tip package into `modified_crates` (`dx:cli/src/build/builder.rs:413-414`)
  and stops the cascade at it (`:425`).
- `file_to_workspace_crate` maps both `src/lib.rs` and `src/main.rs` to that same package
  (`dx:cli/src/serve/runner.rs:1305-1325`).
- The one-package template's code therefore never replays: `replaying crates: []` (row H). Only the
  bin is recompiled (`dx:cli/src/build/link.rs:161`).

**Design.**
- The replay unit is a **(package, target)** pair, not a package.
- A changed file maps to the package's **lib** target unless it is the bin's root file (from
  `cargo metadata`'s `targets[].src_path`) or a module only the bin includes.
- Replay order is a topological sort over lib targets (Kahn's algorithm, as at
  `dx:cli/src/build/link.rs:625-661`). It **includes the tip package's lib**.
- The tip **bin** is recompiled only when one of its own files changed. A frust bin `main.rs` is a
  one-line `__frust_main()` call (`crates/frust-drive/templates/app/src/main.rs.tmpl:20-23`), so it
  normally isn't recompiled. That also removes the ~100-190 ms `Compiling` step Phase 2 measured for
  the tip crate (RESULTS.md, "Phase 2: frust-hotpatch runtime").
- The patch's new-base anchor comes from the lib (section 2.f), so a patch needs no bin objects at all.

### (b) Capture rustc args per crate type

**Cause.**
- Capture keys on the first `--crate-type` (`dx:cli/src/rustcwrapper.rs:110-127`). A lib declared
  `["cdylib","staticlib","rlib"]` (`crates/frust-drive/templates/app/Cargo.toml.tmpl:14-15`) is
  stored as `.bin` and then missing as `.lib`: "Missing captured rustc args for workspace crate
  '<x>.lib'" (`dx:cli/src/build/link.rs:681-690`).
- With `rlib` first, cargo emits no `-C extra-filename` for that crate-type set. The exact-path
  lookup (`:1312-1318`) and the `lib<crate>-*.rlib` fallback glob (`:1320-1348`, the hyphen at
  `:1322`) both miss `lib<crate>.rlib`: "No rlib found ... extra-filename=None" (README.md finding 2).

**Design.**
1. The wrapper reads **all** `--crate-type` values of the invocation. It keys the record by target
   kind: `{crate}.bin` iff `bin` is among them, else `{crate}.lib`. The record stores the full list.
2. Replay runs the captured invocation unchanged, with every crate type. Replay output is read from
   rustc's JSON artifact notifications: replay forces `--json=artifacts` if the captured args lack
   it and collects every `{"artifact": ..., "emit": "link"}` path, the same parse dx applies to its
   tip build (`dx:cli/src/build/request.rs:1078-1093`). The rlib to link is the artifact ending in
   `.rlib`, whatever its name. No `extra-filename` assumption remains.
3. A follow-up optimisation would replay `--crate-type rlib` only, skipping the cdylib/staticlib
   links. It needs its own measurement gate, because changing the crate-type set may change what
   rustc writes into the crate metadata. Not in milestone 1.

With (a) and (b), the unmodified `frust create` layout patches. **The template needs no change**
(section 3.4).

### (c) The boundary-layout invariant

**Invariant.** No type whose values were created by code from one image and are read or written
by code from another image may change layout. Layout here means size, alignment, field offsets and
field types, and enum variant layout. This must hold without detection being skipped. At the
frust seam, those types are:

1. the **component type `C`** (props). Its value is created by the parent's `build` and read by the
   patched `build` as `&self.component` through `call_build` (`crates/frust-core/src/component.rs:178`,
   `:223-226`);
2. **`C::State`**, created by the original `init` and stored inline in `ComponentWidget::state`
   (`crates/frust-core/src/component.rs:124`). A patch never re-runs `init`
   (`crates/frust-core/src/hotpatch.rs:15-16`). Row D2 measured new code reading and writing 4 bytes
   past the old 4-byte value, likely into the `disposed` flag (`component.rs:140`);
3. **`build`'s concrete return type `R`**, at two points:
   - **The return slot.** The old `call_build` monomorph receives `F::Return` and erases it as the
     old `R` (`AnyView::new(crate::hotpatch::call_build(..))`, `component.rs:178`, `:223`). The
     patched `call_it` symbol carries no return type (RESULTS.md row D notes), so a new `R` is
     written into the old `R`'s slot and read through the old `R`'s vtable.
   - **The retained tree.** `prev` (`component.rs:127`) holds the previous `R` behind an `AnyView`,
     and the child element holds `R::Element` (`component.rs:130`, recovered at `:232-236`).
     `AnyView::rebuild` downcasts both **by `TypeId`** (`crates/frust-core/src/view.rs:259-277`).
     A type keeps its `TypeId` when a field is added, because `TypeId` is identity, not layout. So
     new code can read an old value of a same-named type through a new layout.

   Not every `build` edit changes `R`. `.child(..)`, `.flex(..)` and `.when(..)` on a `FlexView`
   return `Self` (`crates/frust-widgets/src/flex.rs:183-193`, `:230`), so adding a child keeps
   `R`. Wrapping the root `column()` (`flex.rs:267`) in another container changes `R`.
4. ...and so on **recursively through fields**: closure environments captured in views, and every
   app-defined type reached through an erased child that a later rebuild downcasts back to a
   concrete type.

**Measured status.**
- Row D (identity change) and row D2 (State layout) are measured.
- **Row D3** (return-type edit: wrap the root `column()`) was not added by p2-01b or p2-02b. It is
  the **first follow-up measurement** (card H0-00), run with dx on the existing spike before any
  builder work.

**Why an instance or symbol comparison is insufficient.**
- dx matches by symbol name (`dx:cli/src/build/patch.rs:420-424`).
- The seam symbol is `<<C as Component>::build as HotFunction<(&C, &mut C::State), Fn2Marker>>::call_it`.
  It names `C` and `C::State` by **path** and names neither layout nor `R`.
- A field added to `C`, a field added to a named `C::State` (D2), and a new `R` all leave that
  symbol unchanged. The entry exists, and the patch is taken over the old layout.
- Only a type-**identity** change of `C::State` (row D) changes the symbol, and that produces a
  silent no-op (no entry), not a crash.
- A symbol comparison can therefore detect D, and nothing that is actually unsafe.

**Design: three layers, each covering what the previous cannot.**

- **L1: remove the return slot from the boundary (structural, zero cost).**
  - The hot function becomes `build_erased::<C>(c: &C, s: &mut C::State, expect: &SeamLayout) ->
    Result<AnyView<C::State>, LayoutMismatch>`. It erases **inside** the patched code. The type
    crossing the return slot is then `Result<AnyView<C::State>, LayoutMismatch>`, whose layout
    cannot change: `AnyView` is one `Box<dyn ErasedView>` (`crates/frust-core/src/view.rs:297-299`).
  - `ComponentView` stops erasing the seam result itself. `AnyView::new` is idempotent (`:307-319`),
    so behaviour with the feature off is unchanged.
  - The erasure tripwire's rule T7 ("helpers ending in one erasure call return impl View",
    `scripts/ci/erasure-check.sh:9`) would flag `build_erased`. It carries the documented opt-out
    `// erasure: keep hot-patch boundary type must be layout-fixed`
    (`scripts/codemod/frust_any_codemod.py:121`).

- **L2: call-time guard in the app (no host data needed).**
  - `SeamLayout { size_c, align_c, size_s, align_s }` is computed by the **caller**, which is old
    code, from `size_of`/`align_of`. The callee, which is new code, compares it with its own
    constants **before touching `state`**.
  - On mismatch the callee returns `Err(LayoutMismatch)` without running `build`. The old caller
    then runs the original `<C as Component>::build` directly (nothing rewrites memory; only `HotFn`
    calls reach new code, `crates/frust-hotpatch/src/lib.rs:6-11`) and records the mismatch for the
    patch outcome.
  - This catches the measured D2 case on every target, Android included, with no DWARF.
  - It cannot see same-size changes, field reorders, `R` internals or closure environments.

- **L3: host-side structural gate (sound for app-defined types).**
  - **Computed at build time.** At the **fat** build, before any replay overwrites an rlib in place
    (dx replays "at the same paths cargo originally wrote to", `dx:cli/src/build/link.rs:514-517`),
    the builder reads the DWARF of every replayable target's objects. It records one entry per type
    whose DWARF path lies in a replayable crate, and per instantiation of an external generic over
    such a type. The entry holds `(byte_size, alignment, [(member name, offset, member type path)],
    variant parts)`, hashed recursively. The table is saved as `layout-base.json` in the session dir.
    Closure environments appear in DWARF as `{closure_env#N}` types under their function's path, so
    they are covered.
  - **Checked at apply time.** Every thin build extracts the same table from the replayed objects
    and compares it before anything is sent. Any type present in both with a different hash means
    `RestartRequired { reason: LayoutChanged { type_path, old_size, new_size } }`. The patch is never
    offered.
  - **Scope.** This is deliberately a superset of the types reachable from `C`/`C::State`/`R`.
    Reachability through `Box<dyn ...>` children is invisible in DWARF (a trait object member is a
    data/vtable pair), while `TypeId` downcasts re-enter those types on rebuild. The cost is false
    positives: editing a type that has no live values also restarts. That is accepted. Being sound
    matters more than avoiding an occasional restart.
  - **Debuginfo.** The template's dev profile is cargo's default, `debug = true`; nothing in
    `crates/frust-drive/templates/app/Cargo.toml.tmpl` overrides it for dev. Card H1-05 must
    verify, on each host, that replayed rlib members carry DWARF under the default
    `split-debuginfo` (on macOS the DWARF stays in the `.o` files, which is where the gate reads
    it).

- **Row D (identity change) detection, host side.**
  - The builder demangles the base's and the patch's `call_it` instances of `build_erased`
    (`rustc-demangle`, v0 symbols, which rustc 1.98.1 emits per RESULTS.md row D notes).
  - If the patch has an instance for `C` whose argument tuple differs from the base's instance for
    the same `C`, `C::State`'s identity changed. The builder answers `RestartRequired {
    StateTypeChanged { component } }` instead of shipping a patch that cannot take effect.
  - In-app, the post-patch **fall-through** count (`crates/frust-hotpatch/src/hot_fn.rs:97-101`)
    stays as the backstop.

- **Forced restart, with a message.**
  - Every `RestartRequired` is shown verbatim, e.g. `restart required: HomeState changed layout (4 →
    8 bytes); restarting to keep memory safe`.
  - The session then runs the existing restart path: CLI kill + relaunch (`crates/frust-cli/src/commands/run.rs:585-589`),
    TUI `restart_session_at` (`crates/frust-tui/src/engine/update.rs:2657-2670`). The app's State
    resets, as it does today.
  - **Never** `applied` (requirement (d)).

### (d) Framework / path-dependency edits

**Cause.**
- dx watches the tip's direct dependencies' directories (`dx:cli/src/serve/runner.rs:1059-1142`),
  which do not include transitive frust crates such as `frust-widgets`.
- dx skips any `.rs` file outside the workspace root ("Skipping file outside workspace dir",
  `dx:cli/src/serve/runner.rs:416-419`).
- dx maps files only to workspace members (`:1308`) and replays only members
  (`dx:cli/src/build/request.rs:2861-2904`).
- Captures cover members only, because `RUSTC_WORKSPACE_WRAPPER` wraps only members
  (`dx:cli/src/build/request.rs:1606-1609`).
- Row E saw no log line at all, and a later app patch mixed new app code with the stale
  `frust-widgets` rlib.

**Design (milestone 1: explicit restart).**
1. The session classifies every watched path from `cargo metadata`:
   - **replayable**: a lib or bin target of a workspace member;
   - **local non-member**: any package with `source == null` outside the workspace, i.e. a path
     dependency such as a `--frust-path` checkout or `frust-material` by path
     (`crates/frust-drive/templates/app/Cargo.toml.tmpl:24-28`);
   - **build input**: any `Cargo.toml`, `build.rs`, `.cargo/config.toml`, or a non-`.rs` file listed
     in the target's dep-info (dx's rule, `dx:cli/src/serve/runner.rs:493-501`).

   It watches all three classes, unlike today's `<root>/src` + `Cargo.toml`
   (`crates/frust-cli/src/commands/run.rs:489-500`, `crates/frust-tui/src/supervise/watch.rs:171-182`).
2. Any change outside the replayable class means `RestartRequired { PathDependencyChanged { package,
   file } }` or `BuildInputChanged`. A **fat** rebuild follows, which re-captures, re-links and
   relaunches. No thin build is attempted.
3. **No success line without an effect.**
   - The app's outcome reply (section 4) carries `seam_hits` (a new frust-hotpatch counter) and
     `seam_fall_throughs`.
   - The CLI/TUI print `patched in N ms (k components rebuilt)` only if `seam_hits > 0`, no L2
     mismatch was recorded, and no fall-through occurred on a seam instance the builder reported as
     present in the patch.
   - Otherwise they print `restart required`.
   - The desktop shell's unconditional `log::info!("frust-hotpatch: applied ...")`
     (`crates/frust-shell-desktop/src/app_handler.rs:2559-2560`) becomes a `debug!`-level probe and
     is never the success signal (act_000001a11662d3d7REewEc83).

**Deferred (optional card H5-01, after milestone 1 data).** Replaying path dependencies needs
three things:
- capture for non-members, through `RUSTC_WRAPPER` filtered to local packages;
- the cascade through every dependent up to the tip;
- the L3 layout gate extended to framework crates, since framework types such as `FlexView` sit in
  every retained tree.

Estimated +400 code lines. It only helps people editing frust itself.

### (e) Per-patch memory growth

**Cause.**
- Row G: each patch is a 1.5 MB image (`libhotpatch-spike-patch-<ms>.dylib`, 1,534,184 bytes). RSS
  grows ~0.55 MB per patch (+5.4 MB over 10).
- Images are never unloaded, by design (`crates/frust-hotpatch/src/patch.rs:110-112`).
- They cannot be unloaded: vtables of every `AnyView` and element built by patched code live in
  that image and stay reachable from the retained tree.
- Patches also grow over a session, because the set is cumulative: every patch relinks every crate
  modified since the fat build (`dx:cli/src/build/builder.rs:403-429`).

**Design.**
1. **Retention policy: never unload.** Unloading would need proof that no vtable, function pointer
   or closure from the image is still reachable, which frust cannot establish.
2. **Bound: a patch budget.**
   - The session keeps `patches_applied` and `patch_bytes_loaded`.
   - Default budget is **64 patches or 96 MiB of patch images, whichever comes first**. At the
     measured rate, 64 patches ≈ **35 MB** RSS growth (0.55 MB × 64), which keeps a long session
     under +40 MB.
   - When the next patch would exceed the budget, the session answers `RestartRequired {
     PatchBudget { patches, bytes } }` and relaunches. The budget is configurable per project (a
     `frust.toml` `[hotpatch]` key, card H1-08).
3. **Disk.**
   - Host patch files go to `<target>/frust-hotpatch/<session>/patch-<n>.<ext>`. That keeps them
     under `build/rust` by default (`crates/frust-drive/templates/app/.cargo/config.toml:4-5`) and
     out of the source tree. The whole dir is deleted at session start.
   - On Android the app deletes its cache copy right after the memfd copy, so only the memfd keeps
     the bytes.
4. **Reporting.** Each outcome reply includes `patches_applied` and `patch_bytes_loaded`, so the
   budget is visible before it is reached.

### (f) Cross-cutting: an app-owned ASLR anchor (from section 0)

- `frust::app!` emits, under `cfg(all(debug_assertions, feature = "hotpatch"))` in the app crate,
  `#[unsafe(no_mangle)] pub extern "C" fn __frust_hotpatch_anchor() {}`. It registers its address
  at startup with `frust_hotpatch::set_anchor(__frust_hotpatch_anchor as usize)`. That takes a
  function pointer, so no `dlsym` is needed.
- The builder takes the anchor's link-time address from the base image's symbol table.
- The thin link exports the same symbol from the patch, so `apply_patch` resolves the patch's anchor
  with `dlsym(patch, ...)` in place of `main` (today `crates/frust-hotpatch/src/patch.rs:118`).
- This works for an executable (desktop, simulator) and for the Android cdylib alike. It replaces
  dx's `main` export flags (`dx:cli/src/build/link.rs:991-1008`, `:479`).

---

## 3. Mapping onto frust

### 3.1 Where the builder lives

| Piece | Home | Why there |
|---|---|---|
| Capture, graph, replay, fat/thin link, symbols, stub, layout gate, session | `crates/frust-drive/src/hotpatch/` (new module) | frust-drive owns every build/run path the CLI, TUI, MCP and DAP share (`crates/frust-drive/src/desktop_run.rs:1-16`). It already spawns processes through `ProcessRunner` and speaks devtools (`crates/frust-drive/src/devtools_client.rs:170-173`). It depends on no framework crate (`crates/frust-devtools-protocol/src/lib.rs:4-11`), and nothing in the builder needs one. |
| `RUSTC_WORKSPACE_WRAPPER` / linker entry point | the `frust` binary itself, dispatched in `main` **before** `Cli::parse()` (`crates/frust-cli/src/main.rs:28-29`) when `FRUST_HOTPATCH_CAPTURE` is set | Same trick as dx (`dx:cli/src/rustcwrapper.rs:46-48`). `std::env::current_exe()` gives the wrapper path. |
| Wire methods | `crates/frust-devtools-protocol` | Section 4 |
| In-app apply | `crates/frust-shell-common/src/devtools.rs` (`ShellBackend`, `:373`, `:397`) calling a **safe** frust-hotpatch entry | frust-devtools "depends on no framework crate" (`crates/frust-shell-common/Cargo.toml:35-36`), and shell-common must stay `unsafe`-free (`docs/CODE_STANDARDS.md:14-17`). So the one `unsafe` call lives in frust-hotpatch, which needs a sanctioned-zone register entry (doc card). |

### 3.2 Desktop: fat build and linker interception

- **Today.** `desktop_plan` builds `cargo run [profile] --features frust/perf-trace --features
  frust/devtools` (`crates/frust-drive/src/desktop_run.rs:60-90`, features from
  `crates/frust-drive/src/build_info.rs:124-126`). `spawn_desktop_session` spawns it
  (`desktop_run.rs:123-138`).
- **Hot session** (`hotpatch::session::start_desktop`), debug mode only:
  1. Add `--features frust/hotpatch` (`crates/frust/Cargo.toml:109`) to the mode's features.
  2. Run `cargo rustc --bin <bin> --message-format json-diagnostic-rendered-ansi <features> --
     -Csave-temps=true -Clink-dead-code -Clinker=<frust>` with `RUSTC_WORKSPACE_WRAPPER=<frust>`,
     `FRUST_HOTPATCH_CAPTURE=<scope dir>` and the link env. These are dx's fat arguments
     (`dx:cli/src/build/request.rs:1627-1646, 1745-1753, 1795-1802`). The scope dir is keyed like
     dx's (`dx:cli/src/build/link.rs:1360-1407`).
  3. Fat-link (`hotpatch::fat_link`), write `layout-base.json` (L3), build the symbol cache.
  4. Spawn the **fat exe directly**, not `cargo run`, through `ProcessRunner::spawn_streaming`. The
     process group and Ctrl-C handling stay as today (`crates/frust-cli/src/commands/run.rs:444-456`).
  5. Read the devtools discovery line from the child's output and connect with the token
     (`crates/frust-drive/src/devtools_client.rs:159-173`). Call `hotpatch_info` to get the runtime
     anchor address. A missing `HotPatch` capability means a cold-restart-only session, with a
     message.
- **Output layout.** The fat exe and its captures live under the cargo target dir (`build/rust` by
  default). frust's `cargo run` and the hot session then share one `target/` instead of the two
  copies RESULTS.md row I measured for dx (`target/dx/...` + `target/debug`, 2.1 GiB). Profile and
  triple stay as `cargo run` uses them. The fat flags change fingerprints only for the tip target.
  Measured in card H1-11.

### 3.3 Android: cargo-ndk + Gradle

- **Today.** The Rust `.so` is built inside Gradle by the template's `cargoNdkBuild` Exec task
  (`crates/frust-drive/templates/app/android.tmpl/app/build.gradle.kts.tmpl:330-357`), driven by
  `gradlew assemble<Flavor><Mode>` (`crates/frust-drive/src/android_run/gradle.rs:1-4`) with
  base64-encoded feature/define properties (`crates/frust-drive/src/android_build/tasks.rs:56-123`).
- **Hot session** (stage 2): frust-drive runs the fat build **outside** Gradle, then skips Gradle's
  own Rust step. **No template change**:
  1. Run `cargo ndk -t arm64-v8a -o <build/android/jniLibs> rustc --lib --features ... --
     -Csave-temps=true -Clink-dead-code` with the capture env. The cdylib's link is intercepted in
     **proxy** mode, the real linker being NDK clang (as `dx:cli/src/cli/link.rs:149-177` with
     `dx:cli/src/build/android.rs:1188-1202`). The builder then re-links it fat, exporting
     `__frust_hotpatch_anchor`.
  2. `gradlew assembleDebug -x cargoNdkBuild` (Gradle's standard task-exclusion flag), then install
     and launch as today.
  3. The symbol cache reads the **unstripped** cdylib from the cargo target dir, never the APK copy.
  4. Connect via `adb forward` (`crates/frust-drive/src/devtools_client.rs:675-700`), which the TUI
     bridge already uses (`crates/frust-tui/src/supervise/devtools_bridge.rs:33-36`).

  Risk: `cargo ndk ... rustc --lib -- <flags>` passthrough is unverified. Card H2-01's first step
  verifies it, falling back to `RUSTFLAGS` scoped by `CARGO_TARGET_<TRIPLE>_RUSTFLAGS`.

### 3.4 What `frust create`'s one-package template needs: **nothing**

With (a) and (b) the builder patches the template's `[lib]` target, with crate types
`["cdylib","staticlib","rlib"]` (`crates/frust-drive/templates/app/Cargo.toml.tmpl:14-15`), in place:
- `main.rs.tmpl` stays "never hand-edit" (`crates/frust-drive/templates/app/src/main.rs.tmpl:1-6`);
- the anchor comes from `frust::app!` (section 2.f);
- Android skips Gradle's step with `-x` (3.3);
- the iOS link is steered by `xcodebuild` build settings on the command line (section 5).

One framework change is needed, not a template change. The desktop root closure that `frust::app!`
generates calls the **root** component's `build` directly (`crates/frust/src/lib.rs:2296`, `:2311`,
`:2334`), so root edits do not patch (README.md finding 4). Card H0-02 routes those three sites
through the same seam.

### 3.5 `frust run --watch` and the TUI's Watch / R

**`frust run --watch`.** Today `run_desktop_watch` (`crates/frust-cli/src/commands/run.rs:430-468`)
debounces (300 ms, `:419`) and kills + relaunches `cargo run` (`:585-589`). In hot mode, the default
for a debug desktop session with `--no-hot` to opt out:
- the watcher feeds the debounced change set into `session.on_change(paths)`;
- `Patched{ms, components}` prints one line;
- `RestartRequired{reason}` prints the reason, then takes the existing kill + relaunch path, as a
  fresh fat session;
- a compile error prints diagnostics and keeps the running app untouched (today's "failed;
  watching" rule, `:568-571`).

The desktop-only rejection of `--watch -d` (`:63-72`) is lifted for Android in stage 2 only.

**TUI.**
- `W` toggles "Watch: restart on save" (`crates/frust-tui/src/runner.rs:1716`;
  `crates/frust-tui/src/engine/message.rs:416-424`). Its trigger `WatchTriggered` restarts through
  `restart_session_at` (`crates/frust-tui/src/engine/update.rs:439`, `:2663-2670`).
- In a hot desktop session `WatchTriggered` becomes an `Effect::HotPatch(session)`. The runner calls
  `session.on_change` off the UI thread and posts a new `Message::HotPatchOutcome{session, outcome}`.
- The update shows a toast with `patched in N ms` or `restart required: <reason>`. For
  `RestartRequired` it falls through to the same `restart_session_at` path.
- `R` (`crates/frust-tui/src/runner.rs:1710`) stays a **full** restart. It is the user's "reset
  state" key, and it must also re-fat the build.
- The palette row is renamed "Watch: hot patch on save" (`crates/frust-tui/src/engine/palette.rs:179`).
- The TUI's desktop launch plan (`crates/frust-tui/src/supervise/session.rs:86-87`) switches to the
  hot session start for watched desktop sessions.

### 3.6 Restart-required signal flow

```
host gates (path dep / build input / L3 layout / D identity / budget)
        ──► session.on_change → Outcome::RestartRequired(reason)        (no app round-trip)
apply_patch reply (L2 mismatch / fall-through / seam_hits == 0)
        ──► devtools response PatchOutcome → Outcome::RestartRequired(reason)
Outcome ──► CLI: println + kill/relaunch (run.rs:585-589)
        ──► TUI: Message::HotPatchOutcome → toast + restart_session_at (update.rs:2663)
```

### 3.7 Debug-only gating and authentication

- **Debug only.**
  - `frust/hotpatch` is added only for `BuildMode::Debug` hot sessions. Profile inherits release
    (`crates/frust-drive/templates/app/Cargo.toml.tmpl:95-98`), so it has no `debug_assertions`, and
    frust-hotpatch consults its table only under `cfg!(debug_assertions)`
    (`crates/frust-hotpatch/src/hot_fn.rs:159-161`).
  - The `HotPatch` capability and both methods are compiled only under `cfg(all(debug_assertions,
    feature = "hotpatch"))`. A release build has neither the devtools listener (`devtools` is
    Debug/Profile only, `crates/frust-drive/src/build_info.rs:124-126`) nor the methods.
  - `frust_hotpatch::apply_patch` itself gains the `debug_assertions` gate that its README lists as
    follow-up work (`crates/frust-hotpatch/README.md`, "Debug-only, like subsecond").
  - The spike runner's `cfg(all(debug_assertions, not(target_os = "ios")))` connection
    (`examples/hotpatch-spike/runner/src/main.rs:16`) is retired: there is no runner. The in-app
    devtools service is the only entry point.
- **Authenticated.**
  - Both methods ride the existing per-process token handshake. Unauthenticated connections get
    `UNAUTHORIZED` for every method but `handshake`
    (`crates/frust-devtools-protocol/src/lib.rs:20-28`; enforced in `crates/frust-devtools/src/dispatch.rs:147`).
  - The listener binds `127.0.0.1` only (`crates/frust-devtools/src/lib.rs:37-49`).
  - The app loads **only bytes it received on that connection and wrote into its own data dir**,
    never a path taken from the wire. A local process that guesses a path therefore cannot get code
    loaded.

---

## 4. Transport

**Recommendation: two new frust-devtools-protocol methods plus a capability. Do not reuse dx's
websocket.**

### 4.1 The methods

| Method | Direction | Params | Result |
|---|---|---|---|
| `hotpatch_info` | client→server request | none | `{ anchor_runtime: u64, pid: u32, triple: String, patches_applied: u32, patch_bytes_loaded: u64 }` |
| `patch_chunk` | client→server request | `{ patch_id: u64, offset: u64, total_len: u64, data_base64: String }` (≤ 512 KiB raw per chunk) | `AckResult` |
| `apply_patch` | client→server request | `{ patch_id: u64, len: u64, table: JumpTableWire, expected_seams: u32 }`, where `JumpTableWire` mirrors `crates/frust-hotpatch/src/jump_table.rs:15-33` minus `lib` | `PatchOutcome { applied: bool, seam_hits: u64, seam_fall_throughs: u64, layout_mismatches: Vec<String>, patches_applied: u32, patch_bytes_loaded: u64 }`, sent **after the next frame**. The backend hops to the UI thread the way existing calls do (`crates/frust-devtools/src/lib.rs:58-74`). |

- **New pieces.** `Capability::HotPatch` joins the handshake's capability list
  (`crates/frust-devtools-protocol/src/messages.rs:44-56`). The three methods join `Method`
  (`crates/frust-devtools-protocol/src/method.rs:14-42`).
- **No version bump.** `PROTOCOL_VERSION` stays 1: the change is additive, and unknown methods are
  `METHOD_NOT_FOUND` by design (`method.rs:61-65`).
- **Why chunking.** Both ends cap a line at 1 MiB (`crates/frust-devtools/src/server.rs:20`,
  `crates/frust-drive/src/devtools_client.rs:414`). A 1.5 MB patch is ~2 MB as base64, so it travels
  in three chunks.
- **base64.** The protocol crate stays a serde-only leaf (`crates/frust-devtools-protocol/src/lib.rs:9-11`)
  and carries a `String`, like `ScreenshotResult.png_base64` (`messages.rs:181-182`). The app side
  decodes with the existing `base64` 0.22 pin (`Cargo.toml:182`, CLI-owned row,
  `docs/DEVELOPMENT.md:406`), added as an optional dependency of frust-shell-common under its
  `hotpatch` feature. No new pin; the row gains a consumer, which is a doc card.

### 4.2 Why not dx's websocket wire

1. **No authentication** (`dx:devtools/src/lib.rs:59`). frust's devtools trust model requires a
   token precisely because loopback is reachable by every co-resident app on a device
   (`crates/frust-devtools/src/lib.rs:39-49`). A patch is arbitrary code execution.
2. **Wrong direction for Android.** dx's app dials out and needs `adb reverse`
   (`dx:cli/src/build/builder.rs:1456-1465`). frust's tooling already dials in through `adb forward`
   (`crates/frust-drive/src/devtools_client.rs:675-700`) and already holds a connection in the TUI.
3. **Dependencies.** dx's types pull `dioxus-core` (`dx:devtools-types/src/lib.rs:1`) and
   tungstenite (`dx:devtools/Cargo.toml`). The runner's `connect(callback)` monomorphisation added
   ~80 ms to every thin build (RESULTS.md, "Phase 2: frust-hotpatch runtime"). The devtools service
   is already compiled into debug builds (`crates/frust-shell-desktop/src/app_handler.rs:929-945`).
4. **No acknowledgement.** dx fires and forgets (`dx:cli/src/serve/server.rs:302-319`). Requirement
   (d) needs a reply after the next frame.

**Kept from dx:** the `JumpTable` serde shape. frust-hotpatch's `wire_compat_with_subsecond_types`
test (`crates/frust-hotpatch/src/jump_table.rs:102-123`) stays while dx 0.7.10 remains a usable
fallback driver. A later card can drop it once the frust builder is the only driver.

### 4.3 Android delivery and load path, from p2-02b

The bytes travel over the devtools connection (`patch_chunk`). The app writes
`<cacheDir>/frust-hotpatch/patch-<id>.so` with mode `0600`, then calls
`frust_hotpatch::load_patch_library` (memfd + `android_dlopen_ext`) and deletes the file.

Evidence (RESULTS.md, "Phase 2: Android load probe"):
- memfd, files-dir `dlopen` and cache-dir `dlopen` all loaded and returned 42 on a Pixel 5 /
  Android 14 in `untrusted_app`, with no `avc` denial;
- control 1: a `0600` file the app owns loads;
- the negative control shows load failures are visible.

This replaces both dx's `/data/local/tmp/dx/` push (`dx:cli/src/build/builder.rs:967-993`), which
the probe did **not** test, and the probe's `run-as cp`, which needs a debuggable package and an adb
round trip per patch.

**Not proven:** relocation of a patch against the base library on Android. The probe's library was
self-contained. That is stage 2's first gate.

---

## 5. Per-target scope

"Extra lines" is new frust code on top of the shared desktop core (section 6), tests excluded.

| Target | In/out | Stage | Reason | Extra lines |
|---|---|---|---|---|
| macOS (aarch64/x86_64-apple-darwin) | **in** | 1 (milestone 1) | Phase 1 and Phase 2 measured here: 475-480 ms through subsecond, 575 ms through frust-hotpatch, vs 1570 ms restart. Mach-O + ld64 via `cc` (`dx:cli/src/build/link.rs:1252`). | 0 (it is the core) |
| Linux (x86_64/aarch64-unknown-linux-gnu) | **in** | 1 | ELF + `cc`/lld. dx's Gnu flavor is the same path as Android's (`dx:cli/src/build/link.rs:428-464`). The CI canary runs here (ubuntu runners, `.github/workflows/ci.yml:67-97`). | ~60 (Gnu thin/fat args, `--export-dynamic-symbol` anchor), already inside H1-03's 500 |
| Android arm64 (aarch64-linux-android) | **in** | 2 | Loading app-delivered code is proven (p2-02b). Needs: the fat build outside Gradle (3.3), the cdylib anchor (2.f), chunked delivery (4.3), and relocation still unproven. `frust-hotpatch` already cross-builds with cargo-ndk for arm64-v8a. 32-bit and x86_64 Android were not probed. | 450 |
| Windows (x86_64-pc-windows-msvc) | **in** | 3 | PE/PDB stubs, `__imp_` data and `lld-link` args exist in dx (224 lines / 160 code, section 1.1). The L3 gate needs PDB type records, not DWARF; until then Windows runs L2 only, as a documented limitation. Gate rig is the Dell. | 400 |
| iOS simulator (aarch64-apple-ios-sim) | **in, behind a probe** | 4 | `frust-hotpatch` builds and is clippy-clean for `aarch64-apple-ios-sim` (conductor check at 593d54d1). Loading on the simulator is **not probed**. Xcode links the Rust staticlib (`crates/frust-drive/templates/app/ios.tmpl/Runner.xcodeproj/project.pbxproj.tmpl:147`, `-l<name>` at `:247`). The fat link must be steered with command-line build settings (`OTHER_LDFLAGS=-Wl,-force_load,...`, `DEAD_CODE_STRIPPING=NO`, anchor export), and the symbol cache read from the Xcode-built executable. | 350 |
| iOS device | **out** | – | Code signing: a patch is an unsigned image that a device will not map executable. dx does not try either: no patch signing anywhere (`dx:cli/src/build/apple.rs:282-365` signs the bundle only), and Apple is simulator-only in the stub (`dx:cli/src/build/patch.rs:909-919`). | – |
| web (wasm32-unknown-unknown) | **out** | – | `frust-hotpatch` is native-only by `compile_error!` (`crates/frust-hotpatch/src/lib.rs:16-17`). wasm patching is a separate 799-line mechanism in dx (walrus/wasmparser, section 1.1 row 4w). | – |

---

## 6. Estimate and card list

### 6.1 New lines per component

| Component | Code | Tests | Card |
|---|---|---|---|
| frust-hotpatch: hit counter, anchor, `debug_assertions` gate on apply, safe devtools-facing entry, mismatch report | 150 | 100 | H0-01 |
| frust-core seam v2 (`build_erased` + `SeamLayout` L2 guard) + root build through the seam in `frust::app!` + anchor emission | 120 | 120 | H0-02 |
| frust-shell-desktop: `#[non_exhaustive]` `ShellUserEvent`, probe demoted | 40 | 30 | H0-03 |
| CI feature-on gate (script + workflow step) | 40 | – | H0-04 |
| frust-drive `hotpatch::capture` + linker interception + `frust` wrapper entry | 450 | 200 | H1-01 |
| `hotpatch::graph` + `hotpatch::replay` (tip lib, per-target, artifact notifications) | 450 | 250 | H1-02 |
| `hotpatch::fat_link` + `hotpatch::thin_link` (Darwin, Gnu) | 500 | 200 | H1-03 |
| `hotpatch::symbols` + `stub` + `jump_table` (Mach-O, ELF; aarch64, x86_64) | 550 | 250 | H1-04 |
| `hotpatch::layout` (DWARF L3) + `hotpatch::seams` (row D identity) | 450 | 300 | H1-05 |
| frust-devtools-protocol methods + capability | 150 | 120 | H1-06 |
| In-app apply: frust-devtools dispatch + frust-shell-common backend + desktop wiring | 300 | 200 | H1-07 |
| `hotpatch::session` + devtools client methods + budget | 500 | 300 | H1-08 |
| frust-cli `--watch` hot mode | 200 | 150 | H1-09 |
| frust-tui Watch hot path | 250 | 200 | H1-10 |
| CI canary (script + standalone fixture) | 200 | – | H1-12 |
| **Stage 1 subtotal (macOS + Linux)** | **4,350** | **2,420** | |
| Android (H2-01..03) | 450 | 200 | stage 2 |
| Windows (H3-01..02) | 400 | 200 | stage 3 |
| iOS simulator (H4-02) | 350 | 150 | stage 4 |
| **All stages** | **5,550** | **2,970** | **≈ 8,500 lines** |

**Why stage 1 is ~2.1× dx's 2,023 code lines.** The L3 layout gate (450) has no dx counterpart.
Integration into existing CLI/TUI/devtools paths (session, protocol, in-app apply, CLI, TUI ≈
1,400) replaces dx's 249-line transport and its TUI. The tip-lib replay and per-crate-type capture
add ~150.

### 6.2 Phase and card breakdown (adoptable as written)

Notation:
- **Complexity** uses Zabin's `simple` / `medium` / `complex`.
- **Doc cards** are `documentation-maintainer` cards; they edit core docs, which implementors may not.
- Every implementor card also runs the standard gate `cargo test --workspace && cargo clippy
  --workspace --all-targets -- -D warnings && cargo fmt --check` (CLAUDE.md). The verification
  column lists only the card-specific commands.
- Lockfile edits are concentrated in card H1-00 so that no two parallel cards touch `Cargo.lock`.

#### Phase H0: measurement, seam hardening, hygiene (no builder yet)

| ID | Title | Write scope | Cx | Deps | Verification |
|---|---|---|---|---|---|
| H0-00 | Row D3: measure a return-type-changing `build` edit (wrap the root `column()` in another container) with dx 0.7.10 on the two-package spike; record whether it no-ops, corrupts or crashes, with a guard-malloc leg | `examples/hotpatch-spike/measure.sh`, `examples/hotpatch-spike/RESULTS.md` | medium | – | `bash -n examples/hotpatch-spike/measure.sh`; `PRE_RUN_PAUSE=20 POST_RUN_PAUSE=15 ./measure.sh --target return-type --runs 3` (GUI session); spike gate from `examples/hotpatch-spike`: `cargo build && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check` |
| H0-01 | frust-hotpatch: seam hit counter, app-owned ASLR anchor (`set_anchor`; patch anchor by name), `debug_assertions` gate on `apply_patch`, safe `apply_from_devtools(bytes_path, table)` entry, layout-mismatch recording | `crates/frust-hotpatch/src/{lib.rs,patch.rs,hot_fn.rs,anchor.rs}`, `crates/frust-hotpatch/README.md` | medium | – | `cargo test -p frust-hotpatch`; `cargo clippy -p frust-hotpatch --all-targets -- -D warnings`; `cargo clippy -p frust-hotpatch --target aarch64-apple-ios-sim -- -D warnings`; `cargo ndk -t arm64-v8a build -p frust-hotpatch` |
| H0-02 | frust-core seam v2: `build_erased::<C>` erasing inside the hot fn (`// erasure: keep`), `SeamLayout` L2 guard with direct-call fallback, `ComponentView` sites updated; `frust::app!` routes the root build through the seam and emits/registers `__frust_hotpatch_anchor` | `crates/frust-core/src/{hotpatch.rs,component.rs,lib.rs}`, `crates/frust/src/lib.rs` | medium | H0-01 | `cargo test -p frust-core --features hotpatch`; `cargo test -p frust-ui --features hotpatch`; `cargo clippy -p frust-core -p frust-ui --features hotpatch --all-targets -- -D warnings`; `scripts/ci/erasure-check.sh` |
| H0-03 | `ShellUserEvent` `#[non_exhaustive]` (act_000001a11662c289GdGV5Rpy); `HotPatched` probe demoted to `debug!`, no success wording | `crates/frust-shell-desktop/src/app_handler.rs` | simple | H0-02 | `cargo test -p frust-shell-desktop`; `cargo test -p frust-shell-desktop --features hotpatch`; `cargo clippy -p frust-shell-desktop --features hotpatch --all-targets -- -D warnings` |
| H0-04 | CI gate compiling and testing the feature-on seam (act_000001a11662c5659hJI8KwH, act_000001a11662d3d4Z8iSAha4, act_000001a11662dc287JopeXfV) | `scripts/ci/hotpatch-feature-gate.sh`, `.github/workflows/ci.yml` | simple | H0-03 | `bash scripts/ci/hotpatch-feature-gate.sh` (runs `cargo test`/`clippy -D warnings` for `-p frust-hotpatch`, `-p frust-core --features hotpatch`, `-p frust-shell-desktop --features hotpatch`, `-p frust-ui --features hotpatch`); `bash -n` on the script |
| H0-05 | Publishability of the optional dependency: frust-hotpatch becomes publishable (`publish` removed; `version.workspace`; license files), so `cargo publish -p frust-core` no longer fails on an unpublished optional dep. First crates.io upload stays a hand step (`.github/workflows/release.yml:4`). | `crates/frust-hotpatch/Cargo.toml`, `crates/frust-hotpatch/LICENSE-MIT`, `crates/frust-hotpatch/LICENSE-APACHE` | simple | H0-01 | `cargo publish -p frust-hotpatch --dry-run --locked`; `cargo publish --workspace --locked --dry-run` (the release job's own check, `.github/workflows/release.yml:67-68`) |

#### Phase H1: the desktop builder (macOS + Linux), milestone 1

| ID | Title | Write scope | Cx | Deps | Verification |
|---|---|---|---|---|---|
| H1-00 | Builder pins: `object` 0.37.x (dx's minor, `read`+`write`, `elf`+`macho`+`coff` only), `ar` 0.9.0, `gimli`, `rustc-demangle`; Version-Pin rows handed to D-03 | `Cargo.toml`, `crates/frust-drive/Cargo.toml`, `Cargo.lock` | simple | – | `cargo generate-lockfile`; `cargo build --workspace --locked`; `cargo tree -d` (no new duplicates) |
| H1-01 | `hotpatch::capture`: `frust` as `RUSTC_WORKSPACE_WRAPPER` (dispatch before `Cli::parse`), records keyed `{crate}.{lib\|bin}` with all crate types, linker interception (no-link and proxy), scope dir + fingerprint bust | `crates/frust-drive/src/hotpatch/{mod.rs,capture.rs,link_intercept.rs}`, `crates/frust-drive/src/lib.rs`, `crates/frust-cli/src/main.rs` | complex | H1-00 | `cargo test -p frust-drive hotpatch::capture`; `cargo test -p frust-drive hotpatch::link_intercept`; `cargo test -p frust-cli` |
| H1-02 | `hotpatch::graph` + `hotpatch::replay`: `cargo metadata` classification (replayable / local non-member / build input), (package, target) replay units including the tip lib, cumulative set, replay with captured args/env, rlib path from rustc artifact notifications | `crates/frust-drive/src/hotpatch/{graph.rs,replay.rs}` | complex | H1-01 | `cargo test -p frust-drive hotpatch::graph`; `cargo test -p frust-drive hotpatch::replay` (fixture: one-package lib with `["cdylib","staticlib","rlib"]`) |
| H1-03 | `hotpatch::fat_link` + `hotpatch::thin_link`: fat archive + force_load/whole-archive, anchor export, Darwin + Gnu thin args, patch naming under `<target>/frust-hotpatch/<session>/` | `crates/frust-drive/src/hotpatch/{fat_link.rs,thin_link.rs}` | complex | H1-01 | `cargo test -p frust-drive hotpatch::fat_link`; `cargo test -p frust-drive hotpatch::thin_link` |
| H1-04 | `hotpatch::symbols` + `stub` + `jump_table`: symbol cache (ELF/Mach-O, TLS init image), undefined-symbol stub (text thunks aarch64/x86_64, TLS, data), name-matched jump table with anchor rebasing | `crates/frust-drive/src/hotpatch/{symbols.rs,stub.rs,jump_table.rs}` | complex | H1-00 | `cargo test -p frust-drive hotpatch::symbols`; `cargo test -p frust-drive hotpatch::stub`; `cargo test -p frust-drive hotpatch::jump_table` |
| H1-05 | `hotpatch::layout` (L3 DWARF fingerprint tables: base at fat, candidate per thin, diff → reasons) + `hotpatch::seams` (row D identity via demangled `build_erased` instances); proves DWARF presence under the default dev `split-debuginfo` on macOS and Linux | `crates/frust-drive/src/hotpatch/{layout.rs,seams.rs}` | complex | H1-00 | `cargo test -p frust-drive hotpatch::layout` (fixtures: D2 field add, same-size reorder, closure capture change, return-type change); `cargo test -p frust-drive hotpatch::seams` |
| H1-06 | Protocol: `hotpatch_info`, `patch_chunk`, `apply_patch`, `PatchOutcome`, `Capability::HotPatch` | `crates/frust-devtools-protocol/src/{method.rs,messages.rs,lib.rs}` | medium | – | `cargo test -p frust-devtools-protocol` |
| H1-07 | In-app apply: frust-devtools dispatch + backend calls; frust-shell-common `ShellBackend` reassembles chunks (size cap), writes `0600` into the app cache dir, calls the safe frust-hotpatch entry, replies after the next frame with hits/fall-throughs/mismatches; facade feature chain | `crates/frust-devtools/src/{dispatch.rs,backend.rs}`, `crates/frust-shell-common/src/devtools.rs`, `crates/frust-shell-common/Cargo.toml`, `crates/frust-shell-desktop/src/app_handler.rs`, `crates/frust-shell-desktop/Cargo.toml`, `crates/frust/Cargo.toml` | complex | H0-02, H0-03, H1-06 | `cargo test -p frust-devtools`; `cargo test -p frust-shell-common --features devtools,hotpatch`; `cargo test -p frust-shell-desktop --features devtools,hotpatch`; `cargo clippy -p frust-shell-common -p frust-shell-desktop --features devtools,hotpatch --all-targets -- -D warnings` |
| H1-08 | `hotpatch::session`: fat build (3.2) → spawn exe → discovery + `hotpatch_info` → on change: classify (d) → thin build → L3 + identity gates → chunked upload + `apply_patch` → `Outcome`; patch budget (e); devtools client methods; `frust/hotpatch` feature for Debug hot sessions | `crates/frust-drive/src/hotpatch/session.rs`, `crates/frust-drive/src/devtools_client.rs`, `crates/frust-drive/src/desktop_run.rs`, `crates/frust-drive/src/build_info.rs` | complex | H1-02, H1-03, H1-04, H1-05, H1-07 | `cargo test -p frust-drive hotpatch::session` (fake runner + fake devtools server); `cargo test -p frust-drive devtools_client` |
| H1-09 | `frust run --watch` hot mode (default for debug desktop, `--no-hot` opt-out), watches all three path classes, prints outcomes, restart path on `RestartRequired` | `crates/frust-cli/src/commands/run.rs`, `crates/frust-cli/src/cli.rs` | medium | H1-08 | `cargo test -p frust-cli` |
| H1-10 | TUI: `Effect::HotPatch`, `Message::HotPatchOutcome`, toasts, restart fallback, palette rename; `R` stays a full restart | `crates/frust-tui/src/engine/{message.rs,update.rs,palette.rs}`, `crates/frust-tui/src/runner.rs`, `crates/frust-tui/src/supervise/{watch.rs,session.rs}` | complex | H1-08 | `cargo test -p frust-tui` |
| H1-11 | **Milestone 1 gate (macOS, GUI session):** stock `frust create` app (one package, template crate types) under `frust run --watch`: rows A, B, C (patched, State kept); D, D2, D3, E (each `restart required`, never `patched`); F (restart median, now through `frust run --watch`); G (10 patches + budget counters); H (one-package now patches); I (cold fat vs cold `cargo build`, one `target/`). Plus the TUI Watch leg. | `examples/hotpatch-spike/measure.sh`, `examples/hotpatch-spike/RESULTS.md` | complex | H1-09, H1-10 | `bash -n examples/hotpatch-spike/measure.sh`; the matrix runs; pass bar: median save→frame ≤ 50% of row F, zero `patched` lines for D/D2/D3/E |
| H1-12 | CI canary against the pinned toolchain (`rust-toolchain.toml`: 1.98.1): a headless standalone fixture with a `HotFn` loop, built fat, edited, thin-built and applied in-process, asserting the new return value; plus a D2-shaped edit asserting `RestartRequired` | `scripts/ci/hotpatch-canary.sh`, `testing/hotpatch-canary/**`, `.github/workflows/ci.yml` | complex | H1-08 | `bash scripts/ci/hotpatch-canary.sh` on macOS and on the Linux CI runner |

#### Phase H2: Android arm64

| ID | Title | Write scope | Cx | Deps | Verification |
|---|---|---|---|---|---|
| H2-01 | Android fat build outside Gradle (`cargo ndk ... rustc --lib -- <fat flags>` with capture env; proxy link through NDK clang; anchor export) + `gradlew ... -x cargoNdkBuild`; NDK clang for thin links; symbol cache from the unstripped cdylib | `crates/frust-drive/src/hotpatch/android.rs`, `crates/frust-drive/src/android_run/{mod.rs,gradle.rs}` | complex | H1-08 | `cargo test -p frust-drive hotpatch::android`; `cargo test -p frust-drive android_run` |
| H2-02 | frust-shell-android: hot-patch feature, patch listener → redraw, cache-dir write + memfd load through the shared backend | `crates/frust-shell-android/src/app.rs`, `crates/frust-shell-android/Cargo.toml`, `crates/frust/Cargo.toml` | medium | H1-07 | `cargo ndk -t arm64-v8a clippy -p frust-shell-android --features hotpatch,devtools -- -D warnings`; `cargo test -p frust-shell-android` |
| H2-03 | `frust run -d <android> --watch` hot path (lift `run.rs:63-72` for hot mode only) + TUI device sessions | `crates/frust-cli/src/commands/run.rs`, `crates/frust-tui/src/engine/update.rs`, `crates/frust-tui/src/engine/palette.rs` | medium | H2-01, H2-02, H1-09, H1-10 | `cargo test -p frust-cli`; `cargo test -p frust-tui` |
| H2-04 | **Pixel 5 gate:** rows A, B, D2, E, G on device via `frust run -d <serial> --watch`; first proof of patch relocation against the base cdylib | `examples/hotpatch-spike/RESULTS.md` | complex | H2-03 | device matrix; pass bar as H1-11 against the Android restart median measured in the same session |

#### Phase H3: Windows (MSVC)

| ID | Title | Write scope | Cx | Deps | Verification |
|---|---|---|---|---|---|
| H3-00 | Pin `pdb` 0.8.0 (dx's version) | `Cargo.toml`, `crates/frust-drive/Cargo.toml`, `Cargo.lock` | simple | H1-00 | `cargo build --workspace --locked`; `cargo tree -d` |
| H3-01 | PE/PDB: symbol cache, jump table, `__imp_` data stubs, Windows x86_64/aarch64 thunks, `lld-link` thin args (`/DLL`, `/EXPORT:` anchor, `/HIGHENTROPYVA:NO`), `/WHOLEARCHIVE` fat link | `crates/frust-drive/src/hotpatch/{pe.rs,thin_link.rs,fat_link.rs}` | complex | H3-00, H1-08 | `cargo test -p frust-drive hotpatch::pe` (on Windows CI) |
| H3-02 | Windows gate on the Dell: rows A, B, D2, G; L3 is DWARF-only, so Windows runs L2 + identity gate (documented limitation until a PDB type-record gate exists) | `examples/hotpatch-spike/RESULTS.md` | medium | H3-01 | matrix on Windows |

#### Phase H4: iOS simulator (probe first)

| ID | Title | Write scope | Cx | Deps | Verification |
|---|---|---|---|---|---|
| H4-01 | Simulator load probe (p2-02b's shape): can a `simctl`-launched frust app `dlopen` an unsigned / ad-hoc-signed dylib from its container? | `examples/hotpatch-spike/ios-probe/**`, `examples/hotpatch-spike/RESULTS.md` | medium | H0-01 | probe run on a booted simulator; negative control |
| H4-02 | Xcode fat-link steering via `xcodebuild` command-line settings (`OTHER_LDFLAGS` force_load, `DEAD_CODE_STRIPPING=NO`, anchor export), symbol cache from the built executable, `-isysroot` + simulator build version in thin links | `crates/frust-drive/src/hotpatch/ios_sim.rs`, `crates/frust-drive/src/ios_run/{xcodebuild.rs,simctl.rs}` | complex | H4-01 (GO), H1-08 | `cargo test -p frust-drive hotpatch::ios_sim`; `cargo test -p frust-drive ios_run` |
| H4-03 | Simulator gate: rows A, B, D2 | `examples/hotpatch-spike/RESULTS.md` | medium | H4-02 | matrix on the simulator |

#### Phase H5: documentation (doc-maintainer) and optional work

| ID | Title | Write scope | Cx | Deps | Verification |
|---|---|---|---|---|---|
| D-01 | **Doc card.** CORE + SHELLS entries for the seam, `set_patch_listener`, the `frust → shell → core` `hotpatch` feature chain and the anchor (act_000001a11662c563ATp0EhAG, act_000001a11662ca36slqbrdcZ) | `docs/CORE_ARCHITECTURE.md`, `docs/SHELLS_ARCHITECTURE.md` | simple | H0-03 | budgets per `docs/DOC_POLICY.md` |
| D-02 | **Doc card.** DEVTOOLS (three methods, `HotPatch` capability), CLI (`hotpatch` module, wrapper entry), TUI (Watch hot path) | `docs/DEVTOOLS_ARCHITECTURE.md`, `docs/CLI_ARCHITECTURE.md`, `docs/TUI_ARCHITECTURE.md` | simple | H1-10 | budgets |
| D-03 | **Doc card.** Version-Pin rows: `object`, `ar`, `gimli`, `rustc-demangle`, later `pdb` in CLI_DEVELOPMENT's table (`docs/DEVELOPMENT.md:406` owner), `base64` gaining a SHELLS consumer; toolchain-bump rule gains "and `scripts/ci/hotpatch-canary.sh` passes" (`docs/DEVELOPMENT.md:412-415`); test-gate rows for the feature gate and canary; DOC_POLICY unit row placing `crates/frust-hotpatch` in CORE (`docs/DOC_POLICY.md:22-33`); CODE_STANDARDS sanctioned-unsafe register entry for `crates/frust-hotpatch` (`docs/CODE_STANDARDS.md:14-23`) | `docs/DEVELOPMENT.md`, `docs/CLI_DEVELOPMENT.md`, `docs/TESTING.md`, `docs/DOC_POLICY.md`, `docs/CODE_STANDARDS.md` | medium | H1-00, H1-12, H0-05 | budgets |
| D-04 | **Doc card.** LIMITATIONS: rewrite `no-hot-reload-restart-is-a-rebuild` (desktop hot patch; restart-required cases (c)-(e); iOS device and web out; Windows L2-only) | `docs/LIMITATIONS.md` | simple | H1-11 | budgets |
| H5-01 | *Optional.* Path-dependency replay (2.d deferred design) | `crates/frust-drive/src/hotpatch/{capture.rs,graph.rs,replay.rs,layout.rs}` | complex | H1-11 data | `cargo test -p frust-drive hotpatch::` |
| H5-02 | *Optional.* rlib-only replay (2.b item 3) with a before/after thin-build measurement | `crates/frust-drive/src/hotpatch/replay.rs` | medium | H1-11 | `cargo test -p frust-drive hotpatch::replay`; timing table in RESULTS.md |

**Card counts.**

| Stage | Implementor | Doc | Gate / probe | Optional |
|---|---|---|---|---|
| 1 | 19 (H0-00..05, H1-00..12) | 4 (D-01..04) | – | – |
| 2 | 4 | – | – | – |
| 3 | 3 | – | – | – |
| 4 | 3 | – | – | – |
| H5 | – | – | – | 2 |

The stage-1 implementor count includes H0-00, H1-11 and H1-12, which are measurement/gate cards.

### 6.3 New pins and their Version-Pin Policy rows

| Pin | Version | Row owner (per `docs/DEVELOPMENT.md:400-408`) | Rationale for the row | Tripwire |
|---|---|---|---|---|
| `object` | 0.37.x (dx 0.7.10 uses 0.37.1, `dx:Cargo.toml:313`) | CLI_DEVELOPMENT.md | Stub-object writer and symbol reader; the Mach-O/ELF writer API is the part toolchain/format drift touches | `cargo test -p frust-drive hotpatch::` + canary |
| `ar` | 0.9.0 (`dx:cli/Cargo.toml:155`) | CLI_DEVELOPMENT.md | Reads rlib members, writes the fat archive | same |
| `gimli` | chosen in H1-00, compatible with the `object` pin | CLI_DEVELOPMENT.md | DWARF reader for the L3 gate | `cargo test -p frust-drive hotpatch::layout` |
| `rustc-demangle` | chosen in H1-00 | CLI_DEVELOPMENT.md | v0 demangling for the row-D identity gate | `cargo test -p frust-drive hotpatch::seams` |
| `pdb` | 0.8.0 (`dx:cli/Cargo.toml:156`), stage 3 | CLI_DEVELOPMENT.md | PE symbol cache | Windows CI `hotpatch::pe` |
| `base64` | existing `0.22` row (`Cargo.toml:182`) | CLI_DEVELOPMENT.md (gains a SHELLS consumer) | App-side chunk decode | `cargo check -p frust-shell-common --features hotpatch` |

Dropped relative to dx: `target-lexicon`, `krates`, `cargo_metadata`, `cargo-config2`, `rayon`,
`itertools`, `uuid`, `sha2`, `dunce`, `tokio`, `axum`, `tungstenite`, `walrus`, `wasmparser`,
`dioxus-*`. The spike-only exact pins (`dioxus-devtools =0.7.10` in
`examples/hotpatch-spike/runner/Cargo.toml`, `subsecond-types =0.7.10` as frust-hotpatch's
dev-dependency) stay off the root graph's normal dependencies. The latter goes when the wire-compat
test is dropped (4.2).

### 6.4 Maintenance risk: rustc and linker argument formats

**What can break on a toolchain bump.** The builder depends on internals that are stable in
practice but not by contract:
- how cargo passes `--crate-type`, `--out-dir`, `-C extra-filename` and `--json`;
- that rustc invokes the linker with `.o` arguments or `@` response files (`dx:cli/src/rustcwrapper.rs:138-170`);
- the linker flags the thin link forwards per flavor (`dx:cli/src/build/link.rs:393-464`). dx
  already tracks one such drift: the Rust 1.86 `-B`/`-fuse-ld=lld` injection, `:442-445`;
- symbol mangling (v0 since the toolchain's default changed; RESULTS.md row D notes);
- TLS symbol conventions (`$tlv$init`, `dx:cli/src/build/patch.rs:1172-1199`).

**Mitigation.**
1. **CI canary (H1-12)** against the pinned toolchain (`rust-toolchain.toml`: 1.98.1), on macOS
   and Linux runners. It exercises capture → fat → edit → thin → L3 → apply → assert.
2. **The toolchain rule.** Bumping the toolchain is already its own change, gated on clippy
   (`docs/DEVELOPMENT.md:412-415`). D-03 adds "and the hot-patch canary passes". A bump that
   breaks the builder then fails in its own PR, not on a developer's machine.
3. **Fail closed.** Any parse surprise (unknown linker flavor, missing capture, missing anchor,
   DWARF absent) yields `RestartRequired { BuilderUnsupported { detail } }`, never a guessed patch.
   `frust run --watch` then degrades to today's restart loop.

---

## 7. Scope recommendation: **STAGED**

**Recommendation in one sentence:** build STAGED, desktop first. **Milestone 1** is H1-11 passing:
on macOS, with Linux covered by the CI canary, `frust run --watch` and the TUI's Watch hot-patch an
unmodified one-package `frust create` app with State kept, every hazard row (D, D2, D3, E) answers
`restart required` and never `patched`, and the median save→frame is at most 50% of the restart
median.

**Desktop is proven end-to-end except for the two builder fixes, which are design work on a
measured path.**
- Phase 1: 475 / 480 ms median save→frame through subsecond, against a 1570 ms restart (30%).
- Phase 2: 575 / 577 ms through frust-hotpatch (37%). The extra ~80 ms is dx recompiling the runner
  tip for `connect(callback)` + serde. Requirement (a) removes the tip recompile altogether (2.a),
  so the frust builder should land under Phase 1's figure. Row A's breakdown is ~230 ms replay +
  ~105 ms tip compile + ~75 ms link + 20-30 ms transport (RESULTS.md, Surprises). This is an
  estimate to be measured by H1-11.
- Everything else in the chain ran on this Mac: capture, replay, stub, jump table, frust-hotpatch
  load and dispatch, State preserved across 10 patches.
- The two builder fixes (2.a, 2.b) are logic changes to a measured path, not new mechanisms.

**Android has three unproven links; desktop has none.** p2-02b proved *loading* only, with a
self-contained library ("nothing in it is relocated against the running app library", RESULTS.md).
On Android, a frust build still needs:
1. a fat cdylib built outside Gradle (3.3);
2. an anchor that is not `main` (2.f);
3. stub relocation against a base `.so` loaded by the JNI loader rather than an executable.

FULL would put all three on the critical path of the first usable feature, while the desktop
payoff is already measured. STAGED ships the desktop value after stage 1 and opens stage 2 with the
relocation proof (H2-04) as its first gate.

**Stage order after milestone 1.**
1. **Android** (stage 2): mobile is frust's primary target, and loading is already proven there.
2. **Windows** (stage 3): mechanics exist in dx; a rig is available.
3. **iOS simulator** (stage 4): behind a load probe, because Xcode owns the link.
