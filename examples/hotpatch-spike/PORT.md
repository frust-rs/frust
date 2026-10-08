# PORT.md: a frust-owned patch builder

Card p2-03c (tsk_000001a1169831d4W8yKhj8T), plan fplan_000001a10e032a8eKL91PGxC, written 2026-10-07 on
`spike/hotpatch` @ 085509c5. Per Ed's Phase 2 direction this document **assumes** the builder is
frust-owned. It does not argue GO/NO-GO for owning it. It sizes that builder from dx 0.7.10, maps it
onto frust-drive / frust-cli / frust-tui, designs the five Phase-1 requirements, and ends with a
card list a follow-up plan can adopt as written.

**Revision r2-03** (tsk_000001a11723a3cdnNA84QrR, on `spike/hotpatch` @ 5adc7136) answers the
Phase 2 review round 1: the L2 witness now comes from the image that created the value (2.c), L3
compares against every live image, not only the base (2.c), Windows is apply-disabled until a PDB
type-record gate exists (5), and section 7 carries the 493 ms estimate, the milestone-1 risks and
the security preconditions (also 3.7).

**Revision r2-04** (tsk_000001a117b592910ov4DT6j, on `spike/hotpatch` @ e4c56f21) answers the
review of r2-03 (round rvr_000001a11758aeedOYMAqhXK). L2 is narrowed to components hosted in their
own erased `Box<dyn Widget>`, after an `Either` arm-swap counterexample, and a witness mismatch now
leaks the state instead of tearing it down (2.c, new H1-11 row D5). A late L2 mismatch makes the
next apply fail closed (2.c, 3.6, H0-01). A fall-through is judged by its missed key, not a count
(2.c, 2.d, 3.6). Every citation into the files cards r2-01 and r2-02 changed is re-anchored.

**Citation conventions.**

- `dx:` paths are DioxusLabs/dioxus at tag **v0.7.10** (commit 57d6794, shallow clone outside the
  repo), relative to `packages/`. Example: `dx:cli/src/build/link.rs:612`.
- Bare paths are this repository on `spike/hotpatch` @ e4c56f21. Between 085509c5 and e4c56f21 the
  only sources that changed are `crates/frust-hotpatch/**`,
  `examples/hotpatch-spike/runner/src/main.rs` and `examples/hotpatch-spike/android-probe/probe.sh`
  (cards r2-01 and r2-02), so every other source citation holds on all three revisions. Revision r2-04 edits doc comments in
  `crates/frust-hotpatch/src/{hot_fn.rs,patch.rs}` and the runner's diagnostics, so citations into
  those two files and the runner name the symbol and give its lines **at r2-04's own commit**.
  `crates/frust-hotpatch/src/lib.rs` is cited at e4c56f21, which r2-04 does not change. A citation
  of the text as it stood before r2-01/r2-02 says "pre-r2-01" or "pre-r2-02".
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
| (implicit in every hot-patch design so far) frust-hotpatch's ASLR anchor `dlsym(RTLD_DEFAULT, "main")` (`main_address`, `crates/frust-hotpatch/src/patch.rs:231-236`) works on every target. | On Android the app's code is the lib's **cdylib** (`crates/frust-drive/templates/app/Cargo.toml.tmpl:14-15`), entered through JNI, and it defines no `main`. dx's Android tip is different: a `[[bin]]` linked as `libmain.so` that does contain `main` (`dx:cli/src/build/request.rs:2546-2550`). On frust's Android the anchor cannot be the app library's own `main`, so an app-owned anchor is required (section 2.f). |
| "Nothing rewrites process memory: a call only reaches new code by passing through a `HotFn`" (`crates/frust-hotpatch/src/lib.rs:10-11`, pre-r2-01), which round 0 of this document relied on for L2. | Memory is indeed never rewritten, but a `HotFn` call is only the **first** entry into patch code. Every trait object patch code creates carries a patch-image vtable: an `AnyView` erased in patched code (`crates/frust-core/src/view.rs:316-318`) is later rebuilt by old code through `dyn_rebuild` (`view.rs:253-274`), and a `Box<dyn Widget>` built by patched `dyn_build` (`view.rs:249-251`) runs patch code on every event and paint. A closure from patched code is copied into an old retained element at rebuild (`crates/frust-widgets/src/button.rs:669`) and called from it on press (`:901`). None of these passes through a `HotFn`. *Refuted*; card r2-01 replaced the sentence (now `lib.rs:16-20` at e4c56f21), and 2.c states the real boundary. |

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
| 5b | Transport, app side | `devtools/src/lib.rs:57-111` | 55 / 41 | tungstenite, dioxus-cli-config | **Replace** (already bypassed: `connect_devserver` and `apply_hot_patch`, `examples/hotpatch-spike/runner/src/main.rs:32-91`) |
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
  (`apply_patch_with_anchor`, `crates/frust-hotpatch/src/patch.rs:165-208`).
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

The two images are not always the base and the newest patch. A value created by patch 1 and read
by patch-2 code crosses a boundary too (the two-patch case under L3 below), and the creating code
is not always the caller (the nested-component case under L2 below).

**Measured status.**
- Row D (identity change) and row D2 (State layout) are measured.
- **Row D3** (return-type edit: wrap the root `column()`) is not measured yet. Card H0-00 measures it
  with dx on the existing spike before any builder work. H0-00 is a **gate on milestone 1**, not a
  follow-up: H1-11 depends on it, and milestone 1 cannot be declared until H0-00 has run and its
  observed behaviour (no-op, corruption or crash) is reproduced as an H1-05 fixture. If H0-00 shows
  a D3 behaviour the L1 + L3 argument below does not predict, the plan stops at phase H0 for a
  redesign.

**Why an instance or symbol comparison is insufficient.**
- dx matches by symbol name (`dx:cli/src/build/patch.rs:420-424`).
- The seam symbol is `<<C as Component>::build as HotFunction<(&C, &mut C::State), Fn2Marker>>::call_it`.
  It names `C` and `C::State` by **path** and names neither layout nor `R`.
- A field added to `C`, a field added to a named `C::State` (D2), and a new `R` all leave that
  symbol unchanged. The entry exists, and the patch is taken over the old layout.
- Only a type-**identity** change of `C::State` (row D) changes the symbol, and that produces a
  silent no-op (no entry), not a crash.
- A symbol comparison can therefore detect D, and nothing that is actually unsafe.

**Design: the safety argument is L1 + L3. L2 is an in-app backstop.**

- **L1: remove the return slot from the boundary (structural, zero cost).**
  - The hot function becomes `build_erased::<C>(c: &C, s: &mut C::State, witness: SeamWitness) ->
    Result<AnyView<C::State>, LayoutMismatch>`. It erases **inside** the patched code. The type
    crossing the return slot is then `Result<AnyView<C::State>, LayoutMismatch>`, whose layout
    cannot change: `AnyView` is one `Box<dyn ErasedView>` (`crates/frust-core/src/view.rs:297-299`).
  - `ComponentView` stops erasing the seam result itself. `AnyView::new` is idempotent (`:307-319`),
    so behaviour with the feature off is unchanged.
  - The erasure tripwire's rule T7 ("helpers ending in one erasure call return impl View",
    `scripts/ci/erasure-check.sh:9`) would flag `build_erased`. It carries the documented opt-out
    `// erasure: keep hot-patch boundary type must be layout-fixed`
    (`scripts/codemod/frust_any_codemod.py:121`).
  - **What L1 does not do.** It removes one crossing, the return slot, and nothing else. It
    **shrinks the D2/D3 hazard to the retained tree rather than eliminating it**: `prev`
    (`component.rs:127`) still holds the old `R`, the child pod holds the element old code built
    for it (`component.rs:130`), and patched `dyn_rebuild` downcasts both by `TypeId`
    (`view.rs:259-266`) and reads them through the new layout. A D3-class edit that keeps `R`'s type
    path (a field added to an app view struct, a changed closure capture) is still a hazard after
    L1. Closing D3 depends on L3 and on H0-00 confirming the mechanism.

