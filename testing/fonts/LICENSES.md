# Bundled Test Fonts — Licenses and Provenance

Every font under `testing/fonts/` is permissively licensed (SIL OFL 1.1 or Apache License
2.0), subsetted with `pyftsubset` (fonttools) down to only the codepoints the acceptance
test in `crates/frust-testing/src/fonts.rs` shapes: `'Hello, مرحبا, 日本語, é̂ , 😀'`
(Latin ASCII + U+00E9/U+0302 combining marks, the Arabic word مرحبا, the CJK word 日本語,
and the U+1F600 grinning-face emoji). No system font is consulted by that test — these are
the *only* fonts registered into its `TextContext`.

Total bundle size: 56 KB (`du -sh testing/fonts` on this checkout), far under the 4 MB cap.

## Files

| File | Script/Purpose | Upstream URL | SHA-256 (subsetted file, as bundled) |
|------|-----------------|--------------|----------------------------------------|
| `NotoSans-Subset.ttf` | Latin sans + combining marks (`Hello`, `é`, U+0302) | https://raw.githubusercontent.com/notofonts/notofonts.github.io/main/fonts/NotoSans/hinted/ttf/NotoSans-Regular.ttf | `f9ff072c524484c5a6f346bc493b89f06ec15a4db855796041319be07d8dd528` |
| `NotoSansArabic-Subset.ttf` | Arabic RTL + joining (`مرحبا`) | https://raw.githubusercontent.com/notofonts/notofonts.github.io/main/fonts/NotoSansArabic/hinted/ttf/NotoSansArabic-Regular.ttf | `7b32dd87900309cb7dfd1360f4c1341b8f32b789ce56b460db426dae3d94db16` |
| `NotoSansJP-Subset.otf` | CJK (`日本語`) | https://raw.githubusercontent.com/notofonts/noto-cjk/main/Sans/SubsetOTF/JP/NotoSansJP-Regular.otf | `6e575a05281f779b88658bc8dae06364dffb1aa1ae63175ce660325654bed660` |
| `NotoEmoji-COLRv1-Subset.ttf` | COLRv1 colour emoji (`😀`, U+1F600) | https://raw.githubusercontent.com/googlefonts/color-fonts/main/fonts/noto_noflags-glyf_colr_1.ttf | `7badb39593ac1871c57a54de53e73a862e9497f61ddd343dc9b5b333e0d716e6` |

Upstream (pre-subset) file identity, for provenance/reproducibility of the subset commands
below:

| Upstream file | SHA-256 (as fetched, before subsetting) |
|---|---|
| `NotoSans-Regular.ttf` | `478c558ea716033cd60c03438f628dfa75694dcf6b5f6d505a2f05fd2b4f3823` |
| `NotoSansArabic-Regular.ttf` | `bdff3e5659d67e67def05b33f749683b9376ae819d65d3dd62ac4640b3aaef48` |
| `NotoSansJP-Regular.otf` | `dff723ba59d57d136764a04b9b2d03205544f7cd785a711442d6d2d085ac5073` |
| `noto_noflags-glyf_colr_1.ttf` | `d4080b4e69f9d7b3fc71b418eb4eb7afae69eaec04b98bfd46477ec4fdc02829` |

## Subset commands

Run with `fonttools` 4.63.0 (`pyftsubset`). Every command keeps `--layout-features='*'` so
the Arabic file retains its `init`/`medi`/`fina`/`isol`/joining GSUB rules (required for
correct joined shaping, not just isolated glyph presence).

