# frust-file-dialogs

Native file dialogs, persistable folder access and file export for Frust apps
on Linux, macOS, Windows, Android and iOS. A platform plugin: add it to your
app's `Cargo.toml` next to `frust`; the facade does not re-export it.

## Platforms

| Target  | Backend                                                   |
|---------|-----------------------------------------------------------|
| Linux   | xdg-desktop-portal (`rfd` default `xdg-portal`), `zenity` fallback |
| macOS   | `NSOpenPanel` / `NSSavePanel` via `rfd`                    |
| Windows | `IFileOpenDialog` / `IFileSaveDialog` via `rfd`            |
| Android | Storage Access Framework                                   |
| iOS     | `UIDocumentPickerViewController`                           |
| Web and others | `FileDialogError::Unsupported`                      |

`rfd` is only compiled on desktop targets.

| Call | Desktop | Android, iOS |
|------|---------|--------------|
| `open_file`, `open_files` | the user's original files | app-private copies |
| `save_file` | a destination path | `Unsupported`: use `export_file` |
| `export_file` | save dialog, then a copy | the platform's export UI |
| `pick_folder` | a `FolderGrant` over the path | a persistable grant (iOS security-scoped bookmark, Android tree URI) |

### Linux runtime requirements

A running xdg-desktop-portal with a file-chooser backend (GTK, KDE, ...).
libdbus is loaded at runtime, so nothing links at build time. If the portal is
unavailable `rfd` falls back to `zenity`. If neither exists, `rfd` logs an
error and returns no selection: the call yields `Ok(None)` / an empty `Vec`,
indistinguishable from a cancel. On a zenity-only desktop `rfd` splits the
multi-selection on `|`, so `open_files` loses a file name containing `|` (the
relative fragments of such a split are dropped, never returned as paths).

## Usage

```rust
use frust_file_dialogs::{FileDialog, FileFilter};

let picked = FileDialog::new()
    .title("Choose an image")
    .directory("/home/me/Pictures")
    .filter(FileFilter::new("Images", &["png", "jpg"]))
    .open_file()?;            // Result<Option<PathBuf>, FileDialogError>
```

Terminal calls: `open_file()`, `open_files()`, `save_file(Some("name.png"))`
(the suggested file name is the argument), `pick_folder()` and
`export_file(&source, Some("name.png"))`. `None`, an empty `Vec` or
`ExportOutcome::Cancelled` means the user cancelled. Filter extensions are
normalised (`*.PNG`, `.png` and `png` are equal).

### Exporting a file

`export_file` lets the user choose where a copy of `source` goes, and writes
it. With no suggested name it pre-fills the source's own file name.

```rust
use frust_file_dialogs::{ExportOutcome, FileDialog};

match FileDialog::new().export_file(&report, Some("report.pdf"))? {
    ExportOutcome::Saved => { /* written */ }
    ExportOutcome::Cancelled => { /* nothing written */ }
    _ => {}
}
```

### Folder access

`pick_folder` returns a `FolderGrant`. Persist `grant.token()` and get back to
the folder after a restart with `FolderGrant::restore(&token)`. A token can
change on restore, so persist the restored grant's token again.
`FileDialogError::AccessLost` means the token is no longer honoured (the folder
is gone or the grant was revoked): ask the user again.

