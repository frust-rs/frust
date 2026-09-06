# Frust playground — i18n showcase messages, German. A complete translation
# of `locales/en/main.ftl` (same five message ids, same `$variable`s), not a
# half-translated fixture — see `pages/i18n.rs`'s module doc for what each
# message exercises.

hello = Hallo, { $name }!

cart-items =
    { $count ->
        [0] Ihr Warenkorb ist leer
        [one] { $count } Artikel im Warenkorb
       *[other] { $count } Artikel im Warenkorb
    }

theme-choice =
    { $theme ->
        [light] Helles Design gewählt
        [dark] Dunkles Design gewählt
       *[other] Systemdesign gewählt
    }

order-total = Gesamtsumme: { NUMBER($amount, style: "currency", currency: "EUR") }

last-visit = Letzter Besuch: { DATETIME($when, dateStyle: "medium") }
