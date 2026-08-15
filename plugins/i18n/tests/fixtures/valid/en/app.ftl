# The fallback locale of the `locales!` macro's happy-path fixture: it owns
# every message the generated `keys` module is built from.

greeting = Hello, { $name }!

cart-items =
    { $count ->
        [one] one item
       *[other] { $count } items
    }

only-en = English only

save = Save
    .tooltip = Save this document
