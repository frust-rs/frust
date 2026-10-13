# frust-qr

A scannable QR code view for Frust apps, painted on a `canvas` (platform-free).

## Add the dependency

```toml
[dependencies]
frust-qr = "0.6"
```

## API

```rust
use frust_qr::{EccLevel, qr_code};

qr_code("https://example.com/booking/42")
    .ecc(EccLevel::Q)        // default M
    .quiet_zone(4)           // modules, default 4
    .colors(dark, light)     // default black on white
    .size(220.0)             // default: min(available width, height)
    .semantic_label("Booking confirmation QR code")
```

The payload is encoded once at construction (`qrcode` crate). Painting fills
the light background, then one rectangle per run of dark modules. The module
side is `floor(side / (modules + 2 * quiet))` logical px so edges stay on the
pixel grid; the remainder is split evenly around the symbol. An unencodable
payload (empty or over capacity) paints only the background; see
`QrCodeView::error`. The accessibility node is an image labelled "QR code"
unless `semantic_label` says otherwise.