```bash
pyftsubset NotoSans-Regular.ttf --output-file=NotoSans-Subset.ttf \
  --unicodes="U+0020,U+002C,U+0048,U+0065,U+006C,U+006F,U+00E9,U+0302" \
  --layout-features='*' --glyph-names --symbol-cmap --legacy-cmap \
  --notdef-glyph --notdef-outline --recommended-glyphs \
  --name-IDs='*' --name-legacy --name-languages='*'

pyftsubset NotoSansArabic-Regular.ttf --output-file=NotoSansArabic-Subset.ttf \
  --unicodes="U+0020,U+002C,U+0627,U+0628,U+062D,U+0631,U+0645" \
  --layout-features='*' --glyph-names --symbol-cmap --legacy-cmap \
  --notdef-glyph --notdef-outline --recommended-glyphs \
  --name-IDs='*' --name-legacy --name-languages='*'

pyftsubset NotoSansJP-Regular.otf --output-file=NotoSansJP-Subset.otf \
  --unicodes="U+0020,U+002C,U+65E5,U+672C,U+8A9E" \
  --layout-features='*' --glyph-names \
  --notdef-glyph --notdef-outline --recommended-glyphs \
  --name-IDs='*' --name-legacy --name-languages='*'

pyftsubset noto_noflags-glyf_colr_1.ttf --output-file=NotoEmoji-COLRv1-Subset-unrenamed.ttf \
  --unicodes=1F600 \
  --layout-features='*' \
  --glyph-names --symbol-cmap --legacy-cmap --notdef-glyph --notdef-outline \
  --recommended-glyphs --name-IDs='*' --name-legacy --name-languages='*'
```

The subsetted emoji face's `name` table records were then rewritten (family/full/PostScript
name records, Windows and Mac platforms) from the test-fixture-derived
`noto_noflags-glyf_colr_1` to `Frust Test Emoji COLR` via a short `fontTools` script — a
purely cosmetic rename (this repo's `TextContext::register_fonts` test fixture precedent,
`crates/frust-text/tests/fonts/Tuffy-As-Helvetica.ttf`, does the same for a deterministic,
readable resolved-family-name assertion). No glyph outline, `COLR`/`CPAL`, `cmap`, or metrics
data was touched. Per Apache License 2.0 §4(b) ("You must cause any modified files to carry
prominent notices stating that You changed the files"): **this file was changed** from the
upstream `noto_noflags-glyf_colr_1.ttf` build only in its `name` table, as described here.

```python
from fontTools.ttLib import TTFont
f = TTFont("NotoEmoji-COLRv1-Subset-unrenamed.ttf")
nm = f["name"]
for name_id, value in ((1, "Frust Test Emoji COLR"), (2, "Regular"),
                       (4, "Frust Test Emoji COLR Regular"),
                       (6, "FrustTestEmojiCOLR-Regular"),
                       (16, "Frust Test Emoji COLR")):
    nm.setName(value, name_id, 3, 1, 0x409)  # Windows, Unicode BMP, en-US
    if name_id != 16:
        nm.setName(value, name_id, 1, 0, 0)  # Macintosh, Roman, English
f.save("NotoEmoji-COLRv1-Subset.ttf")
```

## Copyright and license text

### `NotoSans-Subset.ttf`, `NotoSansArabic-Subset.ttf`, `NotoSansJP-Subset.otf`

Copyright 2022 The Noto Project Authors (Latin: https://github.com/notofonts/latin-greek-cyrillic;
Arabic: https://github.com/notofonts/arabic); Noto Sans JP additionally carries
`© 2014-2021 Adobe (http://www.adobe.com/)` in its embedded `name` table (Source Han Sans
lineage, distributed as Noto Sans CJK/JP by the Noto Project — see
https://github.com/notofonts/noto-cjk). All three are licensed under the SIL Open Font
License, Version 1.1, full text below.

```
-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

### `NotoEmoji-COLRv1-Subset.ttf`

Built by the `googlefonts/color-fonts` project (https://github.com/googlefonts/color-fonts)
from Noto Emoji glyph source data into the `glyf_colr_1` (COLRv1) test-fixture flavor
(`fonts/noto_noflags-glyf_colr_1.ttf`); the whole repository, including this generated
binary, is distributed under the Apache License, Version 2.0. Chosen (per this task's own
acceptance criteria, which name it as the sanctioned alternative to a Noto Color Emoji COLR
subset) because Google's canonical `NotoColorEmoji.ttf` release ships as a `CBDT`/`CBLC`
bitmap-strike font, not COLRv1 — this file is an actual COLRv1 (`COLR`+`CPAL`+`glyf`) face
carrying the U+1F600 grinning-face glyph, needed to exercise a colour-glyph shaping path.

```
                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS
```
