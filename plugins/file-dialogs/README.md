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

```rust
use frust_file_dialogs::{FileDialog, ListOptions};

if let Some(grant) = FileDialog::new().title("Choose a folder").pick_folder()? {
    save_token(grant.token());
    let photos = grant.list(&ListOptions::new().recursive(true).extensions(&["jpg", "png"]))?;
    for entry in &photos {
        let copy = entry.copy_to(&cache_dir)?;        // never replaces a file
        let bytes = entry.read_to_vec(16 * 1024 * 1024)?;  // refused above the limit
    }
    grant.release();                                  // on Android, also revokes the grant
}
```

`list` returns regular files sorted by relative path (`/` separated on every
platform). Links are never followed or listed. Defaults: flat, hidden entries
skipped, at most 8 levels and 10 000 entries when those caps apply; a listing
that reaches `max_entries` is cut short. `local_path()` is `None` on Android,
where `FolderEntry::location` is `EntryLocation::ContentUri`.

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
calling thread. The Linux leg was exercised on 2026-10-18; the macOS and
Windows legs have not been run on this plugin yet.

## Security

Filters are display hints, not validation. Returned paths are untrusted input:
canonicalise and check them before reading or overwriting anything. Errors
never contain paths; the detail goes to the log. A `FolderGrant::token`
identifies user files, so store it as you would a path.
