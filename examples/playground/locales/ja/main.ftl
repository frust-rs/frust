# Frust playground — i18n showcase messages, Japanese. A complete translation
# of `locales/en/main.ftl` (same five message ids, same `$variable`s) — see
# `pages/i18n.rs`'s module doc for what each message exercises.
#
# `cart-items` has no `[one]` variant: Japanese's CLDR cardinal plural rule
# classifies every count as `other`, so `[one]` would be dead code here (never
# selected) — the `[0]` exact-value match still applies, since it selects on
# the literal number, not a plural category.

hello = { $name }さん、こんにちは!

cart-items =
    { $count ->
        [0] カートは空です
       *[other] カート内の商品: { $count } 点
    }

theme-choice =
    { $theme ->
        [light] ライトテーマを選択しました
        [dark] ダークテーマを選択しました
       *[other] システムテーマを選択しました
    }

order-total = 合計: { NUMBER($amount, style: "currency", currency: "EUR") }

last-visit = 最終訪問: { DATETIME($when, dateStyle: "medium") }
