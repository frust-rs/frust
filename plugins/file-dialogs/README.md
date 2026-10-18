# frust-file-dialogs

Native file open and save dialogs for Frust apps on Linux, macOS and Windows,
backed by the [`rfd`](https://crates.io/crates/rfd) crate. A platform plugin:
add it to your app's `Cargo.toml` next to `frust`; the facade does not
re-export it.

## Platforms

| Target  | Backend                                                   |
|---------|-----------------------------------------------------------|
| Linux   | xdg-desktop-portal (`rfd` default `xdg-portal`), `zenity` fallback |
| macOS   | `NSOpenPanel` / `NSSavePanel` via `rfd`                    |
| Windows | `IFileOpenDialog` / `IFileSaveDialog` via `rfd`            |
| Android, iOS, web | `FileDialogError::Unsupported` (out of scope)    |

`rfd` is only compiled on desktop targets.

### Linux runtime requirements

A running xdg-desktop-portal with a file-chooser backend (GTK, KDE, ...).
libdbus is loaded at runtime, so nothing links at build time. If the portal is
unavailable `rfd` falls back to `zenity`. If neither exists, `rfd` logs an
error and returns no selection: the call yields `Ok(None)` / an empty `Vec`,
indistinguishable from a cancel. On a zenity-only desktop `rfd` splits the
multi-selection on `|`, so `open_files` splits a file name containing `|`.

## Usage

```rust
use frust_file_dialogs::{FileDialog, FileFilter};

let picked = FileDialog::new()
    .title("Choose an image")
    .directory("/home/me/Pictures")
    .filter(FileFilter::new("Images", &["png", "jpg"]))
    .open_file()?;            // Result<Option<PathBuf>, FileDialogError>
```

Terminal calls: `open_file()`, `open_files()`, and `save_file(Some("name.png"))`
(the suggested file name is the argument). `None` or an empty `Vec` means the
user cancelled. Filter extensions are normalised (`*.PNG`, `.png` and `png`
are equal).

## Blocking contract

Every terminal call blocks the calling thread until the dialog closes. Run it
in `frust::spawn_blocking` and deliver the result with `use_task` /
`AsyncValue`:

```rust
let picked = use_task(|| frust::spawn_blocking(|| {
    FileDialog::new().filter(FileFilter::new("Images", &["png"])).open_file()
}));
```

Never call from a paint or event handler, and never `signal.set` from the
background thread; hand the value back through the task result.

Threading follows `rfd`: on macOS it marshals the panel to the main thread
(`dispatch2::run_on_main`) when `NSApplication` is running, so calling from
`spawn_blocking` under the frust desktop shell is correct and the main thread
blocks while the modal panel runs; on Windows it initialises COM per calling
thread, so a worker thread is fine; on Linux the portal call blocks only the
calling thread. The macOS and Windows legs are validated by the owed platform
gates (e-g1, e-g2), Linux by the 2026-10-18 rig run.

## Security

Filters are display hints, not validation. Returned paths are untrusted input:
canonicalise and check them before reading or overwriting anything. Errors
never contain paths.
