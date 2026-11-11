# The fallback locale of the `locales!` macro's happy-path fixture: it owns
# every message the generated `keys` module is built from.

greeting = Hello, { $name }!

cart-items =
    { $count ->
        [one] one item
       *[other] { $count } items
    }

editor-notice = Working { $working } of { $full }

only-en = English only

unused-pair = Unused { $left } and { $right }

pass-through = Passed { $args }

save = Save
    .tooltip = Save this document
