//! The thin link: one patch library per accepted change.
//!
//! The tip's fresh `.rcgu.o` files (captured from a no-link thin build),
//! the replayed workspace rlibs and the stub object that binds every symbol
//! the patch does not define to its address in the running image are
//! linked into `<target>/frust-hotpatch/<session>/patch-<n>.{dylib,so,dll}`,
//! mode `0600` (the loopback hand-off names this file to the app, which
//! refuses one that grants group or other access; [`restrict_to_owner`]).
//! Only the linker flags a patch can use survive from the captured line,
//! per flavor, and the patch exports
//! [`ANCHOR_SYMBOL`](super::fat_link::ANCHOR_SYMBOL) so the runtime finds
//! its anchor by name.
//!
//! The argument rules are dioxus-cli 0.7.10's `build/link.rs`
//! (`compile_workspace_hotpatch`, `thin_link_args`), including its Rust
//! 1.86+ `-B`/`-fuse-ld=lld` forwarding on Gnu, its fixed Msvc line
//! (`/DLL`, `/DEBUG` so the patch has the PDB its jump table is read from,
//! the anchor's `/EXPORT:`, `/HIGHENTROPYVA:NO`, `/OUT:`) and its deletion
//! of the fat build's `deps/` copy after each patch. An Msvc patch links
//! through the same linker as the fat image
//! ([`flavor_linker_program`](super::fat_link::flavor_linker_program):
//! `rust-lld -flavor link` by default) and is `patch-<n>.dll` with its own
//! `patch-<n>.pdb` beside it, which the session reads the patch's symbols
//! and its candidate layout table from. Beyond dx, an Msvc patch links at
//! its own fixed base with ASLR off ([`thin_link_with_base`],
//! [`patch_image_base`](super::fat_link::patch_image_base)), so it loads
//! at its link-time VAs.
//!
//! Beyond dx, a Gnu patch exports nothing but its anchor: a version script
//! ([`gnu_version_script`], written beside the patch as `patch-<n>.exports`)
//! makes every other symbol local. A shared library exports every
//! default-visibility global otherwise, which for a large app was tens of
//! thousands of mangled names: about half the stripped upload was
//! `.dynstr`, and every internal call went through the PLT. Nothing reads a
//! patch's dynamic symbols but the runtime's anchor lookup: the jump table
//! and the host's symbol map come from the unstripped patch's `.symtab`,
//! which keeps local symbols, and every symbol the patch leaves undefined is
//! bound by the stub, never by the dynamic linker. See
//! `docs/CLI_ARCHITECTURE.md`.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::process::ProcessRunner;

use super::fat_link::{
    ANCHOR_SYMBOL, HIGH_ENTROPY_VA_OFF, LinkerFlavor, anchor_address, fixed_base_args_at,
    linker_driver_args, render, run_linker,
};
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

/// `<target_dir>/frust-hotpatch/<session>/patch-<n>.<dylib|so|dll>`.
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
/// `-ld-path`. Msvc forwards nothing: its line is dx's fixed list (the
/// system import libraries std links, `/defaultlib:msvcrt`, `/DLL`,
/// `/DEBUG`, `/PDBALTPATH:%_PDB%`), the anchor export and
/// `/HIGHENTROPYVA:NO` following in [`thin_link_args`]. A flag missing its
/// value is [`HotpatchError::BuilderUnsupported`].
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
        LinkerFlavor::Msvc => out.extend(MSVC_THIN_ARGS.map(str::to_string)),
    }
    Ok(out)
}

/// dx's Msvc thin-link flags (`link.rs`), up to its `/EXPORT:` (the anchor's
/// here) and `/HIGHENTROPYVA:NO`.
const MSVC_THIN_ARGS: [&str; 11] = [
    "shlwapi.lib",
    "kernel32.lib",
    "advapi32.lib",
    "ntdll.lib",
    "userenv.lib",
    "ws2_32.lib",
    "dbghelp.lib",
    "/defaultlib:msvcrt",
    "/DLL",
    "/DEBUG",
    "/PDBALTPATH:%_PDB%",
];

/// Everything one thin link needs.
#[derive(Debug, Clone, Copy)]
pub struct ThinLinkRequest<'a> {
    pub flavor: LinkerFlavor,
    /// From [`flavor_linker_program`](super::fat_link::flavor_linker_program).
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

/// Whether `captured[index]` is the linker's output operand: the value after
/// `-o` / `--output`, or a joined `-o<path>` / `--output=<path>` /
/// `/OUT:<path>` spelling.
fn is_output_operand(captured: &[String], index: usize) -> bool {
    let arg = &captured[index];
    if (arg.len() > 2 && arg.starts_with("-o"))
        || arg.starts_with("--output=")
        || arg.starts_with("/OUT:")
    {
        return true;
    }
    index
        .checked_sub(1)
        .is_some_and(|prev| matches!(captured[prev].as_str(), "-o" | "--output"))
}

