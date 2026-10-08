//! The thin link: one patch library per accepted change.
//!
//! The tip's fresh `.rcgu.o` files (captured from a no-link thin build),
//! the replayed workspace rlibs and the stub object that binds every symbol
//! the patch does not define to its address in the running image are
//! linked into `<target>/frust-hotpatch/<session>/patch-<n>.{dylib,so}`,
//! mode `0600` (the loopback hand-off names this file to the app, which
//! refuses one that grants group or other access; [`restrict_to_owner`]).
//! Only the linker flags a patch can use survive from the captured line,
//! per flavor, and the patch exports
//! [`ANCHOR_SYMBOL`](super::fat_link::ANCHOR_SYMBOL) so the runtime finds
//! its anchor by name.
//!
//! The argument rules are dioxus-cli 0.7.10's `build/link.rs`
//! (`compile_workspace_hotpatch`, `thin_link_args`), including its Rust
//! 1.86+ `-B`/`-fuse-ld=lld` forwarding on Gnu and its deletion of the
//! fat build's `deps/` copy after each patch. See
//! `docs/CLI_ARCHITECTURE.md`.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::process::ProcessRunner;

use super::fat_link::{LinkerFlavor, anchor_address, render, run_linker};
use super::link_intercept::output_path;
use super::{HotpatchError, hotpatch_root};

/// `<target_dir>/frust-hotpatch/<session>`. A `session` that is empty,
/// `.`/`..` or contains a path separator is
/// [`HotpatchError::BuilderUnsupported`].
pub fn session_dir(target_dir: &Path, session: &str) -> Result<PathBuf, HotpatchError> {
    let mut components = Path::new(session).components();
    let single_normal =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if !single_normal || session.contains(['/', '\\']) || session.starts_with('.') {
        return Err(HotpatchError::unsupported(format!(
            "`{session}` is not a hot-patch session name"
        )));
    }
    Ok(hotpatch_root(target_dir).join(session))
}

/// Empties [`session_dir`] (removing patches a previous session left) and
/// recreates it.
pub fn reset_session_dir(target_dir: &Path, session: &str) -> Result<PathBuf, HotpatchError> {
    let dir = session_dir(target_dir, session)?;
    match fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(HotpatchError::io(
                format!("clearing session dir `{}`", dir.display()),
                err,
            ));
        }
    }
    fs::create_dir_all(&dir)
        .map_err(|err| HotpatchError::io(format!("creating `{}`", dir.display()), err))?;
    Ok(dir)
}

/// `<target_dir>/frust-hotpatch/<session>/patch-<n>.<dylib|so>`.
pub fn patch_path(
    target_dir: &Path,
    session: &str,
    n: u32,
    flavor: LinkerFlavor,
) -> Result<PathBuf, HotpatchError> {
    Ok(session_dir(target_dir, session)?.join(format!("patch-{n}.{}", flavor.patch_extension())))
}

