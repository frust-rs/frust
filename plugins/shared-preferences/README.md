# frust-shared-preferences

A platform-independent, synchronous key-value store for Frust apps, shaped
like Flutter's `shared_preferences`. `SharedPreferences::standard()` opens the
platform store: `NSUserDefaults` on iOS and macOS, Android `SharedPreferences`,
and a JSON file in the app's data directory on Linux and Windows. Five value
types are supported: `bool`, `i64`, `f64`, `String` and `Vec<String>`.

**Platform support:** Android, iOS, macOS, Linux and Windows. There is no web
backend. Each backend uses its own storage encoding, so stored data is not
compatible with Flutter's.

Like every Frust platform plugin, applications depend on this crate directly
in their own `Cargo.toml`, alongside `frust`; the `frust` facade does not
re-export it. It depends only on `frust-plugin` and `frust-paths` among the
Frust crates.

```toml
[dependencies]
frust-shared-preferences = "0.5"
```

On the two OS-shared stores (`NSUserDefaults` and Android `SharedPreferences`)
every key is prefixed with `frust.`, so `clear()` never removes entries that
belong to another library sharing the store. `keys()` returns the keys with
the prefix stripped.

The API is synchronous and not reactive. See the crate documentation for the
full API and per-backend storage details.

## Links

Documentation: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`)
or the MIT license (`LICENSE-MIT`) at your option (SPDX: `MIT OR Apache-2.0`).
Both license files are included beside this README.
