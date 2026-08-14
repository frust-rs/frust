// A directory name that isn't a BCP-47 identifier (the POSIX `en_US` spelling)
// fails the build rather than registering a locale nothing can negotiate to.

frust_i18n::locales!("fixtures/bad-locale-name");

fn main() {}
