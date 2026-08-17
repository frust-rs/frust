# Bundled shadcn fonts

Raw variable-TTF bytes embedded into `frust-shadcn` via `include_bytes!`
(`plugins/shadcn/src/tokens/fonts.rs`), unconditionally — the design-system
plugin *is* the shadcn opt-in, so the bytes carry no Cargo feature of their
own (the `plugins/glyph/fonts/` precedent).

shadcn/ui itself mandates no typeface; the pairing bundled here is the one its
official `next-template` ships — **Inter** for sans text and **JetBrains Mono**
for monospace. Both families are SIL OFL-1.1 and — verified against each
release's own license text — declare **no Reserved Font Name** (the copyright
lines carry no "with Reserved Font Name" clause), so the unmodified faces ship
here under their original names with the full license text vendored per family
(`inter/OFL.txt`, `jetbrains-mono/OFL.txt`), as the license's
license-inclusion term requires. Neither file's bytes are modified.

These bytes are pure data: they are only ever returned by
`frust_shadcn::font_data()`. Registering them into a live `TextContext` is a
shell's job — `frust_shadcn::install()` pushes them through the facade's
`register_app_fonts` pending-font queue for the running shell to drain.

## Inter Variable

- **Upstream**: `github.com/rsms/inter`
- **Release tag**: `v4.1` (the repository's latest release as of retrieval)
- **Retrieved**: 2026-08-17
- **Release asset**: `github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip`
  (SHA-256 `9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e`)
- **License**: OFL-1.1, `LICENSE.txt` inside the release asset, vendored here as
  `inter/OFL.txt`
- **Faces bundled** (1 of the asset's 3 TTF artifacts — the upright variable
  face only; `InterVariable-Italic.ttf` and the 13 MB `Inter.ttc` static
  collection are omitted, since no shadcn component asks for italic):

  | File | Source (within `Inter-4.1.zip`) | Family name (`name` ID 1) | Size |
  |---|---|---|---|
  | `inter/InterVariable.ttf` | `InterVariable.ttf` | `Inter Variable` | 879,708 B |

  SHA-256 `4989b125924991b90d05b2d16e0e388c48f7d5bb8b30539bbf9c755278d0ccaf`.
  Subtotal: 879,708 B (~859.1 KiB).

## JetBrains Mono Variable

- **Upstream**: `github.com/JetBrains/JetBrainsMono`
- **Release tag**: `v2.304` (the repository's latest release as of retrieval)
- **Retrieved**: 2026-08-17
- **Release asset**:
  `github.com/JetBrains/JetBrainsMono/releases/download/v2.304/JetBrainsMono-2.304.zip`
  (SHA-256 `6f6376c6ed2960ea8a963cd7387ec9d76e3f629125bc33d1fdcd7eb7012f7bbf`)
- **License**: OFL-1.1, `OFL.txt` inside the release asset, vendored here as
  `jetbrains-mono/OFL.txt`
- **Faces bundled** (1 of the asset's 2 variable faces — the upright one; the
  italic variable face is omitted for the same reason as Inter's):

  | File | Source (within `JetBrainsMono-2.304.zip`) | Family name (`name` ID 1) | Size |
  |---|---|---|---|
  | `jetbrains-mono/JetBrainsMono-Variable.ttf` | `fonts/variable/JetBrainsMono[wght].ttf` | `JetBrains Mono` | 303,144 B |

  SHA-256 `662a196d58f1183bf2d77428b6d5283fe3f45161ab021bea4036bc98e5cac016`.
  The upstream filename carries brackets (`JetBrainsMono[wght].ttf`); the file
  is **renamed** here (bytes untouched) so no tool in the build path has to
  handle a bracketed path. Subtotal: 303,144 B (~296.0 KiB).

## Total

**2 faces, 1,182,852 B (~1.13 MiB)** of font bytes, plus 8,779 B of license
text (`1,191,631 B` for the whole directory). Both faces are *variable* fonts,
so this one pair covers the whole weight range the catalog names (Regular
through Bold) instead of one file per weight — which is why two files land in
roughly the same budget seven static Glyph faces did. Subsetting to a narrower
glyph range is a documented **deferred optimization**, as it is for Glyph.