- **L2: a creator-image layout witness, checked at every component seam (in-app backstop, no host
  data).**

  *Why round 0's L2 was wrong.* Round 0 had the **caller** compute `SeamLayout` from its own
  `size_of`/`align_of`, on the premise that the caller is old code. With L1 in place that premise
  holds only for the outermost component. The spike app shows it: root `SpikeApp`
  (`examples/hotpatch-spike/app/src/lib.rs:15-28`) hosts `HomePage`, whose `State` is `HomeState`
  (`app/src/home_page.rs:18-27`). Take the row D2 patch (`HomeState` 4 → 8 bytes):
  1. dx's jump table maps every symbol present in both images (`dx:cli/src/build/patch.rs:420-424`),
     and the patch holds the whole replayed crate. So `build_erased::<SpikeApp>` is mapped although
     `SpikeApp` did not change.
  2. The old root driver, routed through the seam by H0-02, calls patched `build_erased::<SpikeApp>`.
     That function erases `component(HomePage { .. })` in **new** code, so the returned `AnyView`
     carries the patch image's vtable (`view.rs:316-318`).
  3. The old driver rebuilds through it. `AnyView::rebuild` dispatches to the patch image's
     `dyn_rebuild` for `ComponentView<HomePage>` (`view.rs:253-274`), which downcasts the old view
     and the old `ComponentWidget<HomePage>` element by `TypeId` (`:259`, `:263-265`). Both match.
  4. The patch's `ComponentView<HomePage>::rebuild` (`component.rs:203-275`) reads `element.state`,
     stored inline (`component.rs:124`), and every later field of the widget at the **new**
     offsets. It calls `build_erased::<HomePage>` with a `SeamLayout` it computed itself.
     `size_of::<HomeState>()` is 8 on both sides of that call, so round 0's L2 passes and the D2 UB
     runs.

  Old code is the caller only for the outermost component.

  *Design.*
  1. **A widget layout that does not depend on `C`.** Under `feature = "hotpatch"`,
     `ComponentWidget<C>` (`component.rs:121-141`) becomes `#[repr(C)]`. Its first field is
     `witness: SeamWitness`, and its state is boxed and never dropped implicitly:
     `state: ManuallyDrop<Box<C::State>>` (design point 4 says who drops it). Every other field is
     already independent of `C`'s layout: `prev` is an `AnyView`, one `Box<dyn ErasedView>`
     (`view.rs:297-299`), and `child`, `owner`, `next_id` and `disposed` (`component.rs:130-140`)
     name no `C` type. Every image therefore computes the same offset for every field of every
     `ComponentWidget<C>`, whatever `C::State` looks like in that image. Without the feature the
     struct is unchanged. The cost is one heap allocation per component, in hot-patch builds only.
  2. **The witness is written by the creating image.** `SeamWitness { size_state, align_state,
     size_c, align_c }` (four `u32`) is written once, in `ComponentView<C>::build`
     (`component.rs:169-200`), the code that runs `init` and so creates the state. Its values are
     that image's constants.
  3. **It is checked before any `C`-generic code touches the widget's state.**
     `ComponentView<C>::rebuild` and `::teardown` (`component.rs:203`, `:277`) and, under the
     feature, `ComponentWidget<C>`'s `Drop` (`component.rs:158-164`) each compare `element.witness`
     with their own `SeamWitness::of::<C>()` as their first statement, before forming
     `&mut C::State` and before reading `prev` or `child`. `rebuild` and `teardown` are reached by
     `TypeId` downcast from whichever image's `dyn_rebuild` runs (`view.rs:259-266`), or from a
     generic container's own `rebuild` (the counterexample below). `Drop` runs from whichever
     image's drop glue drops the widget. The widget's `Widget` methods (`event`, `layout`, `paint`,
     `component.rs:298-392`) do not check. They run through the vtable of the `Box<dyn Widget>`
     that hosts the widget, so they agree with the stored layout only when the creating image made
     that box. That is the coverage condition below.
  4. **On mismatch nothing `C`-generic runs, the state leaks, and the seam raises the restart
     signal.**
     - `rebuild` returns `ChangeFlags::NONE` without running `build` and without touching `state`,
       `prev` or `child`, so the stale subtree stays on screen until the restart.
     - `teardown` does **not** proceed: it skips the child teardown through `prev`
       (`component.rs:291`) and the dispose (`:294`), and touches nothing.
     - `Drop` drops the state box (`ManuallyDrop::drop`) only when the witness matches. On a
       mismatch it leaves the `ManuallyDrop` untouched, which is `mem::forget` of the
       creator-owned box: neither `drop_in_place::<C::State>` nor the box's dealloc runs, so no
       non-creator image drops or frees the creator's allocation. It does not use `Box::leak`,
       which would form a `&mut C::State` typed with the non-creator's layout. It still disposes
       the owner (`dispose`, `component.rs:147-155`, which touches only `owner` and `disposed`).
       The remaining fields drop normally: none depends on a `C` layout, and `prev`'s erased view
       drops through its own vtable.
     - Each of the three calls `frust_hotpatch::report_layout_mismatch(type_name::<C::State>(),
       stored, own)` (card H0-01), and the host turns the record into `RestartRequired {
       LayoutChanged }` (3.6). When the record reaches the host is set out under *When a mismatch
       surfaces* below.

     The leak is bounded: one `C::State` per mismatched widget, once, and the session restarts.
  5. **The root.** The root driver is the outermost caller and the creator of the root state: it
     runs `init` (`crates/frust/src/lib.rs:2121` desktop; `:2291`, `:2306`, `:2329` Android, iOS,
     web). Its own constants are therefore the creator's, and it passes `SeamWitness::of::<Root>()`
     to `build_erased`. For a nested component, `ComponentView::rebuild` passes `element.witness`.
     `build_erased` compares the witness with its own constants before touching `s`, which also
     covers the root's props. The witness argument always comes from storage the creating image
     wrote, never from the caller's own `size_of`.

  *The trace with this design.* Steps 1-3 are unchanged. In step 4 the patch's
  `ComponentView<HomePage>::rebuild` reads `element.witness` at offset 0, which is the same in both
  images by design point 1. It holds `size_state = 4`, written by the base image when it built the
  widget, while the patch's own constant is 8. The patch reports `HomeState 4 → 8` and returns
  without forming `&mut HomeState` and without calling `build_erased::<HomePage>`. The new
  increment closure (`home_page.rs:51`) is never installed into the old button element either,
  because that happens only inside the skipped rebuild (`crates/frust-widgets/src/button.rs:669`).
  The next `PatchOutcome` carries the mismatch, and the session restarts.

  *Counterexample: a component held inline by a generic container.* Design point 3 holds only when
  the widget sits in its own `Box<dyn Widget>` made by its creator. A generic container can hold it
  **inline** instead. `either(cond, || component(HomePage { .. }), || text(..))` builds an
  `EitherWidget<ComponentWidget<HomePage>, _>` (`crates/frust-widgets/src/either.rs:68`, element
  type at `:122`): an enum holding the live arm's widget by value, with "no `AnyView` or
  `Box<dyn Widget>` field", whose two arms share one element type (module doc, `either.rs:27-30`).
  Any app-defined `View` whose `Element` embeds a child element has the same shape. (In the
  framework, a search for `type Element = .*::Element` over `crates/` and `plugins/` at e4c56f21
  finds only `Either` and a non-generic alias, `plugins/shadcn/src/components/avatar.rs:456`.)
  Take a parent component built by the base whose `build` returns that `either`, and apply a
  D2-class edit (`HomeState` gains a `String` field, which changes its size and alignment):
  - **Patch drop glue over a base allocation.** A patched rebuild of the parent reaches the
    patch's `Either::rebuild` monomorph, which is instantiated in the replayed app crate and
    entered through the patch image's `dyn_rebuild` (`view.rs:253-274`). On an arm swap it calls
    `prev.teardown(element, ctx)` (`either.rs:143`), the patch's `ComponentView<HomePage>::teardown`
    on the base-created widget, and the witness mismatches. Under r2-03's rule teardown then
    proceeded. Next, `*element = self.build(ctx)` (`either.rs:150`) drops the old
    `ComponentWidget<HomePage>` in place with the **patch** image's drop glue: a
    `drop_in_place::<HomeState>` with the new layout, then a dealloc with the new `Layout` of a box
    the base allocated at the old size. That corrupts the heap before any restart is requested.
  - **Base vtable over a patch-sized state.** The swap builds the new arm in patch code, so a later
    swap back to the left arm creates a patch-image `ComponentWidget<HomePage>`: the patch's `init`
    allocates the new, larger `HomeState`. It lands inside the same `EitherWidget`, whose hosting
    `Box<dyn Widget>` the base made. From then on the **base** `EitherWidget` methods dispatch to
    the base `ComponentWidget<HomePage>::event`/`paint`, which read the patch-sized state through
    the old layout, and the base drop glue would later drop and free it with the old layout.

  Neither direction passes through a `rebuild` check, so H0-02's original test (the rebuild-skip
  path only) would have encoded the gap. L3 still refuses this edit on DWARF targets (`HomeState`
  is an app type in the accepted set), so the L1 + L3 argument is untouched. What was wrong is L2's
  stated coverage and r2-03's "teardown proceeds" rule.

  *The L2 coverage condition.* L2 is sound for a component only if, after a witness mismatch:
  1. no `C`-generic code from an image other than the creator touches `Box<C::State>`: not
     `rebuild` or `teardown`, and not the drop glue or `Widget` methods reached through an
     enclosing **inline** element; and
  2. teardown on a mismatch never leads to a drop of `Box<C::State>` by a non-creator image.

  *Design chosen: (ii) narrow + leak.*
  - **L2 covers only components hosted in their own erased `Box<dyn Widget>`**: a `ComponentView`
    built through `AnyView` (`dyn_build`, `view.rs:249-251`). That is the value a component's
    `build` returns (erased by `build_erased`), and every child a container erases through
    `ViewSeq::extend_views` (`view.rs:435-439`) and pods through `build_child`
    (`crates/frust-widgets/src/authoring.rs:278-281`). The hosting box and its vtable then come
    from the image that ran `ComponentView::build`, so `Widget` methods and drop glue always run
    creator code, and condition 1 reduces to the checks of design point 3.
  - **Components held inline by a generic container are covered by L3 only.** For them condition 1
    fails on the `Widget` methods (the base-vtable direction above), and no in-app check can close
    that short of a witness test on every `event`, `layout` and `paint`.
  - **Condition 2 holds for every host, by the leak rule** (design point 4): `teardown` does not
    proceed on a mismatch, and `Drop` frees the state only when the witness matches. The
    patch-drop-glue direction of the counterexample leaks the base's `HomeState` and reports
    instead of corrupting the heap. The rule keeps an L2 mismatch from doing harm in a host L2
    does not cover.
  - **Why not (i)**, routing the state's drop through a creator-image `drop_state: unsafe fn(*mut
    ())` carried in the witness. It makes condition 2 hold by construction but does nothing for the
    reverse direction: a base `EitherWidget` vtable still runs base `event`/`paint` over a
    patch-created state long before any drop. Making that sound would mean dispatching every
    `Widget` method through creator-image pointers too, in effect a second per-component vtable,
    which cannot be shown sound in the page the review allowed. (ii) costs one leaked `C::State`
    per mismatched widget until the restart.

  *When a mismatch surfaces.* A mismatch is found only when its widget is rebuilt, torn down or
  dropped. A retained component that is not rebuilt in the frame after a patch (an off-screen row,
  a subtree rebuilt only on a later signal) is checked at some later frame, after that patch's
  `PatchOutcome` went out with no mismatch and its layouts were merged into the accepted set (L3
  below). Nothing else would surface it before the host sends the next patch, which the app would
  apply before anyone asked for a restart. So:
  - `report_layout_mismatch` keeps each record until it has been reported (H0-01).
  - The apply entry **refuses** while any record is unreported: it loads nothing and answers
    `applied: false` with the records in `layout_mismatches` (4.1).
  - `hotpatch_info` also returns the pending records, and the host reads them before every send.
    A non-empty list is `RestartRequired { LayoutChanged }` with nothing sent (3.6).

  The restart is therefore requested at the latest on the next `PatchOutcome`, and no patch is ever
  applied over a known mismatch. Until then the late mismatch has done only what design point 4
  allows: a skipped rebuild or a leaked state.

  *What L2 covers, and what it does not.*
  - Covered: a size or alignment change of `C::State` at every component **hosted in its own
    erased `Box<dyn Widget>`**, nested ones included, whichever image runs the rebuild, on every
    target, without DWARF; and the root's props. For a mismatched component in any host, no
    non-creator image drops or frees its state (the leak rule).
  - Not covered: components held inline by a generic container (`Either`, or an app `View` whose
    `Element` embeds a child element), whose `Widget` methods can run a non-creator image over the
    state (the counterexample above; L3 only); same-size changes and field reorders of
    `C::State`; nested props (a nested `C` value lives inside the parent's `R`, not in the widget);
    `R` and everything else in the retained tree; closure environments; and values read through `TypeId`-keyed storage, i.e. event-time
    `state_mut` (`crates/frust-core/src/event.rs:2404-2406`) and owner-scoped context
    (`component.rs:24-27`). A same-size `HomeState` edit passes L2, and the new increment closure is
    then installed and later runs on the old value.
  - L2 is therefore a backstop that turns the measured D2 case into a restart even when L3 is absent
    or wrong. It is **not** a soundness argument on its own. Soundness rests on L1 + L3, and a target
    without L3 applies no patch at all (Windows, section 5).

  *The real boundary into patch code.* Before r2-01, `crates/frust-hotpatch/src/lib.rs:10-11` said
  a call only reaches new code through a `HotFn`. That was false (section 0). Card r2-01 replaced
  it with the boundary below (`lib.rs:16-20` at e4c56f21), and nothing here relies on the old
  sentence. Patch code is entered through:
  1. `HotFn` calls resolved through the table (`HotFn::try_call`,
     `crates/frust-hotpatch/src/hot_fn.rs:218-250`): the seams;
  2. the vtable of every trait object patch code creates: an `AnyView` erased in patched code
     (`view.rs:316-318`) and dispatched by old rebuild code (`view.rs:253-274`), and a
     `Box<dyn Widget>` element built by patched `dyn_build` (`view.rs:249-251`), whose `event`,
     `layout` and `paint` then run patch code for the element's lifetime;
  3. closures and function pointers that patched views store into retained elements
     (`button.rs:669`, called at `:901`);
  4. values patch code writes into `TypeId`-keyed storage, read later by either image.

  Once one seam call reaches a patch, entries 2-4 carry patch code through the rest of the tree
  with no `HotFn`. That is why L3 covers every replayed type, not only the seam's argument types.

- **L3: host-side structural gate (sound for app-defined types on DWARF targets).**
  - **Computed at build time.** At the **fat** build, before any replay overwrites an rlib in place
    (dx replays "at the same paths cargo originally wrote to", `dx:cli/src/build/link.rs:514-517`),
    the builder reads the DWARF of every replayable target's objects. It records one entry per type
    whose DWARF path lies in a replayable crate, and per instantiation of an external generic over
    such a type. The entry holds `(byte_size, alignment, [(member name, offset, member type path)],
    variant parts)`, hashed recursively. This base table seeds the session's **accepted-layout
    set** and is saved as `layout-base.json` in the session dir. Closure environments appear in
    DWARF as `{closure_env#N}` types under their function's path, so they are covered.
  - **Checked against every live image, not only the base.** Values created by an accepted patch
    stay live after the next patch. The patch-1 code that created them stays reachable through
    entries 2-4 above, and patch-2 code downcasts them by `TypeId`, which is identity, not layout
    (`view.rs:259-266`). So every thin build extracts the same table from its replayed objects and
    compares it, before anything is sent, with the accepted-layout set: the base table plus the
    entries of every accepted patch, keyed by type path. Every type present in both must have the
    same hash. Any difference means `RestartRequired { reason: LayoutChanged { type_path, old_size,
    new_size } }`, and the patch is never offered. A type absent from the set is new and passes.
  - **The two-patch case.** Patch 1 adds `struct Badge { n: u32 }` as a child component's `State`,
    a view struct, or a closure capture. `Badge` is not in the base table, so patch 1 passes and is
    applied, its `Badge` entry (4 bytes) joins the set, and live `Badge` values now exist, created
    by patch-1 code. Patch 2 adds a field to `Badge`. Round 0 compared only against
    `layout-base.json`, which has no `Badge`, so patch 2 passed and its code would read the live
    4-byte values with the 8-byte layout: D2-class UB. Against the accepted set, `Badge` 4 ≠ 8, and
    the answer is `RestartRequired`. If `Badge` is a component's `State`, L2 also catches it at run
    time. As a view struct or a closure environment, only L3 can.
  - **One layout per type.** A patch that changes any type already in the set is refused, so every
    accepted image agrees on every type, and "the most recently accepted layout" is the only one.
    Entries are never removed within a session: a type that patch 3 drops may still have patch-1
    values live, and re-adding it with another layout in patch 4 must still be caught.
  - **Where the set lives, and when it resets.** `hotpatch::session` (H1-08) owns the set in memory.
    It is seeded from the fat build's base table. A candidate's entries are merged only when that
    patch's `PatchOutcome` reports `applied: true` with no L2 mismatch. A **late** mismatch, found
    after that outcome went out (*When a mismatch surfaces*, above), unmerges nothing: the patch is
    applied and its values may be live. It is caught at the next contact instead. The host reads
    `hotpatch_info`'s pending records before the next send, and the app's apply entry refuses with
    `applied: false` and the records if they are still unreported. Either way the session answers
    `RestartRequired { LayoutChanged }`, the restart is requested at the latest on that next
    `PatchOutcome`, and no further patch is applied in between. After each merge the set is
    written to `<target>/frust-hotpatch/<session>/layouts-accepted.json` (the session dir of 2.e)
    for diagnosis; it is never read back into another session. An outcome that leaves the app's
    state unknown, such as a transport error or timeout after `apply_patch` was sent, answers
    `RestartRequired { PatchOutcomeUnknown }`, because that patch's types may now be live. Patches
    are sent one at a time per session (r2-01 also serialises `apply_patch` in the app), so a merge
    never races a second candidate. Every relaunch (any `RestartRequired`, the TUI's `R`, a fat
    rebuild, the app exiting) starts a new session: the old dir is deleted (2.e), the new process
    holds only base-image values, and the set restarts from the new base table.
  - **Scope.** This is deliberately a superset of the types reachable from `C`/`C::State`/`R`.
    Reachability through `Box<dyn ...>` children is invisible in DWARF (a trait object member is a
    data/vtable pair), while `TypeId` downcasts re-enter those types on rebuild. The cost is false
    positives: editing a type that has no live values also restarts. That is accepted. Being sound
    matters more than avoiding an occasional restart.
  - **Debuginfo.** The template's dev profile is cargo's default, `debug = true`; nothing in
    `crates/frust-drive/templates/app/Cargo.toml.tmpl` overrides it for dev. Card H1-05 must
    verify, on each host, that replayed rlib members carry DWARF under the default
    `split-debuginfo` (on macOS the DWARF stays in the `.o` files, which is where the gate reads
    it). Missing DWARF fails closed (`BuilderUnsupported`, 6.4).
  - **DWARF targets only.** The gate reads DWARF, so the guarantee holds only for targets whose
    objects carry it: Mach-O and ELF (macOS, Linux, Android, the iOS simulator). MSVC Windows keeps
    types in PDB type records. Until a PDB type-record gate exists (card H3-02), Windows applies no
    patch at all (section 5).

- **Row D (identity change) detection, host side.**
  - The builder demangles the `call_it` instances of `build_erased` (`rustc-demangle`, v0 symbols,
    which rustc 1.98.1 emits per RESULTS.md row D notes).
  - It compares each candidate instance with the **accepted seam set**: the base's instances plus
    every accepted patch's, kept beside the accepted-layout set and reset with it. If the candidate
    has an instance for `C` whose argument tuple differs from the set's instance for the same `C`,
    `C::State`'s identity changed. The builder answers `RestartRequired { StateTypeChanged {
    component } }` instead of shipping a patch that cannot take effect. A component first added by
    patch 1 has no base instance; its patch-1 instance is what patch 2 is compared with.
  - In-app, fall-throughs stay a backstop, but a count cannot carry it. The lookup key is the
    calling image's own `call_it` address (`call_it_address` and `HotFn::try_call`,
    `crates/frust-hotpatch/src/hot_fn.rs:124-126`, `:218-250`), while the table's keys are
    base-image addresses (`dx:cli/src/build/patch.rs:420-424`). A seam call made from the newest
    patch's code therefore always misses although it runs the newest code: in the L2 trace's steps
    1-4, which every patch to the spike app follows (not only D2), the patch's
    `ComponentView<HomePage>::rebuild` calls `build_erased::<HomePage>` from patch code. Every patch
    to an app with nested components would record `seam_fall_throughs > 0` (counted by
    `record_lookup` into `fall_through_count`, `hot_fn.rs:58-63`, `:99-101`).
  - **Requirement on the runtime and host (H0-01, H1-07, H1-08).** The runtime records each distinct
    missed **key**, not only a count, as `(image, link_address)`: `image` 0 is the base executable
    and `n` the n-th loaded patch, found from the address ranges of the images it loaded, and
    `link_address` is the key minus that image's slide. `PatchOutcome` carries the list (4.1). The
    host classifies each entry. A key in the newest patch is a benign patch-image caller. A key in
    the base or in an older patch ran stale code, and the host maps it through that image's symbol
    table, which it built. Only such a key that is a seam instance the builder reported as present
    in the patch means `restart required`.
  - The spike crate's `fall_through_count` stays a plain count. Its doc names the benign
    patch-image miss and says a restart rule needs the key; the key recording is new H0-01 work.

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
     `seam_fall_throughs`, the list of missed keys as `(image, link_address)` (2.c, row D
     detection).
   - The CLI/TUI print `patched in N ms (k components rebuilt)` only if `seam_hits > 0`, no L2
     mismatch was recorded (in the reply or pending in `hotpatch_info`), and no missed key from the
     base or an older patch is a seam instance the builder reported as present in the patch. Misses
     whose key lies in the newest patch are benign patch-image callers and are ignored.
   - This rule works only because the runtime records the missed **key** (H0-01): a count cannot
     tell a benign patch-image miss from a stale one. The spike crate's `fall_through_count` stays
     a plain count, so it can never drive the rule on its own.
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
- Images are never unloaded, by design (`apply_patch_with_anchor`,
  `crates/frust-hotpatch/src/patch.rs:182-184`).
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
  with `dlsym(patch, ...)` in place of `main` (today `lib.get(b"main")` in `apply_patch_with_anchor`,
  `crates/frust-hotpatch/src/patch.rs:187`).
- This works for an executable (desktop, simulator) and for the Android cdylib alike. It replaces
  dx's `main` export flags (`dx:cli/src/build/link.rs:991-1008`, `:479`).
- **It is a desktop change too, and unmeasured everywhere.** Desktop today anchors on `main`
  (`apply_patch_with_anchor` and `main_address`, `crates/frust-hotpatch/src/patch.rs:165-208`,
  `:231-236`), and every Phase 1/2 row ran on that.
  No row has run with an app-owned anchor on any target, so it is a milestone-1 risk (section 7).
- **Fail closed.** Before r2-01 an unresolved runtime anchor was cached as `0` and the table was
  rebased by garbage. Card r2-01 made a missing anchor an error: `aslr_reference` retries an
  unresolved 0 instead of caching it (`patch.rs:217-229`), and `apply_patch_with_anchor` returns
  `PatchError::AnchorUnresolved` before loading anything (`patch.rs:174-177`). H0-01 keeps that
  rule for `set_anchor`: `apply_patch` refuses until an anchor is set, and the session answers
  `RestartRequired { BuilderUnsupported }`.

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
  3. Fat-link (`hotpatch::fat_link`), write `layout-base.json` and seed the accepted-layout and
     accepted-seam sets from it (L3, 2.c), build the symbol cache.
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
host gates (path dep / build input / L3 layout vs accepted set / D identity vs accepted seams
            / budget / HotPatch capability absent
            / hotpatch_info reports pending L2 mismatches)
        ──► session.on_change → Outcome::RestartRequired(reason)        (nothing sent)
apply_patch reply (L2 witness mismatch / applied: false on unreported L2 mismatches
                   / missed key from base or older patch on a reported seam / seam_hits == 0)
        ──► devtools response PatchOutcome → Outcome::RestartRequired(reason)
no reply, transport error or timeout after apply_patch was sent
        ──► Outcome::RestartRequired(PatchOutcomeUnknown)
Outcome ──► CLI: println + kill/relaunch (run.rs:585-589)
        ──► TUI: Message::HotPatchOutcome → toast + restart_session_at (update.rs:2663)
```

