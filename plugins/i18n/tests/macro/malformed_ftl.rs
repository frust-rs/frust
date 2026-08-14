// A malformed `.ftl` fails the build, naming the file, line, and column.

frust_i18n::locales!("fixtures/malformed");

fn main() {}
