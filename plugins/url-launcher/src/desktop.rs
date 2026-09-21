//! The desktop (macOS/Linux/Windows) backend — the platform's own URL
//! opener: `open` on macOS, `xdg-open` on Linux, `ShellExecuteW` on Windows.
//!
//! # macOS/Linux: spawned, never waited for
//!
//! `open`/`xdg-open` are launched via [`std::process::Command::spawn`] with
//! their stdio piped to `/dev/null` rather than inherited, and the returned
//! [`Child`](std::process::Child) is dropped immediately — lifted from
//! `frust-cli`'s own `crates/frust-cli/src/commands/run.rs::open_browser`/
//! `browser_command` shape (the macOS/Linux arm only; see the next section
//! for why the Windows arm below is not `cmd /C start`) without this crate
//! taking a `frust-cli` dependency, which the platform-plugin charter
//! forbids (`docs/PLUGINS_CODE_STANDARDS.md`). Dropping a [`Child`] handle
//! neither kills nor joins it (only an explicit `kill`/`wait` does), so the
//! opener runs to completion on its own — matching this crate's
//! fire-and-forget contract. The one failure this can observe is the spawn
//! itself: [`std::io::ErrorKind::NotFound`] (no `open`/`xdg-open` on `PATH`)
//! maps to [`UrlLauncherError::NoHandler`]; anything else maps to
//! [`UrlLauncherError::Platform`], carrying only the
//! [`ErrorKind`](std::io::ErrorKind) — never the OS error message or the
//! URL, which a raw `io::Error`'s `Display` could otherwise leak.
//!
//! # Windows: `ShellExecuteW`, not `cmd /C start`
//!
//! `frust-cli`'s own web-preview opener uses `cmd /C start "" <url>` for
//! Windows, but that goes through `cmd.exe`'s command-line parser, which
//! treats `&` as a command separator and expands `%VAR%` references — both
//! of which can appear in a legitimate URL (a query string with `&`, or a
//! path segment that happens to look like `%3D`-style percent-encoding
//! colliding with `cmd`'s own `%`-expansion syntax) and would corrupt or
//! hijack the launch. `ShellExecuteW("open", url)` (`shell32.dll`) opens the
//! URL through the registered protocol handler directly, with no shell
//! command-line parsing in between.
//!
//! The `HINSTANCE` `ShellExecuteW` returns is not a real instance handle for
//! values `<= 32`: Microsoft's documented convention is a real handle
//! (`> 32`) on success, one of a fixed set of small integer error codes
//! otherwise. `SE_ERR_NOASSOC`/`SE_ERR_ASSOCINCOMPLETE` — no association, or
//! an incomplete/invalid one, for the URL's protocol — map to
//! [`UrlLauncherError::NoHandler`]; any other code maps to
//! [`UrlLauncherError::Platform`], carrying only the numeric code.

use std::io;
use std::process::{Command, Stdio};

use crate::UrlLauncherError;

/// macOS/Linux: launch `open`/`xdg-open <url>`, detached (module doc's
/// *macOS/Linux: spawned, never waited for*).
#[cfg(not(target_os = "windows"))]
pub(crate) fn open_external(url: &str) -> Result<(), UrlLauncherError> {
    let program = opener_program();
    match Command::new(program)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => {
            // Detached: dropping neither kills nor joins the child (module
            // doc). Explicit for readability — an implicit end-of-scope drop
            // would do the same.
            drop(child);
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Err(UrlLauncherError::NoHandler),
        Err(err) => Err(UrlLauncherError::Platform(format!(
            "spawn {program}: {:?}",
            err.kind()
        ))),
    }
}

/// `open` on macOS, `xdg-open` on Linux — the only two targets this module
/// compiles for besides Windows (the crate's `#[cfg(any(target_os = "macos",
/// target_os = "linux", target_os = "windows"))]` gate on `mod desktop` in
/// `lib.rs`).
#[cfg(not(target_os = "windows"))]
fn opener_program() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// Windows: `ShellExecuteW("open", url)` (module doc's *Windows:
/// `ShellExecuteW`, not `cmd /C start`*).
#[cfg(target_os = "windows")]
pub(crate) fn open_external(url: &str) -> Result<(), UrlLauncherError> {
    use std::ptr;

    use windows_sys::Win32::UI::Shell::{SE_ERR_ASSOCINCOMPLETE, SE_ERR_NOASSOC, ShellExecuteW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    // NUL-terminated UTF-16 buffers, held in locals across the call —
    // `PCWSTR` is a borrowed pointer and `ShellExecuteW` does not outlive it
    // (mirrors `frust-shell-windows::win32_glue::set_app_user_model_id`).
    let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let file: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: `verb`/`file` are NUL-terminated UTF-16 buffers that live for
    // the whole call, which is `ShellExecuteW`'s contract for `lpOperation`/
    // `lpFile`; `hwnd`/`lpParameters`/`lpDirectory` null is documented as
    // valid (no owner window, no parameters, inherit the current directory).
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = result as isize;
    if code > 32 {
        return Ok(());
    }
    if code == SE_ERR_NOASSOC as isize || code == SE_ERR_ASSOCINCOMPLETE as isize {
        return Err(UrlLauncherError::NoHandler);
    }
    Err(UrlLauncherError::Platform(format!(
        "ShellExecuteW returned {code}"
    )))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::env;
    use std::fs;
    use std::sync::Mutex;

    use super::*;

    /// Serializes tests that mutate the process-wide `PATH` env var — `PATH`
    /// is process-global state, so a concurrent test run elsewhere in this
    /// binary must not observe (or clobber) the temporary value below.
    static PATH_LOCK: Mutex<()> = Mutex::new(());

    /// With `PATH` pointed at an empty directory, `xdg-open` cannot be
    /// found — the spawn itself fails with `NotFound`, which this backend
    /// maps to [`UrlLauncherError::NoHandler`] (module doc's *macOS/Linux:
    /// spawned, never waited for*).
    #[test]
    fn no_handler_when_no_opener_on_path() {
        let _guard = PATH_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let empty_dir = env::temp_dir().join(format!(
            "frust-url-launcher-empty-path-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&empty_dir).expect("create empty PATH directory");

        let original_path = env::var_os("PATH");
        // SAFETY: no other thread in this test binary reads/writes `PATH`
        // concurrently while `PATH_LOCK` is held — every test that touches
        // `PATH` in this module takes the same lock first.
        unsafe {
            env::set_var("PATH", &empty_dir);
        }

        let result = open_external("https://example.com/");

        // SAFETY: see above — still holding `PATH_LOCK`.
        unsafe {
            match &original_path {
                Some(path) => env::set_var("PATH", path),
                None => env::remove_var("PATH"),
            }
        }
        let _ = fs::remove_dir_all(&empty_dir);

        assert!(
            matches!(result, Err(UrlLauncherError::NoHandler)),
            "{result:?}"
        );
    }
}
