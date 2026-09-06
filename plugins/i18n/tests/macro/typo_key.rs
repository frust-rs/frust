// A typo'd key is a missing function: the whole point of the typed surface.

frust_i18n::locales!("fixtures/valid");

fn main() {
    let chain = engine().negotiate(&["en".parse().expect("valid locale")]);

    println!("{}", keys::greting(&engine().with_chain(&chain), "Ada"));
}
