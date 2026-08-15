# A deliberately incomplete translation: `only-en` and `save` are absent, so
# the macro reports them as fallback-served and the runtime round-trip test
# can prove the chain actually falls back.

greeting = Hallo, { $name }!

cart-items =
    { $count ->
        [one] ein Artikel
       *[other] { $count } Artikel
    }