/// The patch link line: the tip's `.rcgu.o` files (sorted), the replayed
/// rlibs, the stub object, any shared libraries the tip linked, the
/// [`forwarded_args`], the anchor export, and `-o <output>` (Msvc:
/// `/HIGHENTROPYVA:NO` after the export, then `/OUT:<output>`). A capture
/// with no `.rcgu.o`, or in the other dialect (`/OUT:` for Darwin/Gnu, `-o`
/// for Msvc), is [`HotpatchError::BuilderUnsupported`]. No fixed base:
/// [`thin_link_args_with_base`] with `None`.
pub fn thin_link_args(request: &ThinLinkRequest<'_>) -> Result<Vec<String>, HotpatchError> {
    thin_link_args_with_base(request, None)
}

/// [`thin_link_args`], with an Msvc line linked at `fixed_base`: after
/// `/HIGHENTROPYVA:NO` come `/DYNAMICBASE:NO /BASE:<fixed_base>`
/// ([`fixed_base_args_at`], omitted for an ARM tip object, where lld-link
/// refuses them), then `/OUT:`. Darwin and Gnu lines ignore `fixed_base`.
pub fn thin_link_args_with_base(
    request: &ThinLinkRequest<'_>,
    fixed_base: Option<u64>,
) -> Result<Vec<String>, HotpatchError> {
    let captured = request.tip_link_args;
    if request.flavor == LinkerFlavor::Msvc {
        if captured.iter().any(|arg| arg == "-o") {
            return Err(HotpatchError::unsupported(
                "captured linker arguments are not MSVC-style (`-o`)",
            ));
        }
    } else if captured.iter().any(|arg| arg.starts_with("/OUT:")) {
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
    let machine_object = tip_objects[tip_objects.len() - 1];
    let fixed_base_args = fixed_base
        .filter(|_| request.flavor == LinkerFlavor::Msvc)
        .map(|base| fixed_base_args_at(base, Path::new(machine_object)))
        .unwrap_or_default();

    let mut args: Vec<String> = tip_objects.into_iter().cloned().collect();
    args.extend(request.replayed_rlibs.iter().map(|rlib| render(rlib)));
    args.push(render(request.stub_object));
    // The output operand is the thin build's own output, not a library it
    // linked: on Android it is the `cdylib`'s `lib<crate>.so`.
    args.extend(
        captured
            .iter()
            .enumerate()
            .filter(|(index, arg)| {
                (arg.ends_with(".dylib") || arg.ends_with(".so"))
                    && !is_output_operand(captured, *index)
            })
            .map(|(_, arg)| arg.clone()),
    );
    args.extend(forwarded_args(request.flavor, captured)?);
    args.push(request.flavor.anchor_export_arg());
    if request.flavor == LinkerFlavor::Gnu {
        args.push(format!(
            "-Wl,--version-script={}",
            render(&version_script_path(request.output))
        ));
    }
    if request.flavor == LinkerFlavor::Msvc {
        args.push(HIGH_ENTROPY_VA_OFF.to_string());
        args.extend(fixed_base_args);
        args.push(format!("/OUT:{}", render(request.output)));
    } else {
        args.push("-o".to_string());
        args.push(render(request.output));
    }
    Ok(args)
}

/// Where a Gnu patch link's version script is written: `patch-<n>.exports`
/// beside `output`.
pub fn version_script_path(output: &Path) -> PathBuf {
    output.with_extension("exports")
}

/// The version script of a Gnu patch: the anchor is its only dynamic
/// export, every other symbol is local (see the module doc).
pub fn gnu_version_script() -> String {
    format!("{{\n  global:\n    {ANCHOR_SYMBOL};\n  local:\n    *;\n}};\n")
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
/// [`HotpatchError::Process`]. The linker's
/// [`linker_driver_args`] lead the line. No fixed base:
/// [`thin_link_with_base`] with `None`.
pub fn thin_link(
    runner: &dyn ProcessRunner,
    request: &ThinLinkRequest<'_>,
) -> Result<ThinLinkOutput, HotpatchError> {
    thin_link_with_base(runner, request, None)
}

/// [`thin_link`] through [`thin_link_args_with_base`]: an Msvc patch links
/// at `fixed_base` with ASLR off, so it loads there and its anchor's VA is
/// its runtime address.
pub fn thin_link_with_base(
    runner: &dyn ProcessRunner,
    request: &ThinLinkRequest<'_>,
    fixed_base: Option<u64>,
) -> Result<ThinLinkOutput, HotpatchError> {
    let args = thin_link_args_with_base(request, fixed_base)?;
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
    if request.flavor == LinkerFlavor::Gnu {
        let script = version_script_path(request.output);
        fs::write(&script, gnu_version_script())
            .map_err(|err| HotpatchError::io(format!("writing `{}`", script.display()), err))?;
    }
    let mut argv = linker_driver_args(request.flavor, request.linker);
    argv.extend(args);
    let linked = run_linker(runner, request.linker, &argv, request.envs, "thin link");
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
    use super::super::fat_link::test_support::*;
    use super::super::fat_link::{ANCHOR_SYMBOL, patch_image_base};
    use super::super::pe;
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
                format!(
                    "-Wl,--version-script={}",
                    render(&fx.output.with_extension("exports"))
                ),
                "-o".to_string(),
                render(&fx.output),
            ]
        );
        assert!(fx.output.ends_with("frust-hotpatch/s-1/patch-3.so"));
        assert_eq!(
            fs::read_to_string(fx.output.with_extension("exports")).unwrap(),
            "{\n  global:\n    __frust_hotpatch_anchor;\n  local:\n    *;\n};\n",
            "the anchor is the patch's only export"
        );
        assert_eq!(output.removed_deps_copy, Some(fx.deps_copy.clone()));
        assert!(!fx.deps_copy.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(&output.patch).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the patch is restricted to its owner");
        }
    }

    fn msvc_capture(deps_copy: &Path) -> Vec<String> {
        strings(&[
            "/NOLOGO",
            "C:/t/rustcXYZ/symbols.o",
            "C:/t/deps/app-4444.app.bbbb-cgu.1.rcgu.o",
            "C:/t/deps/app-4444.app.aaaa-cgu.0.rcgu.o",
            "C:/t/deps/libui-1.rlib",
            "kernel32.lib",
            "/defaultlib:msvcrt",
            "/LIBPATH:C:/t/deps",
            &format!("/OUT:{}", render(deps_copy)),
            "/DEBUG",
        ])
    }

    /// dx's Msvc DLL line for [`msvc_capture`], with `fixed_base` (the
    /// `/DYNAMICBASE:NO /BASE:` pair) between `/HIGHENTROPYVA:NO` and
    /// `/OUT:`.
    fn msvc_line(fx: &Fixture, fixed_base: &[&str]) -> Vec<String> {
        let mut line = vec![
            "C:/t/deps/app-4444.app.aaaa-cgu.0.rcgu.o".to_string(),
            "C:/t/deps/app-4444.app.bbbb-cgu.1.rcgu.o".to_string(),
            render(&fx.rlibs[0]),
            render(&fx.rlibs[1]),
            render(&fx.stub),
            "shlwapi.lib".to_string(),
            "kernel32.lib".to_string(),
            "advapi32.lib".to_string(),
            "ntdll.lib".to_string(),
            "userenv.lib".to_string(),
            "ws2_32.lib".to_string(),
            "dbghelp.lib".to_string(),
            "/defaultlib:msvcrt".to_string(),
            "/DLL".to_string(),
            "/DEBUG".to_string(),
            "/PDBALTPATH:%_PDB%".to_string(),
            "/EXPORT:__frust_hotpatch_anchor".to_string(),
            "/HIGHENTROPYVA:NO".to_string(),
        ];
        line.extend(strings(fixed_base));
        line.push(format!("/OUT:{}", render(&fx.output)));
        line
    }

    #[test]
    fn msvc_thin_link_args_are_dx_dll_line_at_the_patch_fixed_base() {
        let fx = fixture("thin-msvc", LinkerFlavor::Msvc);
        let deps_copy = fx.deps_copy.with_extension("exe");
        fs::write(&deps_copy, b"no-link stand-in").unwrap();
        let capture = msvc_capture(&deps_copy);
        let mut req = request(&fx, LinkerFlavor::Msvc, &capture, &[]);
        req.linker = "link.exe";
        let runner = RecordingRunner::linking(b"MZ stand-in".to_vec());
        // The stand-in DLL has no PDB, so its anchor is refused after the
        // link ran.
        unsupported(thin_link_with_base(&runner, &req, patch_image_base(1)));
        let link = runner.only_call("link.exe");
        assert_eq!(
            link.args,
            msvc_line(&fx, &["/DYNAMICBASE:NO", "/BASE:0x20000000"])
        );
        assert!(fx.output.ends_with("frust-hotpatch/s-1/patch-3.dll"));
        assert!(fs::read(&fx.output).is_ok(), "the link wrote /OUT:");
        assert!(
            !deps_copy.exists(),
            "the thin build's /OUT: copy is deleted"
        );

        // Patch 2 links one stride higher; without a base the line is dx's.
        assert_eq!(
            thin_link_args_with_base(&req, patch_image_base(2)).unwrap(),
            msvc_line(&fx, &["/DYNAMICBASE:NO", "/BASE:0x30000000"])
        );
        assert_eq!(thin_link_args(&req).unwrap(), msvc_line(&fx, &[]));

        // A `-o` capture is not an Msvc line.
        let cc_style = strings(&["a.rcgu.o", "-o", "/t/x"]);
        unsupported(thin_link_args(&request(
            &fx,
            LinkerFlavor::Msvc,
            &cc_style,
            &[],
        )));
    }

    #[test]
    fn an_arm_msvc_patch_keeps_aslr_and_cc_lines_ignore_the_base() {
        let fx = fixture("thin-msvc-arm", LinkerFlavor::Msvc);
        let mut header = pe::MACHINE_ARM64.to_le_bytes().to_vec();
        header.resize(64, 0);
        let arm64 = fx.target.join("app.app.aaaa-cgu.0.rcgu.o");
        fs::write(&arm64, header).unwrap();
        let capture = vec![render(&arm64), "/OUT:C:/t/deps/app.exe".to_string()];
        let req = request(&fx, LinkerFlavor::Msvc, &capture, &[]);
        // lld-link refuses `/DYNAMICBASE:NO` on ARM: the DLL stays
        // relocatable, and the runtime's anchor check refuses it.
        assert_eq!(
            thin_link_args_with_base(&req, patch_image_base(1)).unwrap(),
            thin_link_args(&req).unwrap()
        );

        for (flavor, capture) in [
            (LinkerFlavor::Darwin, darwin_capture(&fx.deps_copy)),
            (LinkerFlavor::Gnu, gnu_capture(&fx.deps_copy)),
        ] {
            let req = request(&fx, flavor, &capture, &[]);
            assert_eq!(
                thin_link_args_with_base(&req, patch_image_base(1)).unwrap(),
                thin_link_args(&req).unwrap(),
                "{flavor:?}"
            );
        }
    }

    #[test]
    fn an_msvc_link_through_rust_lld_leads_with_the_link_flavor() {
        let fx = fixture("thin-msvc-lld", LinkerFlavor::Msvc);
        let capture = strings(&[
            "C:/t/deps/app.app.aaaa-cgu.0.rcgu.o",
            "/OUT:C:/t/deps/app.exe",
        ]);
        let mut req = request(&fx, LinkerFlavor::Msvc, &capture, &[]);
        let lld = "/rust/lib/rustlib/x86_64-pc-windows-msvc/bin/rust-lld.exe";
        req.linker = lld;
        let runner = RecordingRunner::linking(b"MZ stand-in".to_vec());
        unsupported(thin_link(&runner, &req));
        let link = runner.only_call(lld);
        assert_eq!(link.args[..2], ["-flavor".to_string(), "link".to_string()]);
        assert_eq!(
            link.args[2..],
            thin_link_args(&req).unwrap()[..],
            "the link line itself is unchanged"
        );
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

    fn shared_libs_in(args: &[String]) -> Vec<&str> {
        args.iter()
            .map(String::as_str)
            .filter(|arg| arg.ends_with(".so") || arg.ends_with(".dylib"))
            .collect()
    }

    fn thin_args(flavor: LinkerFlavor, tail: &[&str]) -> Vec<String> {
        let fx = fixture("thin-output-operand", flavor);
        let mut capture = strings(&["a.rcgu.o"]);
        capture.extend(strings(tail));
        thin_link_args(&request(&fx, flavor, &capture, &[])).unwrap()
    }

    #[test]
    fn the_android_cdylib_output_is_not_relinked_but_other_so_libs_pass() {
        let args = thin_args(
            LinkerFlavor::Gnu,
            &["/ndk/libc++_shared.so", "-o", "/t/deps/libapp.so"],
        );
        let libs = shared_libs_in(&args[..args.len() - 2]);
        assert_eq!(libs, ["/ndk/libc++_shared.so"]);
    }

    #[test]
    fn the_desktop_dylib_output_is_not_relinked() {
        let args = thin_args(
            LinkerFlavor::Darwin,
            &["/t/libdep.dylib", "-o", "/t/deps/x.dylib"],
        );
        assert_eq!(shared_libs_in(&args[..args.len() - 2]), ["/t/libdep.dylib"]);
    }

    #[test]
    fn joined_output_spellings_are_skipped_not_linked() {
        for tail in [
            &["-o/t/deps/libapp.so"][..],
            &["--output=/t/deps/libapp.so"][..],
            &["--output", "/t/deps/libapp.so"][..],
        ] {
            let args = thin_args(LinkerFlavor::Gnu, tail);
            assert!(
                shared_libs_in(&args[..args.len() - 2]).is_empty(),
                "{tail:?} leaked the output into the link"
            );
        }
    }

    #[test]
    fn a_non_output_so_passes_through() {
        let args = thin_args(LinkerFlavor::Gnu, &["/ndk/liblog.so", "-o", "/t/x"]);
        assert_eq!(shared_libs_in(&args[..args.len() - 2]), ["/ndk/liblog.so"]);
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
