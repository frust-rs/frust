# Bundled Glyph fonts (task 18)

Raw TTF bytes embedded into `frust-theme` via `include_bytes!`
(`crates/frust-theme/src/glyph/fonts.rs`), gated behind the default-on
`glyph-fonts` Cargo feature. Both families are OFL-1.1; the full license
text is vendored per family (`space-mono/OFL.txt`, `ibm-plex-mono/OFL.txt`)
and neither family's reserved font name ("Space Mono", "Plex") has been
modified.

This crate stays pure-data: these bytes are only ever returned by
`glyph::fonts::font_data()`. Registering them into a live
`frust_text::TextContext` is a shell's job (task 27), via the task 08
pending-font registry.

## Space Mono

- **Upstream**: `github.com/googlefonts/spacemono`
- **Commit**: `329858c2c4dbd3476f972a4ae00624b018cf4b81` (`main`, "Merge pull
  request #12 from emmamarichal/main — Update Space Mono", 2025-01-17)
- **Retrieved**: 2026-07-23
- **License**: OFL-1.1, `github.com/googlefonts/spacemono/blob/main/OFL.txt`
- **Faces bundled** (3 of the family's 4 upstream faces — Bold Italic
  omitted, not needed by the Glyph type scale):

  | File | Source URL | Size |
  |---|---|---|
  | `space-mono/SpaceMono-Regular.ttf` | `raw.githubusercontent.com/googlefonts/spacemono/main/fonts/ttf/SpaceMono-Regular.ttf` | 99,356 B |
  | `space-mono/SpaceMono-Bold.ttf` | `raw.githubusercontent.com/googlefonts/spacemono/main/fonts/ttf/SpaceMono-Bold.ttf` | 98,232 B |
  | `space-mono/SpaceMono-Italic.ttf` | `raw.githubusercontent.com/googlefonts/spacemono/main/fonts/ttf/SpaceMono-Italic.ttf` | 121,396 B |

  Subtotal: 318,984 B (~311.5 KiB).

## IBM Plex Mono

- **Upstream**: `github.com/IBM/plex`
- **Release tag**: `@ibm/plex-mono@2.5.0` (published 2026-06-11)
- **Commit**: `2f9ba1b25957d958db71a849e85d72e3ecfb845a` (the commit the
  annotated release tag points to)
- **Retrieved**: 2026-07-23
- **License**: OFL-1.1, `ibm-plex-mono/LICENSE.txt` inside the release asset
  (`github.com/IBM/plex/releases/download/%40ibm/plex-mono%402.5.0/ibm-plex-mono.zip`)
- **Faces bundled** (4 of the family's much larger weight range — Regular/
  Medium/SemiBold/Italic only, matching the task's scope):

  | File | Source (within `ibm-plex-mono.zip`) | Size |
  |---|---|---|
  | `ibm-plex-mono/IBMPlexMono-Regular.ttf` | `fonts/complete/ttf/IBMPlexMono-Regular.ttf` | 173,052 B |
  | `ibm-plex-mono/IBMPlexMono-Medium.ttf` | `fonts/complete/ttf/IBMPlexMono-Medium.ttf` | 174,008 B |
  | `ibm-plex-mono/IBMPlexMono-SemiBold.ttf` | `fonts/complete/ttf/IBMPlexMono-SemiBold.ttf` | 174,608 B |
  | `ibm-plex-mono/IBMPlexMono-Italic.ttf` | `fonts/complete/ttf/IBMPlexMono-Italic.ttf` | 179,756 B |

  Subtotal: 701,424 B (~685.0 KiB).

## Total

**7 faces, 1,020,408 B (~996.5 KiB / ~0.97 MiB).** This is over the task's
~800KB soft target — IBM Plex Mono's "complete" TTF build is the hinted,
full-glyph-set release artifact (no pre-subsetted TTF variant is published
upstream; only smaller OTF and unicode-range-split WOFF/WOFF2 exist, neither
of which match the TTF format this task specified). Subsetting to a smaller
glyph range (e.g. Latin-1 + common punctuation) is a documented **deferred
optimization**, not part of this task — see task 18's Notes.
