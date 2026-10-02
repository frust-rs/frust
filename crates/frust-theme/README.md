# frust-theme

`frust-theme` holds Frust's design tokens: the `Theme` aggregate and its token groups (color, typography, shape, elevation, motion, glass, status), plus the `ThemeBuilder` and `ThemeExtensions` seams a design system composes its own token set through. It contains no design language of its own; `Theme::neutral` is the language-free baseline.

Applications do not normally depend on this crate directly; they reach it through the `frust` facade crate.

- Website: https://frust.dev
- Repository: https://github.com/frust-rs/frust

## License

Licensed under either of Apache License, Version 2.0 (`LICENSE-APACHE`) or MIT license (`LICENSE-MIT`) at your option (SPDX: `MIT OR Apache-2.0`).
