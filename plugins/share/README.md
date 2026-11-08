# frust-share

Share text, an http/https URL or files through the platform share sheet.

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
// One or more files (absolute paths to existing regular files).
Share::files(&[path.as_path()], None)?;
Share::files_anchored(&[path.as_path()], None, Some(&ShareAnchor { rect }))?;
```

Fire-and-forget: `Ok` means the platform call was enqueued. `ShareError` is
`#[non_exhaustive]` and never echoes the payload. `Share::url` accepts only
absolute http/https URLs.

## Per-platform behaviour

| Platform | Behaviour | Outcome |
|----------|-----------|---------|
| Android | `ACTION_SEND` chooser (`text/plain`, `EXTRA_TEXT`, `EXTRA_SUBJECT`) via JNI | `Presented` |
| iOS | `UIActivityViewController` from the key window's top view controller, on the main queue | `Presented` |
| macOS | text/URL: clipboard copy via `frust-clipboard`; files: `NSSharingServicePicker` | `CopiedToClipboard` / `Presented` |
| Linux / Windows | text/URL: clipboard copy via `frust-clipboard`; files: `ShareError::Unsupported` | `CopiedToClipboard` / - |
| Android (files) | `ShareError::Unsupported` for now | - |
| Other | `ShareError::Platform("unsupported platform")` (text/URL), `Unsupported` (files) | - |

## Files

`Share::files(paths, subject)` takes 1 to 64 paths; each must be absolute and
name an existing regular file (checked and canonicalised before any platform
call). Failures are `EmptyPayload` (no paths), `TooManyFiles` (more than 64)
and `InvalidFile` (relative, missing, a directory, or not UTF-8); none echoes
a path. On iOS the activity sheet carries one file URL per path (iPad popover
rules below apply). On macOS an `NSSharingServicePicker` opens relative to the
key window's content view: with a `ShareAnchor` (top-left logical points) at
that rect, otherwise a zero-size rect at the view's centre; the `subject` is
ignored. Linux and Windows have no file share UI and return
`ShareError::Unsupported`.

## iOS

The activity sheet is presented from the key window's top view controller
(its root controller, followed up the `presentedViewController` chain) on the
main queue. `Share::url` hands the sheet an `NSURL`, `Share::text` an
`NSString`; a `subject` is offered to targets that use one (Mail). Called on
the main thread before any window exists, a share returns
`ShareError::NoPresenter`.

Popover rule: on an iPad the sheet is a popover. With a `ShareAnchor` it
points at that rect (logical points are UIKit points, in the root view's
space), so pass the tapped button's frame via `Share::text_anchored`; without
one (and always for `Share::url`) it sits at the centre of the root view with
no arrow. iPhone ignores the anchor.

No `Info.plist` key or entitlement is needed.

## Device gate checklist (sh-03)

- [ ] Android (Pixel): chooser opens, text and subject arrive in the target app.
- [ ] iPhone SE: activity sheet presents.
- [ ] iPad: popover anchors to the button rect.
- [ ] Desktop: payload lands on the clipboard.

## Related

Manual-test surface: the playground's URL page
(`examples/playground/src/pages/url_launcher.rs`, More -> URL). Desktop copies through
[`frust-clipboard`](../clipboard/README.md); for maps links see
[`frust-url-launcher`](../url-launcher/README.md)'s *Maps* section.

## License

Licensed under either of Apache License 2.0 or MIT, at your option.