/// Sets `path`'s mode to `0600` (owner read/write, nothing for group or
/// other) — what the app requires of a patch handed off by file. A no-op
/// off unix, where no app accepts the hand-off.
pub fn restrict_to_owner(path: &Path) -> Result<(), HotpatchError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|err| {
            HotpatchError::io(
                format!("restricting `{}` to its owner", path.display()),
                err,
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// The captured flags a patch link keeps, after the flavor's own
/// shared-library flags. Darwin keeps `-framework`/`-arch`/`-L`/`-target`
/// pairs, `-l*`, `-m*` and `-nodefaultlibs` (and `-isysroot` for an iOS
/// target); Gnu keeps `-L` pairs, `-l*`, `-m*`, `-Wl,--target=`, the
/// `-B<path>`/`-fuse-ld` pair rustc injects to select its bundled lld, and
/// `-ld-path`. A flag missing its value is
/// [`HotpatchError::BuilderUnsupported`].
pub fn forwarded_args(
    flavor: LinkerFlavor,
    captured: &[String],
) -> Result<Vec<String>, HotpatchError> {
    let value_of = |index: usize| {
        captured.get(index + 1).cloned().ok_or_else(|| {
            HotpatchError::unsupported(format!(
                "captured linker flag `{}` has no value",
                captured[index]
            ))
        })
    };
    let mut out = Vec::new();
    match flavor {
        LinkerFlavor::Darwin => {
            out.push("-Wl,-dylib".to_string());
            let ios = captured.iter().enumerate().any(|(i, arg)| {
                arg.starts_with("-mios")
                    || (arg == "-target" && captured.get(i + 1).is_some_and(|t| t.contains("-ios")))
            });
            let mut pairs = vec!["-framework", "-arch", "-L", "-target"];
            if ios {
                pairs.push("-isysroot");
            }
            let mut index = 0;
            while index < captured.len() {
                let arg = &captured[index];
                if pairs.contains(&arg.as_str()) {
                    out.push(arg.clone());
                    out.push(value_of(index)?);
                    index += 2;
                    continue;
                }
                if arg.starts_with("-l") || arg.starts_with("-m") || arg == "-nodefaultlibs" {
                    out.push(arg.clone());
                }
                index += 1;
            }
        }
        LinkerFlavor::Gnu => {
            out.extend(
                [
                    "-shared",
                    "-Wl,--eh-frame-hdr",
                    "-Wl,-z,noexecstack",
                    "-Wl,-z,relro,-z,now",
                    "-nodefaultlibs",
                    "-Wl,-Bdynamic",
                ]
                .map(str::to_string),
            );
            let mut index = 0;
            while index < captured.len() {
                let arg = &captured[index];
                if arg == "-L" {
                    out.push(arg.clone());
                    out.push(value_of(index)?);
                    index += 2;
                    continue;
                }
                if arg.starts_with("-l")
                    || arg.starts_with("-m")
                    || arg.starts_with("-Wl,--target=")
                    || arg.starts_with("-Wl,-fuse-ld")
                    || arg.starts_with("-fuse-ld")
                    || arg.starts_with("-B")
                    || arg.contains("-ld-path")
                {
                    out.push(arg.clone());
                }
                index += 1;
            }
        }
    }
    Ok(out)
}

/// Everything one thin link needs.
#[derive(Debug, Clone, Copy)]
pub struct ThinLinkRequest<'a> {
    pub flavor: LinkerFlavor,
    /// From [`linker_program`](super::fat_link::linker_program).
    pub linker: &'a str,
    /// The tip's linker arguments captured from this thin build (not the
    /// fat build's): its `.rcgu.o` files and flags.
    pub tip_link_args: &'a [String],
    /// The rlibs of the replayed workspace crates, in link order.
    pub replayed_rlibs: &'a [PathBuf],
    /// The stub object resolving the patch's undefined symbols.
    pub stub_object: &'a Path,
    /// From [`patch_path`].
    pub output: &'a Path,
    /// Extra environment for the linker: the tip's captured rustc
    /// environment.
    pub envs: &'a [(String, String)],
}

/// The patch link line: the tip's `.rcgu.o` files (sorted), the replayed
/// rlibs, the stub object, any shared libraries the tip linked, the
/// [`forwarded_args`], the anchor export, and `-o <output>`. A capture with
/// no `.rcgu.o` is [`HotpatchError::BuilderUnsupported`].
pub fn thin_link_args(request: &ThinLinkRequest<'_>) -> Result<Vec<String>, HotpatchError> {
    let captured = request.tip_link_args;
    if captured.iter().any(|arg| arg.starts_with("/OUT:")) {
        return Err(HotpatchError::unsupported(
            "captured linker arguments are MSVC-style (`/OUT:`)",
        ));
    }
    let mut tip_objects: Vec<&String> = captured
        .iter()
        .filter(|arg| arg.ends_with(".rcgu.o"))
        .collect();
    if tip_objects.is_empty() {
        return Err(HotpatchError::unsupported(
            "the thin build's captured linker arguments name no `.rcgu.o` object",
        ));
    }
    tip_objects.sort();

    let mut args: Vec<String> = tip_objects.into_iter().cloned().collect();
    args.extend(request.replayed_rlibs.iter().map(|rlib| render(rlib)));
    args.push(render(request.stub_object));
    args.extend(
        captured
            .iter()
            .filter(|arg| arg.ends_with(".dylib") || arg.ends_with(".so"))
            .cloned(),
    );
    args.extend(forwarded_args(request.flavor, captured)?);
    args.push(request.flavor.anchor_export_arg());
    args.push("-o".to_string());
    args.push(render(request.output));
    Ok(args)
}

/// A successful thin link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinLinkOutput {
    pub patch: PathBuf,
    /// The link-time address of the anchor in the patch.
    pub anchor_address: u64,
    /// The fat build's `deps/` copy of the executable, when one was found
    /// and deleted.
    pub removed_deps_copy: Option<PathBuf>,
    /// The linker's diagnostics (warnings), if it printed any.
    pub linker_output: String,
}

