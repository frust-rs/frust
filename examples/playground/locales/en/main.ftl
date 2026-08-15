# Frust playground — i18n showcase messages, English (the `locales!` macro's
# default fallback locale, per `pages/i18n.rs`'s module doc). Every message
# here is covered by `pages/i18n.rs`'s typed `keys::` call sites, and exists
# to exercise one shape: `hello` interpolates a variable, `cart-items` is a
# CLDR plural, `theme-choice` is a plain Fluent select expression (selecting
# on a literal string, not a plural category), and `order-total`/`last-visit`
# each carry an ICU-backed NUMBER/DATETIME placeable (registered via
# `frust_i18n::fmt::with_icu_functions` — see `crate::setup_i18n`).

hello = Hello, { $name }!

cart-items =
    { $count ->
        [0] Your cart is empty
        [one] { $count } item in your cart
       *[other] { $count } items in your cart
    }

theme-choice =
    { $theme ->
        [light] Light theme selected
        [dark] Dark theme selected
       *[other] System theme selected
    }

order-total = Order total: { NUMBER($amount, style: "currency", currency: "EUR") }

last-visit = Last visit: { DATETIME($when, dateStyle: "medium") }
