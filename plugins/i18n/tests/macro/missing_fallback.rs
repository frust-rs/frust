// No `en` directory and no explicit `fallback:` — the macro refuses to guess
// which locale is authoritative.

frust_i18n::locales!("fixtures/no-fallback");

fn main() {}