/// Links the patch through `runner`, restricts it to its owner
/// ([`restrict_to_owner`]), then deletes the `deps/` copy of the
/// executable the thin build's `-o` names: dioxus-cli found that leaving it
/// makes later `dlopen`s fail with missing symbols that never existed. A
/// missing stub object, a failed link or a patch without the anchor is
/// [`HotpatchError::BuilderUnsupported`]; a linker that cannot be spawned is
/// [`HotpatchError::Process`].
pub fn thin_link(
    runner: &dyn ProcessRunner,
    request: &ThinLinkRequest<'_>,
) -> Result<ThinLinkOutput, HotpatchError> {
    let args = thin_link_args(request)?;
    let deps_copy = output_path(request.tip_link_args)?;
    if !request.stub_object.is_file() {
        return Err(HotpatchError::unsupported(format!(
            "stub object `{}` does not exist",
            request.stub_object.display()
        )));
    }
    if let Some(parent) = request
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|err| HotpatchError::io(format!("creating `{}`", parent.display()), err))?;
    }
    let linked = run_linker(runner, request.linker, &args, request.envs, "thin link");
    let removed_deps_copy =
        (deps_copy != request.output && fs::remove_file(&deps_copy).is_ok()).then_some(deps_copy);
    let linker_output = linked?;
    restrict_to_owner(request.output)?;
    let anchor_address = anchor_address(request.flavor, request.output)?;
    Ok(ThinLinkOutput {
        patch: request.output.to_path_buf(),
        anchor_address,
        removed_deps_copy,
        linker_output,
    })
}

#[cfg(test)]
mod tests {
    use super::super::fat_link::ANCHOR_SYMBOL;
    use super::super::fat_link::test_support::*;
    use super::*;

    struct Fixture {
        target: PathBuf,
        deps_copy: PathBuf,
        stub: PathBuf,
        rlibs: Vec<PathBuf>,
        output: PathBuf,
    }

    fn fixture(tag: &str, flavor: LinkerFlavor) -> Fixture {
        let root = temp_dir(tag);
        let target = root.join("target");
        let deps = target.join("debug").join("deps");
        fs::create_dir_all(&deps).unwrap();
        let deps_copy = deps.join("app-4444");
        fs::write(&deps_copy, b"no-link stand-in").unwrap();
        let stub = root.join("stub.o");
        fs::write(&stub, b"stub").unwrap();
        Fixture {
            output: patch_path(&target, "s-1", 3, flavor).unwrap(),
            rlibs: vec![deps.join("libui-1.rlib"), deps.join("libmodel-2.rlib")],
            target,
            deps_copy,
            stub,
        }
    }

    fn darwin_capture(deps_copy: &Path) -> Vec<String> {
        strings(&[
            "/t/rustcXYZ/symbols.o",
            "/t/deps/app-4444.app.bbbb-cgu.1.rcgu.o",
            "/t/deps/app-4444.app.aaaa-cgu.0.rcgu.o",
            "/t/deps/libui-1.rlib",
            "/rustlib/libstd-9.rlib",
            "/t/deps/libnative.dylib",
            "-framework",
            "CoreFoundation",
            "-lSystem",
            "-lc",
            "-arch",
            "arm64",
            "-mmacosx-version-min=11.0.0",
            "-isysroot",
            "/sdk",
            "-L",
            "/t/deps",
            "-Wl,-dead_strip",
            "-o",
            &render(deps_copy),
            "-nodefaultlibs",
        ])
    }

    fn gnu_capture(deps_copy: &Path) -> Vec<String> {
        strings(&[
            "/t/rustcXYZ/symbols.o",
            "/t/deps/app-4444.app.aaaa-cgu.0.rcgu.o",
            "/t/deps/libui-1.rlib",
            "-Wl,--as-needed",
            "-Wl,-Bstatic",
            "-lgcc_s",
            "-lutil",
            "-lc",
            "-m64",
            "-B/rust/lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld",
            "-fuse-ld=lld",
            "-Wl,--target=x86_64-unknown-linux-gnu",
            "-L",
            "/t/deps",
            "-Wl,-Bdynamic",
            "-Wl,-pie",
            "-o",
            &render(deps_copy),
        ])
    }

