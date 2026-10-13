# frust-network-image

A network image view for [frust](https://github.com/frust-rs/frust) apps. `network_image(url)` loads
an image over HTTPS, decodes it off the UI thread, fits it to a target size, and caches it: decoded
images in a byte-budgeted memory LRU (32 MiB by default) and fetched bytes on disk under
`cache_dir()/frust-network-image` (128 MiB LRU by default; the URL is never written to disk).
Concurrent loads of one URL share a single fetch.

## 1. Install

```toml
frust-network-image = "0.6"            # add `features = ["webp"]` for WebP
```

```rust,ignore
frust::app!(App, setup = { NetworkImages::install(); });
```

`frust add plugin network-image` adds the dependency. With no installed loader a view shows its
error slot and logs once.

## 2. Use

```rust,ignore
network_image("https://example.com/cover.jpg")
    .placeholder(text("Loading"))
    .error(text("Image unavailable"))
    .fit(ImageFit::Cover)
    .target_px(800, 600)      // downscale after decode
    .fade(Duration::from_millis(200))  // Duration::ZERO turns the fade off
    .retry_on_tap(true)
    .headers(vec![("Authorization".into(), token)])
    .semantic_label("Album cover")
```

A memory-cache hit paints on the first frame with no fade. `ImageLoader` (from
`NetworkImages::loader()`) exposes `memory_stats`, `disk_stats`, `inflight_count`, `clear_memory`,
`clear_disk` and `clear_all`.

## 3. Transport and TLS

Fetching goes through the `ImageFetcher` trait. The default `ReqwestFetcher` (cargo feature
`reqwest`, on by default) uses reqwest over rustls with the `ring` provider and the Mozilla
`webpki-roots` trust store on every target, so no platform certificate verifier is involved. It
applies a 30 s timeout, follows redirects, and streams the body against a size cap (20 MiB by
default). With `default-features = false` you supply your own `ImageFetcher` through
`NetworkImages::install_with`.

## 4. Limits

- The caches ignore HTTP caching headers; the disk key is the URL alone (headers are not part of
  it), so a header-dependent response is not cached per header set.
- A JPEG is fully decoded before `target_px` downscales it. SVG is not supported; WebP needs the
  `webp` feature.
- Android's cache directory exists only after platform init; an earlier load is memory-only.

The accepted-limitation register is `docs/LIMITATIONS.md` in the frust repository
(`network-image-*` entries).

## 5. Android and iOS builds

`ring` compiles C, so the Android compile gate is
`cargo ndk -t arm64-v8a -P 26 check -p frust-network-image` (the plain `cargo check --target`
form works with `--no-default-features`). iOS: `cargo check --target aarch64-apple-ios-sim -p
frust-network-image`. A device walkthrough is the playground's **Images** page.

## License

MIT OR Apache-2.0
