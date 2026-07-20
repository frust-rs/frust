# Huddle — feature-slice convention

Huddle follows the clean-architecture feature-slice convention the sibling
`clean-signals-rs` docs prescribe: each feature under `src/features/<name>/`
splits into `domain/` (entities, repository traits, use cases —
framework-free), `data/` (repository impls over the shared `src/data::store`
dataset), and `presentation/` (controllers + pages, the only layer that talks
to `frust`). Dependencies point inward only (`presentation` → `domain` ←
`data`); a presentation file never reaches into a `data` module directly, and
a domain file never mentions `frust::`, `::presentation::`, or `::data::`.

`../clean-signals-rs/examples/team-demo` is the reference implementation —
"when in doubt, imitate it." `tests/architecture.rs` is the automated
enforcement of the layering rules above (a plain source scan, run as a normal
`cargo test`); consult it for the exact rules and any documented, file-scoped
exemption before adding a new cross-layer import.
