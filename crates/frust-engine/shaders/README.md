# Frust Engine Shaders

All engine WGSL shader code lives in this directory as separate `.wgsl` files.

## Guidelines

- **No inline shader strings:** Engine shaders must be loaded from files in this directory via `include_str!()`, never hardcoded as string literals in Rust source.
- **Lint compliance:** All shaders in this directory are linted by the `crates/frust-gpu/tests/downlevel_rules.rs` test harness.
- **Isolation:** Shaders belong to the engine rendering pipeline and must not duplicate or conflict with shaders in other crates.

## Structure

Shaders are organized by feature or rendering pass. Each `.wgsl` file should have:

- A clear header comment describing its purpose (compute, fragment, vertex, etc.)
- Layout and binding declarations aligned with GPU-side data structures in `src/`.
- Comments explaining complex logic or GPU-specific constraints.

## Loading

Use `include_str!()` to embed shader files at compile time:

```rust
const MY_SHADER: &str = include_str!("my_shader.wgsl");
```

This approach ensures shaders are:
1. Type-checked at compile time
2. Bundled statically (no runtime file I/O)
3. Part of the release binary