Two timing rules sit under that flow (2.c). An L2 mismatch can surface frames after the
`PatchOutcome` of the patch that caused it. It is then caught before the next send by the host's
`hotpatch_info` read, or at the latest by the app's apply entry, which refuses with `applied: false`
while unreported records exist, so no patch is applied over a known mismatch. A fall-through is
judged by its missed key, not a count: a key in the newest patch is a benign patch-image caller,
and only a base or older-patch key on a reported seam instance requires a restart. The runtime must
therefore record the key (H0-01); the spike crate's `fall_through_count` stays a plain count and
does not feed this flow.

### 3.7 Debug-only gating, authentication, and the code-execution preconditions

A patch is arbitrary code execution in the app process. `Capability::HotPatch` (4.1) is advertised,
and its methods are dispatched, only when **all five** preconditions below hold for that process.
If any fails, the capability is absent, the session is restart-only, and the CLI/TUI says which
precondition failed.

1. **Debug builds only.**
   - `frust/hotpatch` is added only for `BuildMode::Debug` hot sessions. Profile inherits release
     (`crates/frust-drive/templates/app/Cargo.toml.tmpl:95-98`), so it has no `debug_assertions`,
     and frust-hotpatch consults its table only under `cfg!(debug_assertions)`
     (`HotFn::try_call`, `crates/frust-hotpatch/src/hot_fn.rs:219`).
   - The `HotPatch` capability and the three methods are compiled only under
     `cfg(all(debug_assertions, feature = "hotpatch"))`. A release build has neither the devtools
     listener (`devtools` is Debug/Profile only, `crates/frust-drive/src/build_info.rs:124-126`)
     nor the methods.
   - `frust_hotpatch::apply_patch` itself gains the `debug_assertions` gate that its README lists
     as follow-up work (`crates/frust-hotpatch/README.md`, "Debug-only, like subsecond").
   - The spike runner's devserver connection, under `cfg(all(debug_assertions, not(any(target_os
     = "android", target_os = "ios", target_arch = "wasm32"))))` (the cfg on the
     `connect_devserver()` call in `main`, `examples/hotpatch-spike/runner/src/main.rs:17-20`,
     repeated on `connect_devserver` and `apply_hot_patch`), is retired: there is no runner. The
     in-app devtools service is the only entry point.
2. **A per-session token with real entropy, on every host that advertises the capability.**
   - Both methods ride the existing per-process token handshake. Unauthenticated connections get
     `UNAUTHORIZED` for every method but `handshake`
     (`crates/frust-devtools-protocol/src/lib.rs:20-28`; enforced in
     `crates/frust-devtools/src/dispatch.rs:147-155`).
   - The token must come from the OS CSPRNG. Today that holds only on unix, and only when
     `/dev/urandom` is readable (`crates/frust-devtools/src/token.rs:22-26`, `:92-105`). Otherwise
     `generate` silently takes the non-CSPRNG fallback (`token.rs:51`, `:27-35`), and **Windows
     always takes it** (`token.rs:23-26`; `docs/LIMITATIONS.md:1318`,
     `devtools-token-entropy-windows-fallback`). That fallback is acceptable for reading a widget
     tree but not for loading code. So `token::generate` reports its source, and `HotPatch` is
     advertised only for an OS-sourced token (card H1-07). **Windows is excluded from
     `Capability::HotPatch`** until a `BCryptGenRandom` source lands (card H3-02), which is also
     when its PDB layout gate lands, and the Dell gate (H3-03) passes (section 5).
   - `ServiceConfig::require_token` must be on (`crates/frust-devtools/src/service.rs:36-43`). With
     it off there is no token (`service.rs:115`) and every connection starts authenticated
     (`dispatch.rs:49`), so `HotPatch` is never advertised in that mode.
3. **Loopback only.** The listener binds `127.0.0.1:0` and nothing else, not configurably
   (`crates/frust-devtools/src/service.rs:104-109`; trust model at `crates/frust-devtools/src/lib.rs:37-49`).
   The host reaches an Android app through `adb forward` (3.3). The host session sends a patch only
   to a loopback endpoint (card H1-08).
4. **Bytes over the authenticated channel, or the one checked loopback hand-off path.** The app
   loads **only bytes it received on that connection** (`patch_chunk`) or read through the loopback
   hand-off, and wrote, mode `0600`, into its own cache dir (4.3). `apply_patch`'s `JumpTableWire`
   has no `lib` field (4.1), and the app-side entry takes the path it wrote itself, never a
   client-supplied one. The only path on the wire is `ApplyPatchParams.file.path` (amended
   2026-10-08, R3-01): sent only on a loopback session to an app that advertises
   `HotpatchInfo.patch_file_hand_off` (unix shells), naming the patch the host already wrote with
   its SHA-256. The app reads it only when all five checks hold: opened `O_NOFOLLOW` and a regular
   file by `fstat`; owned by the app's effective uid; `mode & 0o077 == 0`; size equal to `len`;
   SHA-256 equal to `file.sha256`. It never logs the path. The threat model does not widen: an
   authenticated client can already send code as chunks, the digest binds the file to what the host
   linked, and the app refuses any file it does not own outright. A local process that guesses or
   plants a path therefore still cannot get code loaded.
5. **The patch matches this process and this connection.** `apply_patch` names a `patch_id` whose
   chunks arrived on the same connection, and the reassembled length must equal `len`. It also
   carries the `pid` and `anchor_runtime` the host built the stub against (from `hotpatch_info`);
   the app refuses a mismatch with its own values, and a missing anchor fails closed (2.f).

---

## 4. Transport

**Recommendation: two new frust-devtools-protocol methods plus a capability. Do not reuse dx's
websocket.**

### 4.1 The methods

| Method | Direction | Params | Result |
|---|---|---|---|
| `hotpatch_info` | client→server request | none | `{ anchor_runtime: u64, pid: u32, triple: String, patches_applied: u32, patch_bytes_loaded: u64, pending_layout_mismatches: Vec<String>, patch_file_hand_off: bool }` (L2 records not yet reported, 2.c; `patch_file_hand_off` defaults to `false` when absent) |
| `patch_chunk` | client→server request | `{ patch_id: u64, offset: u64, total_len: u64, data_base64: String }` (≤ 512 KiB raw per chunk) | `AckResult` |
| `apply_patch` | client→server request | `{ patch_id: u64, len: u64, pid: u32, anchor_runtime: u64, table: JumpTableWire, expected_seams: u32, file?: PatchFile }`, where `JumpTableWire` mirrors `crates/frust-hotpatch/src/jump_table.rs:15-33` minus `lib`. Without `file`, the bytes are `patch_id`'s chunks from this connection. With `file: { path: String, sha256: String }` (the loopback hand-off, only to an app advertising `patch_file_hand_off`), no chunk is sent and the app reads the host-written patch under 3.7 item 4's five checks (`O_NOFOLLOW` regular file, same euid, `mode & 0o077 == 0`, size `len`, SHA-256 match); a `patch_id` both uploaded and named by file is refused | `PatchOutcome { applied: bool, seam_hits: u64, seam_fall_throughs: Vec<MissedKey>, layout_mismatches: Vec<String>, patches_applied: u32, patch_bytes_loaded: u64 }` with `MissedKey { image: u32, link_address: u64 }` (2.c), sent **after the next frame**. While L2 records are unreported, the app refuses: `applied: false`, nothing loaded, the records in `layout_mismatches`. The backend hops to the UI thread the way existing calls do (`crates/frust-devtools/src/lib.rs:58-74`). |

- **New pieces.** `Capability::HotPatch` joins the handshake's capability list
  (`crates/frust-devtools-protocol/src/messages.rs:44-56`). The three methods join `Method`
  (`crates/frust-devtools-protocol/src/method.rs:14-42`).
- **The capability is conditional.** It is advertised only when the five preconditions of 3.7 hold
  (debug build, OS-CSPRNG token with `require_token` on, loopback bind, bytes or the checked loopback
  hand-off only, pid and anchor match). It is never advertised on Windows until H3-02 lands and H3-03 passes. The host sends nothing to an
  app that does not advertise it. While it is absent, `patch_chunk` and `apply_patch` answer
  `METHOD_NOT_FOUND`, exactly as on a build without the feature.
- **One patch at a time.** The host sends one patch per session and waits for its
  `PatchOutcome`, and r2-01 serialises `apply_patch` in the app. The accepted-layout merge (2.c L3)
  depends on that ordering.
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
   (`crates/frust-devtools/src/lib.rs:39-49`). A patch is arbitrary code execution, so frust goes
   further than the existing token. The capability carries its own preconditions (3.7): an
   OS-CSPRNG token on every host that advertises it (Windows excluded until `BCryptGenRandom`), a
   `127.0.0.1`-only bind, patch bytes over the authenticated connection and never a wire-supplied
   path, and debug builds only.
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

**The layout guarantee is DWARF-targets-only.** L3 (2.c) reads DWARF, and L2 is only a backstop. A
target applies patches only when its objects carry DWARF (Mach-O, ELF) **and** its devtools token
comes from an OS CSPRNG (3.7). Windows (MSVC) meets neither today, so it is **apply-disabled**: a
hot session there treats every build as `RestartRequired { BuilderUnsupported }`, and the app does
not advertise `Capability::HotPatch`.

| Target | In/out | Stage | Reason | Extra lines |
|---|---|---|---|---|
| macOS (aarch64/x86_64-apple-darwin) | **in** | 1 (milestone 1) | Phase 1 and Phase 2 measured here: 475-480 ms through subsecond, 575 ms through frust-hotpatch, 493 ms for the same-day control, vs 1570 ms restart. Mach-O + ld64 via `cc` (`dx:cli/src/build/link.rs:1252`). | 0 (it is the core) |
| Linux (x86_64/aarch64-unknown-linux-gnu) | **in** | 1 | ELF + `cc`/lld. dx's Gnu flavor is the same path as Android's (`dx:cli/src/build/link.rs:428-464`). The CI canary runs here (ubuntu runners, `.github/workflows/ci.yml:67-97`). | ~60 (Gnu thin/fat args, `--export-dynamic-symbol` anchor), already inside H1-03's 500 |
| Android arm64 (aarch64-linux-android) | **in** | 2 | Loading app-delivered code is proven (p2-02b). Needs: the fat build outside Gradle (3.3), the cdylib anchor (2.f), chunked delivery (4.3), and relocation still unproven. `frust-hotpatch` already cross-builds with cargo-ndk for arm64-v8a. 32-bit and x86_64 Android were not probed. | 450 |
| Windows (x86_64-pc-windows-msvc) | **in, apply-disabled until H3-02 lands and H3-03 passes** | 3 | PE/PDB stubs, `__imp_` data and `lld-link` args exist in dx (224 lines / 160 code, section 1.1). The L3 gate needs PDB type records, not DWARF, and the devtools token there is the non-CSPRNG fallback (`crates/frust-devtools/src/token.rs:23-26`). Until card H3-02 adds a PDB type-record gate and a `BCryptGenRandom` token source and H3-03 passes on the Dell, Windows is excluded from `Capability::HotPatch` and restarts on every build. L2 alone is never enough to enable it. Gate rig is the Dell. | 700 |
| iOS simulator (aarch64-apple-ios-sim) | **in, behind a probe** | 4 | `frust-hotpatch` builds and is clippy-clean for `aarch64-apple-ios-sim` (conductor check at 593d54d1). Loading on the simulator is **not probed**. Xcode links the Rust staticlib (`crates/frust-drive/templates/app/ios.tmpl/Runner.xcodeproj/project.pbxproj.tmpl:147`, `-l<name>` at `:247`). The fat link must be steered with command-line build settings (`OTHER_LDFLAGS=-Wl,-force_load,...`, `DEAD_CODE_STRIPPING=NO`, anchor export), and the symbol cache read from the Xcode-built executable. | 350 |
| iOS device | **out** | – | Code signing: a patch is an unsigned image that a device will not map executable. dx does not try either: no patch signing anywhere (`dx:cli/src/build/apple.rs:282-365` signs the bundle only), and Apple is simulator-only in the stub (`dx:cli/src/build/patch.rs:909-919`). | – |
| web (wasm32-unknown-unknown) | **out** | – | `frust-hotpatch` is native-only by `compile_error!` (`crates/frust-hotpatch/src/lib.rs:25-26`). wasm patching is a separate 799-line mechanism in dx (walrus/wasmparser, section 1.1 row 4w). | – |

---

## 6. Estimate and card list

### 6.1 New lines per component

| Component | Code | Tests | Card |
|---|---|---|---|
| frust-hotpatch: hit counter, missed-key recording, fail-closed anchor, `debug_assertions` gate on apply, safe devtools-facing entry that refuses while L2 records are unreported, `report_layout_mismatch` | 190 | 140 | H0-01 |
| frust-core seam v2 (`build_erased` + creator-image `SeamWitness` L2 guard in a `repr(C)` `ComponentWidget` with boxed State, checked in `rebuild`/`teardown`/`Drop`, leak on mismatch) + root build through the seam in `frust::app!` + anchor emission | 190 | 200 | H0-02 |
| frust-shell-desktop: `#[non_exhaustive]` `ShellUserEvent`, probe demoted | 40 | 30 | H0-03 |
| CI feature-on gate (script + workflow step) | 40 | – | H0-04 |
| frust-drive `hotpatch::capture` + linker interception + `frust` wrapper entry | 450 | 200 | H1-01 |
| `hotpatch::graph` + `hotpatch::replay` (tip lib, per-target, artifact notifications) | 450 | 250 | H1-02 |
| `hotpatch::fat_link` + `hotpatch::thin_link` (Darwin, Gnu) | 500 | 200 | H1-03 |
| `hotpatch::symbols` + `stub` + `jump_table` (Mach-O, ELF; aarch64, x86_64) | 550 | 250 | H1-04 |
| `hotpatch::layout` (DWARF L3 against the accepted-layout set) + `hotpatch::seams` (row D identity against the accepted seam set) | 500 | 350 | H1-05 |
| frust-devtools-protocol methods + capability | 150 | 120 | H1-06 |
| In-app apply: frust-devtools dispatch + token provenance + conditional capability + frust-shell-common backend + desktop wiring | 330 | 230 | H1-07 |
| `hotpatch::session` + accepted-set lifecycle + send preconditions (incl. pending L2 records) + missed-key classification + devtools client methods + budget | 570 | 370 | H1-08 |
| frust-cli `--watch` hot mode | 200 | 150 | H1-09 |
| frust-tui Watch hot path | 250 | 200 | H1-10 |
| CI canary (script + standalone fixture) | 200 | – | H1-12 |
| **Stage 1 subtotal (macOS + Linux)** | **4,610** | **2,690** | |
| Android (H2-01..03) | 450 | 200 | stage 2 |
| Windows (H3-01..03, incl. the PDB type-record gate and the token source) | 700 | 350 | stage 3 |
| iOS simulator (H4-02) | 350 | 150 | stage 4 |
| **All stages** | **6,110** | **3,390** | **≈ 9,500 lines** |

