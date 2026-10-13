# frust-share

Share text or an http/https URL through the platform share sheet.

## Add the dependency

```toml
[dependencies]
frust-share = "0.6"
```

## API

```rust
use frust_share::{Share, ShareAnchor, ShareOutcome};

Share::text("Hello", Some("Subject"))?;
Share::url("https://frust.dev", None)?;
// iOS iPad popover anchor (logical points); ignored elsewhere.
Share::text_anchored("Hello", None, Some(&ShareAnchor { rect }))?;
```

Fire-and-forget: `Ok` means the platform call was enqueued. `ShareError` is
`#[non_exhaustive]` and never echoes the payload. `Share::url` accepts only
absolute http/https URLs.

## Per-platform behaviour

| Platform | Behaviour | Outcome |
|----------|-----------|---------|
| Android | `ACTION_SEND` chooser (`text/plain`, `EXTRA_TEXT`, `EXTRA_SUBJECT`) via JNI | `Presented` |
| iOS | activity sheet (landing in sh-02) | `Presented` |
| macOS / Linux / Windows | desktop = clipboard copy via `frust-clipboard` | `CopiedToClipboard` |
| Other | `ShareError::Platform("unsupported platform")` | - |

## Device gate checklist (sh-03)

- [ ] Android (Pixel): chooser opens, text and subject arrive in the target app.
- [ ] iPhone SE: activity sheet presents.
- [ ] iPad: popover anchors to the button rect.
- [ ] Desktop: payload lands on the clipboard.

## License

Licensed under either of Apache License 2.0 or MIT, at your option.