`grant.entry(relative_path)` reopens one file whose relative path the app stored,
without listing the folder. It takes the `/`-separated string a listing puts in
`FolderEntry::relative_path` and returns `Ok(Some(entry))`, readable like a listed
one, or `Ok(None)` when there is no regular file there (nothing at the path, a
folder, a link, or a hidden segment). Matching is exact and case-sensitive. A path
a listing cannot produce (empty, leading `/`, `\`, NUL, or an empty, `.` or `..`
segment) is `Io`; a revoked grant is `AccessLost`; Android and iOS refuse the UI
thread with `UiThread`. On Android each path segment costs one directory listing,
so when reading many files in one session keep the `FolderEntry` from a listing
instead of calling `entry` for each.

On iOS the grant holds the folder's security scope while it lives, so a listed
`FolderEntry` can only be read while its `FolderGrant` is alive. Once the grant
is released or dropped, `copy_to` and `read_to_vec` fail: with `AccessLost` when
the system's denial can be told apart, otherwise `Io`. On Android,
`FolderGrant::release` ends the persisted permission for that folder, including
for any other grant on the same folder. `copy_to` and `read_to_vec` refuse the UI
thread with `UiThread` on Android and iOS, like the rest of the plugin.

`read_to_vec(max_bytes)` reads the whole file and fails with
`FileDialogError::TooLarge { limit, size }` when it is longer than `max_bytes`
(`size` is the file's length when the platform reports it, else `None`); other
failures stay `Io` or `AccessLost`. `read_prefix(max_bytes)` returns only the
first `max_bytes` bytes, or the whole file when it is shorter, and is never
`TooLarge`; `0` gives an empty `Vec`. Both refuse the UI thread on Android and
iOS. On Android `read_prefix` copies at most `max_bytes` of the document.

```rust
// Sniff a file's type from its first 16 bytes without loading the rest.
let head = entry.read_prefix(16)?;
let is_png = head.starts_with(b"\x89PNG\r\n\x1a\n");
```

```rust
use frust_file_dialogs::{FileDialog, ListOptions};

if let Some(grant) = FileDialog::new().title("Choose a folder").pick_folder()? {
    save_token(grant.token());
    let photos = grant.list(&ListOptions::new().recursive(true).extensions(&["jpg", "png"]))?;
    for entry in &photos {
        let copy = entry.copy_to(&cache_dir)?;        // never replaces a file
        let bytes = entry.read_to_vec(16 * 1024 * 1024)?;  // refused above the limit
    }
    grant.release();     // iOS: ends the scope; Android: ends the persisted permission
}
```

`list` returns regular files sorted by relative path (`/` separated on every
platform). Links are never followed or listed. Defaults: flat, hidden entries
skipped, at most 8 levels and 10 000 entries when those caps apply; a listing
that reaches `max_entries` is cut short. `local_path()` is `None` on Android,
where `FolderEntry::location` is `EntryLocation::ContentUri`.

### Cancel and progress

`FileDialog::cancel(token)` and `ListOptions::cancel(token)` take a `CancelToken`
(clone it, hand one clone to the call and fire the other from any thread). A
cancelled call returns `FileDialogError::Cancelled`, which is distinct from the
user dismissing a dialog (`Ok(None)`, an empty `Vec`, `ExportOutcome::Cancelled`).
A token is single-use; a token that is already cancelled fails every call before
any dialog appears.

`FileDialog::on_progress(|p| ...)` reports `FileProgress { items, done, fraction }`
(at most about 20 calls per second; the first and final calls always arrive;
callbacks may run on any thread). The first call means the dialog has closed and
the chosen items are loading.

```rust
let token = CancelToken::new();
let stop = token.clone(); // fire `stop.cancel()` from a Cancel button
let outcome = FileDialog::new()
    .cancel(token)
    .on_progress(|p| set_fraction(p.fraction))
    .export_file(&report, Some("report.pdf"));
```

| Platform | Token semantics |
|----------|-----------------|
| Desktop | A native dialog cannot be closed from another thread: the token is checked before the dialog shows and again when it closes. `export_file` copies in chunks, checks the token between them and reports progress; a cancelled or failed copy removes its partial file and never touches an existing destination. `ListOptions::cancel` is checked between entries. |
| Android | The token cancels the wait and any copy, but cannot close a system dialog that is already showing. |
| iOS | The token cancels the wait and any copy. |

Desktop `open_file`, `open_files`, `save_file` and `pick_folder` return the user's
own paths and report no progress.

## Blocking contract

Every terminal call blocks the calling thread until the dialog closes, and
`FolderGrant` and `FolderEntry` calls block on storage. Run them in
`frust::spawn_blocking` and deliver the result with `use_task` / `AsyncValue`:

```rust
let picked = use_task(|| frust::spawn_blocking(|| {
    FileDialog::new().filter(FileFilter::new("Images", &["png"])).open_file()
}));
```

Never call from a paint or event handler, and never `signal.set` from the
background thread; hand the value back through the task result. On Android and
iOS a call on the UI thread is refused with `FileDialogError::UiThread`.

Desktop threading follows `rfd`: on macOS it marshals the panel to the main thread
(`dispatch2::run_on_main`) when `NSApplication` is running, so calling from
`spawn_blocking` under the frust desktop shell is correct and the main thread
blocks while the modal panel runs; on Windows it initialises COM per calling
thread, so a worker thread is fine; on Linux the portal call blocks only the
calling thread.

## Tested platforms

Round trips by hand on real hardware: Linux, macOS and Windows desktops (2026-10-18, open, open-multi and
save-as), an iPhone SE 2nd gen (iOS 26.7.1) and a Pixel 5 (Android 14), 2026-11-09 (open copies, export,
folder pick and restore across relaunch), with release on the iPhone and cancel on the Pixel; macOS and
Windows export, folder pick and restore, 2026-11-09. Not measured: GPS location through an Android
folder read; the export and folder legs on Linux; cancel on any desktop. Web only reports
`Unsupported`. See the *File-dialogs manual test* in `docs/PLUGINS_DEVELOPMENT.md`.

## Security

Filters are display hints, not validation. Returned paths are untrusted input:
canonicalise and check them before reading or overwriting anything. Errors
never contain paths; the detail goes to the log. A `FolderGrant::token`
identifies user files, so store it as you would a path.