    fn request<'a>(
        fx: &'a Fixture,
        flavor: LinkerFlavor,
        capture: &'a [String],
        envs: &'a [(String, String)],
    ) -> ThinLinkRequest<'a> {
        ThinLinkRequest {
            flavor,
            linker: "cc",
            tip_link_args: capture,
            replayed_rlibs: &fx.rlibs,
            stub_object: &fx.stub,
            output: &fx.output,
            envs,
        }
    }

    fn patch_image(flavor: LinkerFlavor) -> Vec<u8> {
        image_defining(flavor, &[&flavor.object_symbol(ANCHOR_SYMBOL)])
    }

    #[test]
    fn patches_are_numbered_under_the_session_dir() {
        let target = Path::new("/w/build/rust");
        assert_eq!(
            patch_path(target, "s-1", 3, LinkerFlavor::Darwin).unwrap(),
            Path::new("/w/build/rust/frust-hotpatch/s-1/patch-3.dylib")
        );
        assert_eq!(
            patch_path(target, "s-1", 0, LinkerFlavor::Gnu).unwrap(),
            Path::new("/w/build/rust/frust-hotpatch/s-1/patch-0.so")
        );
        for bad in ["", ".", "..", "a/b", "a\\b", ".captured-args", "/abs"] {
            unsupported(session_dir(target, bad));
        }
    }

    #[test]
    fn resetting_the_session_dir_removes_old_patches() {
        let target = temp_dir("session");
        let dir = reset_session_dir(&target, "s-1").unwrap();
        fs::write(dir.join("patch-0.dylib"), b"old").unwrap();
        let again = reset_session_dir(&target, "s-1").unwrap();
        assert_eq!(again, dir);
        assert!(fs::read_dir(&dir).unwrap().next().is_none());
    }

    #[test]
    fn darwin_thin_link_args_name_objects_rlibs_stub_anchor_and_patch() {
        let fx = fixture("thin-darwin", LinkerFlavor::Darwin);
        let capture = darwin_capture(&fx.deps_copy);
        let runner = RecordingRunner::linking(patch_image(LinkerFlavor::Darwin));
        let envs = vec![("CARGO_PKG_NAME".to_string(), "app".to_string())];
        let output = thin_link(
            &runner,
            &request(&fx, LinkerFlavor::Darwin, &capture, &envs),
        )
        .unwrap();
        let link = runner.only_call("cc");
        assert_eq!(
            link.args,
            vec![
                "/t/deps/app-4444.app.aaaa-cgu.0.rcgu.o".to_string(),
                "/t/deps/app-4444.app.bbbb-cgu.1.rcgu.o".to_string(),
                render(&fx.rlibs[0]),
                render(&fx.rlibs[1]),
                render(&fx.stub),
                "/t/deps/libnative.dylib".to_string(),
                "-Wl,-dylib".to_string(),
                "-framework".to_string(),
                "CoreFoundation".to_string(),
                "-lSystem".to_string(),
                "-lc".to_string(),
                "-arch".to_string(),
                "arm64".to_string(),
                "-mmacosx-version-min=11.0.0".to_string(),
                "-L".to_string(),
                "/t/deps".to_string(),
                "-nodefaultlibs".to_string(),
                "-Wl,-exported_symbol,___frust_hotpatch_anchor".to_string(),
                "-o".to_string(),
                render(&fx.output),
            ]
        );
        assert_eq!(link.env, envs);
        assert!(fx.output.ends_with("frust-hotpatch/s-1/patch-3.dylib"));
        assert!(fx.output.starts_with(&fx.target));
        assert_eq!(output.patch, fx.output);
        assert_eq!(output.removed_deps_copy, Some(fx.deps_copy.clone()));
        assert!(
            !fx.deps_copy.exists(),
            "the fat exe's deps/ copy is deleted"
        );
    }

    #[test]
    fn darwin_keeps_the_sysroot_only_for_an_ios_target() {
        let ios = strings(&[
            "a.rcgu.o",
            "-target",
            "arm64-apple-ios17.0-simulator",
            "-isysroot",
            "/sim-sdk",
            "-o",
            "/t/x",
        ]);
        let args = forwarded_args(LinkerFlavor::Darwin, &ios).unwrap();
        assert_eq!(
            args,
            strings(&[
                "-Wl,-dylib",
                "-target",
                "arm64-apple-ios17.0-simulator",
                "-isysroot",
                "/sim-sdk"
            ])
        );
    }

    #[test]
    fn gnu_thin_link_args_forward_the_lld_selection_and_export_the_anchor() {
        let fx = fixture("thin-gnu", LinkerFlavor::Gnu);
        let capture = gnu_capture(&fx.deps_copy);
        let runner = RecordingRunner::linking(patch_image(LinkerFlavor::Gnu));
        let output = thin_link(&runner, &request(&fx, LinkerFlavor::Gnu, &capture, &[])).unwrap();
        let link = runner.only_call("cc");
        assert_eq!(
            link.args,
            vec![
                "/t/deps/app-4444.app.aaaa-cgu.0.rcgu.o".to_string(),
                render(&fx.rlibs[0]),
                render(&fx.rlibs[1]),
                render(&fx.stub),
                "-shared".to_string(),
                "-Wl,--eh-frame-hdr".to_string(),
                "-Wl,-z,noexecstack".to_string(),
                "-Wl,-z,relro,-z,now".to_string(),
                "-nodefaultlibs".to_string(),
                "-Wl,-Bdynamic".to_string(),
                "-lgcc_s".to_string(),
                "-lutil".to_string(),
                "-lc".to_string(),
                "-m64".to_string(),
                "-B/rust/lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld".to_string(),
                "-fuse-ld=lld".to_string(),
                "-Wl,--target=x86_64-unknown-linux-gnu".to_string(),
                "-L".to_string(),
                "/t/deps".to_string(),
                "-Wl,--export-dynamic-symbol,__frust_hotpatch_anchor".to_string(),
                "-o".to_string(),
                render(&fx.output),
            ]
        );
        assert!(fx.output.ends_with("frust-hotpatch/s-1/patch-3.so"));
        assert_eq!(output.removed_deps_copy, Some(fx.deps_copy.clone()));
        assert!(!fx.deps_copy.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(&output.patch).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the patch is restricted to its owner");
        }
    }

    #[test]
    fn the_deps_copy_is_deleted_even_when_the_link_fails() {
        let fx = fixture("thin-fail", LinkerFlavor::Gnu);
        let capture = gnu_capture(&fx.deps_copy);
        let runner = RecordingRunner::failing("undefined symbol: foo");
        let detail = unsupported(thin_link(
            &runner,
            &request(&fx, LinkerFlavor::Gnu, &capture, &[]),
        ));
        assert!(detail.contains("undefined symbol: foo"), "{detail}");
        assert!(!fx.deps_copy.exists());
    }

    #[test]
    fn malformed_thin_inputs_are_builder_unsupported() {
        let fx = fixture("thin-bad", LinkerFlavor::Darwin);
        let runner = RecordingRunner::linking(patch_image(LinkerFlavor::Darwin));

        let no_objects = strings(&["/t/deps/libui-1.rlib", "-o", "/t/x"]);
        unsupported(thin_link(
            &runner,
            &request(&fx, LinkerFlavor::Darwin, &no_objects, &[]),
        ));

        let dangling = strings(&["a.rcgu.o", "-o", "/t/x", "-framework"]);
        unsupported(thin_link(
            &runner,
            &request(&fx, LinkerFlavor::Darwin, &dangling, &[]),
        ));

        let no_output = strings(&["a.rcgu.o"]);
        unsupported(thin_link(
            &runner,
            &request(&fx, LinkerFlavor::Darwin, &no_output, &[]),
        ));

        let capture = darwin_capture(&fx.deps_copy);
        let missing_stub = fx.stub.with_file_name("missing-stub.o");
        let mut req = request(&fx, LinkerFlavor::Darwin, &capture, &[]);
        req.stub_object = &missing_stub;
        unsupported(thin_link(&runner, &req));
        assert!(runner.calls().is_empty(), "nothing was linked");
    }

    #[test]
    fn a_patch_without_the_anchor_is_builder_unsupported() {
        let fx = fixture("thin-anchor", LinkerFlavor::Darwin);
        let capture = darwin_capture(&fx.deps_copy);
        let runner = RecordingRunner::linking(image_defining(LinkerFlavor::Darwin, &["_other"]));
        let detail = unsupported(thin_link(
            &runner,
            &request(&fx, LinkerFlavor::Darwin, &capture, &[]),
        ));
        assert!(detail.contains("___frust_hotpatch_anchor"), "{detail}");
    }
}