**Why stage 1 is ~2.3× dx's 2,023 code lines.** The L3 layout gate (500) has no dx counterpart.
Integration into existing CLI/TUI/devtools paths (session, protocol, in-app apply, CLI, TUI ≈
1,500) replaces dx's 249-line transport and its TUI. The tip-lib replay and per-crate-type capture
add ~150. Revision r2-03 added 170 code lines to stage 1 (creator-image witness, accepted-layout set,
capability preconditions) and 300 to stage 3 (PDB type-record gate, Windows token source).
Revision r2-04 adds 90 code and 110 test lines to stage 1: missed-key recording and the
fail-closed apply on pending L2 records (H0-01, +40 / +40), the `Drop` witness check, leak rule and
inline-container test (H0-02, +20 / +40), and their host side (H1-08, +30 / +30). The card count
is unchanged. This section is the authoritative sizing; RESULTS.md's Phase 2 conclusion quotes it.

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
| H0-00 | **Gate on milestone 1.** Row D3: measure a return-type-changing `build` edit (wrap the root `column()` in another container) with dx 0.7.10 on the two-package spike; record whether it no-ops, corrupts or crashes, with a guard-malloc leg. A behaviour the L1 + L3 argument (2.c) does not predict stops the plan at H0 for a redesign | `examples/hotpatch-spike/measure.sh`, `examples/hotpatch-spike/RESULTS.md` | medium | – | `bash -n examples/hotpatch-spike/measure.sh`; `PRE_RUN_PAUSE=20 POST_RUN_PAUSE=15 ./measure.sh --target return-type --runs 3` (GUI session); spike gate from `examples/hotpatch-spike`: `cargo build && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check` |
| H0-01 | frust-hotpatch (on top of r2-01): seam hit counter; missed-key recording (each distinct missed `HotFn` key as `(image, link_address)`, image 0 = base, n = n-th loaded patch; `fall_through_count` may stay as a count beside it, 2.c); app-owned ASLR anchor (`set_anchor`; patch anchor by name; apply refuses while no anchor is set), `debug_assertions` gate on `apply_patch`, safe `apply_from_devtools(bytes_path, table)` entry whose path is always the one the app wrote; `report_layout_mismatch(type, stored, own)` records kept until reported, readable for `hotpatch_info`, and the apply entry **refuses while any is unreported** (loads nothing, answers `applied: false` with the records, 2.c "When a mismatch surfaces") | `crates/frust-hotpatch/src/{lib.rs,patch.rs,hot_fn.rs,anchor.rs}`, `crates/frust-hotpatch/README.md` | medium | – | `cargo test -p frust-hotpatch`; `cargo clippy -p frust-hotpatch --all-targets -- -D warnings`; `cargo clippy -p frust-hotpatch --target aarch64-apple-ios-sim -- -D warnings`; `cargo ndk -t arm64-v8a build -p frust-hotpatch` |
| H0-02 | frust-core seam v2: `build_erased::<C>(c, s, witness)` erasing inside the hot fn (`// erasure: keep`); under the feature `ComponentWidget<C>` is `repr(C)` with `witness: SeamWitness` first and `state: ManuallyDrop<Box<C::State>>`; the witness is written in `ComponentView::build` and checked first in `rebuild`, `teardown` and `Drop`; on a mismatch `rebuild` is skipped, `teardown` does not proceed, `Drop` leaks the state box (no `drop_in_place`, no dealloc) but disposes the owner, and each calls `report_layout_mismatch` (2.c design (ii)); `frust::app!` routes the root build through the seam with the driver's own witness and emits/registers `__frust_hotpatch_anchor` | `crates/frust-core/src/{hotpatch.rs,component.rs,lib.rs}`, `crates/frust/src/lib.rs` | complex | H0-01 | `cargo test -p frust-core --features hotpatch` (incl. a nested-component test: a widget whose stored witness differs from the running code's is never rebuilt, its state is never touched, and a mismatch is reported; an **`Either` arm-swap test** (frust-core cannot depend on frust-widgets, so the test defines a two-arm view whose element holds the live arm inline, `Either`'s shape): a component held inline in one arm, its stored witness overwritten through a test-only hook to simulate another image, is swapped out by that view's rebuild, and its state is neither dropped nor freed (a drop-counting `State` records no drop), its owner is disposed and a mismatch is reported; and a test that `ComponentWidget<C>`'s size and field offsets are equal for two `C` with different `State` sizes); `cargo test -p frust-ui --features hotpatch`; `cargo clippy -p frust-core -p frust-ui --features hotpatch --all-targets -- -D warnings`; `scripts/ci/erasure-check.sh` |
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
| H1-05 | `hotpatch::layout` (L3 DWARF fingerprint tables: base at fat, candidate per thin, compared with the accepted-layout set = base + every accepted patch, merge on accept, diff → reasons) + `hotpatch::seams` (row D identity via demangled `build_erased` instances, compared with the accepted seam set); proves DWARF presence under the default dev `split-debuginfo` on macOS and Linux | `crates/frust-drive/src/hotpatch/{layout.rs,seams.rs}` | complex | H1-00 | `cargo test -p frust-drive hotpatch::layout` (fixtures: D2 field add, same-size reorder, closure capture change, return-type change, H0-00's observed D3 edit, **add a type in patch 1, change its layout in patch 2, expect `RestartRequired`**, a type dropped in patch 2 and re-added with another layout in patch 3, expect `RestartRequired`); `cargo test -p frust-drive hotpatch::seams` (fixture: a component added in patch 1 whose State identity changes in patch 2, expect `StateTypeChanged`) |
| H1-06 | Protocol: `hotpatch_info`, `patch_chunk`, `apply_patch` (with `pid` + `anchor_runtime`), `PatchOutcome`, `Capability::HotPatch`, whose doc states the five code-execution preconditions (3.7); no wire type carries a filesystem path (amended by R3-01: `ApplyPatchParams.file.path`, under 3.7 item 4's checks) | `crates/frust-devtools-protocol/src/{method.rs,messages.rs,lib.rs}` | medium | – | `cargo test -p frust-devtools-protocol` (incl. a serde test that `apply_patch` params and `JumpTableWire` reject or ignore a `lib`/path field) |
| H1-07 | In-app apply: frust-devtools dispatch + backend calls; `token::generate` reports its source, and `HotPatch` is advertised (and its methods dispatched) only for an OS-CSPRNG token with `require_token` on, in a `debug_assertions` + `hotpatch` build, never on Windows; frust-shell-common `ShellBackend` reassembles chunks per connection (size cap), checks `pid`/`anchor_runtime`, writes `0600` into the app cache dir, calls the safe frust-hotpatch entry with the path it wrote, replies after the next frame with hits, missed keys and mismatches (or with the entry's `applied: false` refusal while L2 records are unreported), and returns pending L2 records from `hotpatch_info`; facade feature chain | `crates/frust-devtools/src/{dispatch.rs,backend.rs,token.rs,service.rs}`, `crates/frust-shell-common/src/devtools.rs`, `crates/frust-shell-common/Cargo.toml`, `crates/frust-shell-desktop/src/app_handler.rs`, `crates/frust-shell-desktop/Cargo.toml`, `crates/frust/Cargo.toml` | complex | H0-02, H0-03, H1-06 | `cargo test -p frust-devtools`; `cargo test -p frust-shell-common --features devtools,hotpatch`; `cargo test -p frust-shell-desktop --features devtools,hotpatch`; `cargo clippy -p frust-shell-common -p frust-shell-desktop --features devtools,hotpatch --all-targets -- -D warnings` |
| H1-08 | `hotpatch::session`: fat build (3.2) → spawn exe → discovery + authenticated handshake → `HotPatch` capability present or restart-only session with the reason → `hotpatch_info` → on change: classify (d) → thin build → L3 + identity gates against the accepted sets → pending L2 records from `hotpatch_info` (non-empty → `RestartRequired { LayoutChanged }`, nothing sent) → chunked upload over the loopback/`adb forward` connection (bytes only, one patch in flight) + `apply_patch` → `Outcome`; accepted-set merge on `applied: true` only, `PatchOutcomeUnknown` on a lost reply, reset on every relaunch; missed keys classified by image (newest patch benign; a base or older-patch key on a reported seam instance → restart); patch budget (e); devtools client methods; `frust/hotpatch` feature for Debug hot sessions | `crates/frust-drive/src/hotpatch/session.rs`, `crates/frust-drive/src/devtools_client.rs`, `crates/frust-drive/src/desktop_run.rs`, `crates/frust-drive/src/build_info.rs` | complex | H1-02, H1-03, H1-04, H1-05, H1-07 | `cargo test -p frust-drive hotpatch::session` (fake runner + fake devtools server; cases: no `HotPatch` capability → nothing sent; non-loopback endpoint → nothing sent; merge only on `applied: true`; lost reply → `PatchOutcomeUnknown`; set reset after a relaunch; a late L2 mismatch pending in `hotpatch_info` after an `applied: true` outcome → `RestartRequired { LayoutChanged }`, nothing further sent; an `applied: false` refusal carrying records → `RestartRequired { LayoutChanged }`; missed keys only in the newest patch → `Patched`); `cargo test -p frust-drive devtools_client` |
| H1-09 | `frust run --watch` hot mode (default for debug desktop, `--no-hot` opt-out), watches all three path classes, prints outcomes, restart path on `RestartRequired` | `crates/frust-cli/src/commands/run.rs`, `crates/frust-cli/src/cli.rs` | medium | H1-08 | `cargo test -p frust-cli` |
| H1-10 | TUI: `Effect::HotPatch`, `Message::HotPatchOutcome`, toasts, restart fallback, palette rename; `R` stays a full restart | `crates/frust-tui/src/engine/{message.rs,update.rs,palette.rs}`, `crates/frust-tui/src/runner.rs`, `crates/frust-tui/src/supervise/{watch.rs,session.rs}` | complex | H1-08 | `cargo test -p frust-tui` |
| H1-11 | **Milestone 1 gate (macOS, GUI session):** stock `frust create` app (one package, template crate types) under `frust run --watch`: rows A, B, C (patched, State kept); D, D2 (nested component, root through the seam: the 2.c trace), D3, D4 (type added by patch 1, layout changed by patch 2), D5 (inline-container arm swap under a D2 edit: `HomePage` inside an `either(..)` arm, `HomeState` gains a field, then the arm swaps; with L3 on the host refuses it, and under guard-malloc no heap error appears, i.e. no non-creator drop; the in-app leak path itself is H0-02's test), E (each `restart required`, never `patched`); F (restart median, now through `frust run --watch`); G (10 patches + budget counters); H (one-package now patches); I (cold fat vs cold `cargo build`, one `target/`). Plus the TUI Watch leg. | `examples/hotpatch-spike/measure.sh`, `examples/hotpatch-spike/RESULTS.md` | complex | H0-00, H1-09, H1-10 | `bash -n examples/hotpatch-spike/measure.sh`; the matrix runs; pass bar: median save→frame ≤ 50% of row F, zero `patched` lines for D/D2/D3/D4/D5/E; row A's median recorded against the 493 ms estimate (section 7) |
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
| H3-02 | **Windows enablement preconditions.** A PDB type-record L3 gate (same entries and accepted-set rules as 2.c, read from the PDB's TPI stream) and an OS-CSPRNG devtools token on Windows (`BCryptGenRandom`, through a pinned crate or a sanctioned-unsafe entry per `docs/CODE_STANDARDS.md:14-23`, closing `devtools-token-entropy-windows-fallback`). Until both land, Windows hot sessions are apply-disabled and the app never advertises `HotPatch` | `crates/frust-drive/src/hotpatch/pdb_layout.rs`, `crates/frust-devtools/src/token.rs` | complex | H3-01, H1-05, H1-07 | `cargo test -p frust-drive hotpatch::pdb_layout` (on Windows CI; the H1-05 fixture set, two-patch case included); `cargo test -p frust-devtools token` on Windows (source reported as OS) |
| H3-03 | Windows gate on the Dell: rows A, B, D2, D4, G with the PDB gate active; Windows joins `Capability::HotPatch` only if it passes | `examples/hotpatch-spike/RESULTS.md` | medium | H3-02 | matrix on Windows; pass bar as H1-11 |

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
| D-04 | **Doc card.** LIMITATIONS: rewrite `no-hot-reload-restart-is-a-rebuild` (desktop hot patch; restart-required cases (c)-(e); the layout guarantee is DWARF-targets-only; Windows apply-disabled until H3-02/H3-03; iOS device and web out) | `docs/LIMITATIONS.md` | simple | H1-11 | budgets |
| H5-01 | *Optional.* Path-dependency replay (2.d deferred design) | `crates/frust-drive/src/hotpatch/{capture.rs,graph.rs,replay.rs,layout.rs}` | complex | H1-11 data | `cargo test -p frust-drive hotpatch::` |
| H5-02 | *Optional.* rlib-only replay (2.b item 3) with a before/after thin-build measurement | `crates/frust-drive/src/hotpatch/replay.rs` | medium | H1-11 | `cargo test -p frust-drive hotpatch::replay`; timing table in RESULTS.md |

**Card counts.**

| Stage | Implementor | Doc | Gate / probe | Optional |
|---|---|---|---|---|
| 1 | 19 (H0-00..05, H1-00..12) | 4 (D-01..04) | – | – |
| 2 | 4 | – | – | – |
| 3 | 4 | – | – | – |
| 4 | 3 | – | – | – |
| H5 | – | – | – | 2 |

The stage-1 implementor count includes H0-00, H1-11 and H1-12, which are measurement/gate cards.
The list holds **36** cards: round 0 had 35, and r2-03 split Windows enablement (H3-02) from the
Dell gate (now H3-03).

### 6.3 New pins and their Version-Pin Policy rows

| Pin | Version | Row owner (per `docs/DEVELOPMENT.md:400-408`) | Rationale for the row | Tripwire |
|---|---|---|---|---|
| `object` | 0.37.x (dx 0.7.10 uses 0.37.1, `dx:Cargo.toml:313`) | CLI_DEVELOPMENT.md | Stub-object writer and symbol reader; the Mach-O/ELF writer API is the part toolchain/format drift touches | `cargo test -p frust-drive hotpatch::` + canary |
| `ar` | 0.9.0 (`dx:cli/Cargo.toml:155`) | CLI_DEVELOPMENT.md | Reads rlib members, writes the fat archive | same |
| `gimli` | chosen in H1-00, compatible with the `object` pin | CLI_DEVELOPMENT.md | DWARF reader for the L3 gate | `cargo test -p frust-drive hotpatch::layout` |
| `rustc-demangle` | chosen in H1-00 | CLI_DEVELOPMENT.md | v0 demangling for the row-D identity gate | `cargo test -p frust-drive hotpatch::seams` |
| `pdb` | 0.8.0 (`dx:cli/Cargo.toml:156`), stage 3 | CLI_DEVELOPMENT.md | PE symbol cache; PDB type records for the Windows L3 gate (H3-02) | Windows CI `hotpatch::pe` + `hotpatch::pdb_layout` |
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

**Recommendation in one sentence:** build STAGED, desktop first. **Milestone 1** is H0-00 run and
H1-11 passing: on macOS, with Linux covered by the CI canary, `frust run --watch` and the TUI's
Watch hot-patch an unmodified one-package `frust create` app with State kept, every hazard row (D,
D2, D3, D4, D5, E) answers `restart required` and never `patched`, and the median save→frame is at
most 50% of the restart median.

**What makes the hazard rows safe: L1 + L3, on DWARF targets.**
- L1 removes the return slot but leaves the retained tree (2.c). It shrinks the D2/D3 hazard to the
  retained tree; it does not eliminate it.
- D2, D4 and D5 are closed by L3 against the accepted-layout set. L2's creator-image witness is an
  in-app backstop for State size changes of components hosted in their own erased box, with a
  leak-on-mismatch rule for every host (2.c design (ii)); it is not part of the soundness argument.
- D3 is closed only if L3 covers it **and** H0-00 confirms the mechanism. That is why H0-00 gates
  milestone 1 instead of following it.
- The guarantee is DWARF-targets-only. Windows applies no patch until H3-02 and H3-03 (section 5).

**Latency: an estimate, to be confirmed by H1-11.**
- Measured: Phase 1, 475 / 480 ms through subsecond; Phase 2, 575 / 577 ms through frust-hotpatch;
  the same-day control on the unmodified base (A-ctl), **493 ms**; restart, 1570 ms (RESULTS.md
  rows A and F, "Phase 2: frust-hotpatch runtime").
- Round 0 said the frust builder "should land under Phase 1's figure". That compared against
  another day's numbers. The baseline is the same-day control: the subsecond runtime on the same
  machine and day as the 575 ms rows. RESULTS.md attributes the ~80 ms between them to the runner
  tip recompiling `connect(callback)` and the serde round-trip, and the frust builder has neither
  (the connection lives in the devtools service, 4.2).
- **Post-fix estimate: ~493 ms save→frame, about 31% of the restart median.** It assumes the tip
  compile that requirement (a) removes (104-106 ms in the control) is roughly used up by costs the
  frust builder adds and nothing has measured yet: the L3 DWARF read and comparison over the
  replayed objects, the identity demangle, ~2 MB of base64 in three `patch_chunk` round trips
  (4.1), and a `PatchOutcome` that waits for the next frame. The figure therefore includes those
  new gate and transport costs as assumptions, not as data. H1-11 records the real median against
  it. The pass bar (50% of 1570 ms = 785 ms) leaves ~290 ms for the estimate to be wrong.
- The first patch of a session took 1-2 s in every Phase 1/2 session (RESULTS.md, Surprises).
  Nothing here changes that.

**Why desktop first, stated without overreach.** Round 0 said desktop was "proven end-to-end
except for the two builder fixes" and had no unproven links. That overstated it. What ran on this
Mac, through dx: capture, replay of a non-tip lib package, stub, jump table, frust-hotpatch load
and dispatch, and State preserved across 10 patches (rows A-C and G; Phase 2 rows A and B). What
the frust builder replaces or adds on desktop has not run anywhere. Those are milestone-1 risks.

**Milestone-1 risks: unmeasured desktop dependencies.**
1. **A patch built only from the tip lib's objects.** Requirement (a) replays the one-package tip
   lib, and 2.a has the patch need no bin objects at all. Row H was cited from two research
   artifacts, not re-run (RESULTS.md, row H), and no row has linked a patch from a tip package's
   lib objects alone. First evidence: the H1-02 and H1-03 fixtures, then H1-11 row H.
2. **The app-owned, non-`main` anchor (2.f).** It applies to desktop too: every measured desktop row
   anchored on `main` (`apply_patch_with_anchor`, `crates/frust-hotpatch/src/patch.rs:165-208`).
   No target has run with `__frust_hotpatch_anchor`. First evidence: H0-01's tests, then H1-12's
   canary.
3. **The gates themselves.** L3 needs DWARF under the default `split-debuginfo` (H1-05). Its
   per-build cost is folded into the 493 ms estimate, and its false-positive rate on ordinary edits
   is unknown. The identity gate depends on demangling. L2's `repr(C)` `ComponentWidget` with boxed
   State and a leak-on-mismatch `Drop` (H0-02) changes the hot-patch build's layout and
   allocations. The accepted-set lifecycle (H1-08) is new.
4. **H0-00's outcome.** If D3 does not behave as 2.c predicts, the plan stops at phase H0.
5. **The security preconditions (3.7).** Token provenance and the conditional capability are new
   code in frust-devtools (H1-07). A defect there fails safe only if the capability is absent by
   default, which H1-07's tests must show.

**Android has more unproven links than desktop.** p2-02b proved *loading* only, with a
self-contained library ("nothing in it is relocated against the running app library",
RESULTS.md). On top of the desktop risks above, Android needs:
1. a fat cdylib built outside Gradle (3.3);
2. the non-`main` anchor in a JNI-loaded cdylib, where it is mandatory rather than a choice (2.f);
3. stub relocation against a base `.so` loaded by the JNI loader rather than an executable.

FULL would put all of these on the critical path of the first usable feature. STAGED ships the
desktop value after stage 1, retires desktop risks 1-3 on the cheaper rig first, and opens stage 2
with the relocation proof (H2-04) as its first gate.

**Stage order after milestone 1.**
1. **Android** (stage 2): mobile is frust's primary target, and loading is already proven there.
2. **Windows** (stage 3): the mechanics exist in dx and a rig is available. Windows stays
   apply-disabled until H3-02 adds the PDB type-record gate and an OS-CSPRNG token, and H3-03
   passes.
3. **iOS simulator** (stage 4): behind a load probe, because Xcode owns the link.
