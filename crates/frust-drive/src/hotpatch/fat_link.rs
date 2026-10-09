//! The fat link: the base image every later patch is linked against.
//!
//! A fat build runs with `-Csave-temps=true -Clink-dead-code` and the link
//! step intercepted as no-link ([`link_intercept`](super::link_intercept)),
//! so its captured linker arguments name the tip's own `.rcgu.o` files and
//! every rlib. The builder then links for real: every `.rcgu.o` inside the
//! rlibs cargo built under the target dir is packed into one
//! `libdeps-<hash>.a` that the linker force-loads (`-force_load` on Darwin,
//! `--whole-archive` on Gnu, `/WHOLEARCHIVE:` on Msvc), so no function a
//! later patch may reference is
//! dropped, and the image exports [`ANCHOR_SYMBOL`] instead of dioxus-cli's
//! `main`. Toolchain rlibs (std) pass through unchanged.
//!
//! The argument rules are dioxus-cli 0.7.10's `build/link.rs`
//! (`run_fat_link`, `linker_flavor`, `select_linker`) without
//! `target-lexicon`: the flavor comes from the target triple string, and an
//! unknown one is [`HotpatchError::BuilderUnsupported`]. Darwin and Gnu
//! arguments are in the `cc`-driver form rustc emits for a linker it does
//! not recognise by name; Msvc arguments are link.exe's (`/OUT:`), and an
//! Msvc image's symbols come from its PDB ([`super::pe`]). See
//! `docs/CLI_ARCHITECTURE.md`.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::host_path;
use crate::process::{ProcessRunner, tail_lines};

use super::HotpatchError;
use super::link_intercept::output_path;

/// The app-owned ASLR anchor every image and patch must define: a
/// `#[unsafe(no_mangle)] pub extern "C" fn` the `frust::app!` macro emits.
/// Must equal `frust_hotpatch::ANCHOR_SYMBOL`, which resolves it at runtime.
pub const ANCHOR_SYMBOL: &str = "__frust_hotpatch_anchor";

/// The `cc` driver both supported flavors link through (ld64 on macOS, the
/// system or rustc-selected `ld`/`lld` on Linux).
pub const DEFAULT_LINKER: &str = "cc";

/// Linkers that take raw linker arguments rather than `cc`-driver ones. The
/// builder only speaks the driver form, so naming one of these fails closed.
const RAW_LINKERS: &[&str] = &[
    "ld", "ld64", "ld64.lld", "ld.lld", "lld", "ld.gold", "ld.bfd", "mold", "sold", "wild",
    "lld-link", "link", "wasm-ld", "rust-lld",
];

/// The link.exe flag both Msvc images (the fat exe and every patch DLL)
/// carry: dx's "Prevent alsr from overflowing 32 bits".
pub const HIGH_ENTROPY_VA_OFF: &str = "/HIGHENTROPYVA:NO";

/// Which linker argument dialect a target needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkerFlavor {
    /// Mach-O through `cc` and ld64 (macOS, and the iOS simulator).
    Darwin,
    /// ELF through `cc` and a GNU-compatible linker (Linux, Android).
    Gnu,
    /// PE/COFF through link.exe-style arguments (`pc-windows-msvc`).
    Msvc,
}

impl LinkerFlavor {
    /// The flavor for a target `triple` (`aarch64-apple-darwin`,
    /// `x86_64-unknown-linux-gnu`, `aarch64-linux-android`, ...), following
    /// rustc's own environment-then-OS order. `<arch>-pc-windows-msvc` is
    /// [`LinkerFlavor::Msvc`]; GNU-environment Windows, UWP, wasm and
    /// anything unrecognised are [`HotpatchError::BuilderUnsupported`].
    pub fn for_triple(triple: &str) -> Result<Self, HotpatchError> {
        let parts: Vec<&str> = triple.split('-').collect();
        let [arch, rest @ ..] = parts.as_slice() else {
            return Err(HotpatchError::unsupported("empty target triple"));
        };
        let has = |name: &str| rest.contains(&name);
        let unsupported = |why: &str| {
            Err(HotpatchError::unsupported(format!(
                "no hot-patch linker flavor for target `{triple}`: {why}"
            )))
        };
        if arch.is_empty() || rest.is_empty() {
            return unsupported("not a target triple");
        }
        if arch.starts_with("wasm") {
            return unsupported("wasm is not hot-patched by the native builder");
        }
        if has("windows") || has("msvc") || has("uwp") {
            if rest == ["pc", "windows", "msvc"] {
                return Ok(Self::Msvc);
            }
            return unsupported("only the `pc-windows-msvc` Windows targets are hot-patched");
        }
        let apple_os = ["darwin", "macos", "ios", "tvos", "watchos", "visionos"];
        if has("apple") || rest.iter().any(|part| apple_os.contains(part)) {
            return Ok(Self::Darwin);
        }
        let gnu_env = rest.iter().any(|part| {
            part.starts_with("gnu") || part.starts_with("musl") || part.starts_with("android")
        });
        if has("linux") || gnu_env {
            return Ok(Self::Gnu);
        }
        unsupported("unknown operating system or environment")
    }

    /// The file extension of a patch library: `dylib`, `so` or `dll`.
    pub fn patch_extension(self) -> &'static str {
        match self {
            Self::Darwin => "dylib",
            Self::Gnu => "so",
            Self::Msvc => "dll",
        }
    }

    /// `name` as it appears in this flavor's symbol tables (Mach-O prefixes
    /// C symbols with `_`).
    pub fn object_symbol(self, name: &str) -> String {
        match self {
            Self::Darwin => format!("_{name}"),
            Self::Gnu | Self::Msvc => name.to_string(),
        }
    }

    /// The driver argument that exports [`ANCHOR_SYMBOL`] from the linked
    /// image. On Darwin this also limits the export trie to the anchor; the
    /// symbol table still lists every function, which is all a jump table
    /// needs.
    pub fn anchor_export_arg(self) -> String {
        match self {
            Self::Darwin => format!("-Wl,-exported_symbol,{}", self.object_symbol(ANCHOR_SYMBOL)),
            Self::Gnu => format!("-Wl,--export-dynamic-symbol,{ANCHOR_SYMBOL}"),
            Self::Msvc => format!("/EXPORT:{ANCHOR_SYMBOL}"),
        }
    }

    /// The arguments that force-load every member of `archive`.
    fn force_load_args(self, archive: &Path) -> Vec<String> {
        let archive = render(archive);
        match self {
            Self::Darwin => vec!["-Wl,-force_load".to_string(), archive],
            Self::Gnu => vec![
                "-Wl,--whole-archive".to_string(),
                archive,
                "-Wl,--no-whole-archive".to_string(),
            ],
            Self::Msvc => vec![format!("/WHOLEARCHIVE:{archive}")],
        }
    }
}

/// The linker program to run: `custom` when the build configured one,
/// otherwise [`DEFAULT_LINKER`]. A raw linker (`ld64.lld`, `mold`, ...)
/// cannot take the driver-form arguments and is
/// [`HotpatchError::BuilderUnsupported`].
pub fn linker_program(custom: Option<&Path>) -> Result<String, HotpatchError> {
    let Some(custom) = custom else {
        return Ok(DEFAULT_LINKER.to_string());
    };
    let name = custom
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase())
        .unwrap_or_default();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    if stem.is_empty() || RAW_LINKERS.contains(&stem) {
        return Err(HotpatchError::unsupported(format!(
            "linker `{}` is not a `cc`-compatible driver",
            custom.display()
        )));
    }
    Ok(render(custom))
}

/// The packed `libdeps-<hash>.a` and what the link line still needs beside
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FatArchive {
    /// The archive, holding every `.rcgu.o` of the packed rlibs.
    pub path: PathBuf,
    /// rlibs that stay on the link line: toolchain rlibs outside the target
    /// dir, and packed rlibs that also carry non-`.rcgu.o` objects (bundled
    /// native code) the linker may still need. Captured order.
    pub kept_rlibs: Vec<PathBuf>,
}

/// Packs the `.rcgu.o` members of every rlib in `link_args` that lies under
/// `target_dir` into `<archive_dir>/libdeps-<hash>.a`. The hash covers each
/// packed rlib's path, size and modification time plus this crate's
/// version, so an unchanged dependency set reuses the archive written last
/// time; older `libdeps-*` files in `archive_dir` are removed. `Ok(None)`
/// when no rlib contributed an object (the link line is then left alone).
/// An unreadable rlib or member is [`HotpatchError::BuilderUnsupported`].
pub fn write_fat_archive(
    runner: &dyn ProcessRunner,
    flavor: LinkerFlavor,
    link_args: &[String],
    target_dir: &Path,
    archive_dir: &Path,
) -> Result<Option<FatArchive>, HotpatchError> {
    let rlibs: Vec<PathBuf> = link_args
        .iter()
        .filter(|arg| arg.ends_with(".rlib"))
        .map(PathBuf::from)
        .collect();
    // rustc canonicalises `--extern` paths, so a target dir reached through
    // a symlink (macOS `/var` -> `/private/var`) is compared in both forms.
    let canonical_target = host_path::canonicalize_simplified(target_dir).ok();
    let (packed, toolchain): (Vec<&PathBuf>, Vec<&PathBuf>) = rlibs.iter().partition(|rlib| {
        host_path::is_under(rlib, target_dir)
            || canonical_target.as_ref().is_some_and(|target| {
                host_path::canonicalize_simplified(rlib)
                    .is_ok_and(|rlib| host_path::is_under(&rlib, target))
            })
    });

    let mut hash = Fnv1a64::new();
    hash.write(env!("CARGO_PKG_VERSION").as_bytes());
    for rlib in &packed {
        let meta = fs::metadata(rlib)
            .map_err(|err| HotpatchError::io(format!("reading rlib `{}`", rlib.display()), err))?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|since| since.as_nanos())
            .unwrap_or_default();
        hash.write(render(rlib).as_bytes());
        hash.write(&meta.len().to_le_bytes());
        hash.write(&mtime.to_le_bytes());
    }
    let stem = format!("libdeps-{:016x}", hash.finish());
    let archive_path = archive_dir.join(format!("{stem}.a"));
    let kept_list = archive_dir.join(format!("{stem}.rlibs"));

    if archive_path.is_file()
        && let Ok(list) = fs::read_to_string(&kept_list)
    {
        let kept_packed: Vec<PathBuf> = list.lines().map(PathBuf::from).collect();
        return Ok(Some(FatArchive {
            path: archive_path,
            kept_rlibs: in_link_order(&rlibs, &toolchain, &kept_packed),
        }));
    }

    let mut members = 0usize;
    let mut kept_packed = Vec::new();
    let mut builder = ar::Builder::new(Vec::new());
    for rlib in &packed {
        let (added, has_other_objects) = pack_rlib(rlib, &mut builder)?;
        members += added;
        if has_other_objects {
            kept_packed.push(rlib.to_path_buf());
        }
    }
    if members == 0 {
        return Ok(None);
    }
    let bytes = builder
        .into_inner()
        .map_err(|err| HotpatchError::io("finalising the fat archive", err))?;

    fs::create_dir_all(archive_dir)
        .map_err(|err| HotpatchError::io(format!("creating `{}`", archive_dir.display()), err))?;
    remove_stale_archives(archive_dir, &stem);
    let partial = archive_dir.join(format!("{stem}.a.partial"));
    fs::write(&partial, &bytes)
        .and_then(|()| fs::rename(&partial, &archive_path))
        .map_err(|err| HotpatchError::io(format!("writing `{}`", archive_path.display()), err))?;
    if flavor == LinkerFlavor::Darwin {
        // ld64 wants a table of contents. A missing or failing `ranlib`
        // leaves the archive usable under `-force_load`, as in dioxus-cli.
        let _ = runner.run("ranlib", &[&render(&archive_path)]);
    }
    let list: Vec<String> = kept_packed.iter().map(|rlib| render(rlib)).collect();
    fs::write(&kept_list, list.join("\n"))
        .map_err(|err| HotpatchError::io(format!("writing `{}`", kept_list.display()), err))?;

    Ok(Some(FatArchive {
        path: archive_path,
        kept_rlibs: in_link_order(&rlibs, &toolchain, &kept_packed),
    }))
}

/// Appends `rlib`'s `.rcgu.o` members to `builder`. Returns how many were
/// added and whether the rlib also holds another kind of object.
fn pack_rlib(
    rlib: &Path,
    builder: &mut ar::Builder<Vec<u8>>,
) -> Result<(usize, bool), HotpatchError> {
    let bad = |what: String| {
        HotpatchError::unsupported(format!("unreadable rlib `{}`: {what}", rlib.display()))
    };
    let bytes = fs::read(rlib)
        .map_err(|err| HotpatchError::io(format!("reading rlib `{}`", rlib.display()), err))?;
    let mut archive = ar::Archive::new(bytes.as_slice());
    let mut added = 0;
    let mut has_other_objects = false;
    while let Some(entry) = archive.next_entry() {
        let mut entry = entry.map_err(|err| bad(err.to_string()))?;
        let name = std::str::from_utf8(entry.header().identifier())
            .map_err(|_| bad("a member name is not UTF-8".to_string()))?
            .to_string();
        let metadata = name.starts_with("lib.rmeta") || name.ends_with(".rmeta");
        let symbol_table = name.starts_with("__.SYMDEF") || name == "/" || name == "//";
        if metadata || symbol_table || entry.header().size() == 0 {
            continue;
        }
        if !name.ends_with(".rcgu.o") {
            has_other_objects = true;
            continue;
        }
        let header = entry.header().clone();
        let mut data = Vec::with_capacity(header.size() as usize);
        entry
            .read_to_end(&mut data)
            .map_err(|err| bad(format!("member `{name}`: {err}")))?;
        builder
            .append(&header, data.as_slice())
            .map_err(|err| HotpatchError::io("appending to the fat archive", err))?;
        added += 1;
    }
    Ok((added, has_other_objects))
}

/// `toolchain` plus `kept_packed`, in the order they appear in `rlibs`.
fn in_link_order(
    rlibs: &[PathBuf],
    toolchain: &[&PathBuf],
    kept_packed: &[PathBuf],
) -> Vec<PathBuf> {
    rlibs
        .iter()
        .filter(|rlib| toolchain.contains(rlib) || kept_packed.contains(rlib))
        .cloned()
        .collect()
}

fn remove_stale_archives(archive_dir: &Path, keep_stem: &str) {
    let Ok(entries) = fs::read_dir(archive_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with("libdeps-") && !name.starts_with(keep_stem) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The fat link line: `link_args` (captured, already expanded) with every
/// rlib replaced by `archive`'s force-load and its kept rlibs right after
/// the last object, the anchor export appended, and the output redirected
/// to `exe`. With no archive the rlibs stay as captured. No object
/// argument, or a missing or repeated `-o`, is
/// [`HotpatchError::BuilderUnsupported`].
///
/// Msvc follows dx (`link.rs`): `/WHOLEARCHIVE:<archive>` and the kept
/// rlibs go right *before* the last object, then `/HIGHENTROPYVA:NO` (dx:
/// "Prevent alsr from overflowing 32 bits"), the anchor's `/EXPORT:` and
/// `/OUT:<exe>` replace the captured `/OUT:`. A `-o` in an Msvc capture is
/// [`HotpatchError::BuilderUnsupported`].
pub fn fat_link_args(
    flavor: LinkerFlavor,
    link_args: &[String],
    archive: Option<&FatArchive>,
    exe: &Path,
) -> Result<Vec<String>, HotpatchError> {
    if link_args.is_empty() {
        return Err(HotpatchError::unsupported(
            "no captured linker arguments for the fat link",
        ));
    }
    if flavor == LinkerFlavor::Msvc {
        if link_args.iter().any(|arg| arg == "-o") {
            return Err(HotpatchError::unsupported(
                "captured linker arguments are not MSVC-style (`-o`)",
            ));
        }
    } else if link_args.iter().any(|arg| arg.starts_with("/OUT:")) {
        return Err(HotpatchError::unsupported(
            "captured linker arguments are MSVC-style (`/OUT:`)",
        ));
    }
    output_path(link_args)?;
    let last_object = link_args
        .iter()
        .rposition(|arg| arg.ends_with(".o"))
        .ok_or_else(|| {
            HotpatchError::unsupported("captured linker arguments name no object file")
        })?;

    let mut args = Vec::with_capacity(link_args.len() + 8);
    let mut skip_value = false;
    for (index, arg) in link_args.iter().enumerate() {
        if std::mem::take(&mut skip_value) {
            continue;
        }
        if arg == "-o" {
            skip_value = true;
            continue;
        }
        if flavor == LinkerFlavor::Msvc && arg.starts_with("/OUT:") {
            continue;
        }
        if archive.is_some() && arg.ends_with(".rlib") {
            continue;
        }
        if flavor == LinkerFlavor::Msvc
            && index == last_object
            && let Some(archive) = archive
        {
            args.extend(flavor.force_load_args(&archive.path));
            args.extend(archive.kept_rlibs.iter().map(|rlib| render(rlib)));
            args.push(arg.clone());
            continue;
        }
        args.push(arg.clone());
        if index == last_object
            && let Some(archive) = archive
        {
            args.extend(flavor.force_load_args(&archive.path));
            args.extend(archive.kept_rlibs.iter().map(|rlib| render(rlib)));
        }
    }
    if flavor == LinkerFlavor::Msvc {
        args.push(HIGH_ENTROPY_VA_OFF.to_string());
        args.push(flavor.anchor_export_arg());
        args.push(format!("/OUT:{}", render(exe)));
    } else {
        args.push(flavor.anchor_export_arg());
        args.push("-o".to_string());
        args.push(render(exe));
    }
    Ok(args)
}

/// Everything one fat link needs.
#[derive(Debug, Clone, Copy)]
pub struct FatLinkRequest<'a> {
    pub flavor: LinkerFlavor,
    /// From [`linker_program`].
    pub linker: &'a str,
    /// The tip's captured linker arguments
    /// ([`read_link_args`](super::link_intercept::read_link_args)).
    pub link_args: &'a [String],
    /// Extra environment for the linker: the tip's captured rustc
    /// environment.
    pub envs: &'a [(String, String)],
    /// The cargo target dir; rlibs under it are packed.
    pub target_dir: &'a Path,
    /// Where `libdeps-<hash>.a` lives: one directory per build
    /// configuration, since older archives in it are removed.
    pub archive_dir: &'a Path,
    /// The fat executable to write.
    pub exe: &'a Path,
}

/// A successful fat link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FatLinkOutput {
    pub exe: PathBuf,
    pub archive: Option<FatArchive>,
    /// The link-time address of [`ANCHOR_SYMBOL`] in `exe`.
    pub anchor_address: u64,
    /// The linker's diagnostics (warnings), if it printed any.
    pub linker_output: String,
}

/// Packs the archive, runs the fat link through `runner`, and checks that
/// the image defines [`ANCHOR_SYMBOL`]. A failed link or a missing anchor
/// is [`HotpatchError::BuilderUnsupported`] (the session restarts instead);
/// a linker that cannot be spawned is [`HotpatchError::Process`].
pub fn fat_link(
    runner: &dyn ProcessRunner,
    request: &FatLinkRequest<'_>,
) -> Result<FatLinkOutput, HotpatchError> {
    let archive = write_fat_archive(
        runner,
        request.flavor,
        request.link_args,
        request.target_dir,
        request.archive_dir,
    )?;
    let args = fat_link_args(
        request.flavor,
        request.link_args,
        archive.as_ref(),
        request.exe,
    )?;
    if let Some(parent) = request.exe.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|err| HotpatchError::io(format!("creating `{}`", parent.display()), err))?;
    }
    let linker_output = run_linker(runner, request.linker, &args, request.envs, "fat link")?;
    let anchor_address = anchor_address(request.flavor, request.exe)?;
    Ok(FatLinkOutput {
        exe: request.exe.to_path_buf(),
        archive,
        anchor_address,
        linker_output,
    })
}

/// Runs `linker args` with `envs` added and returns its combined output. A
/// non-zero exit is [`HotpatchError::BuilderUnsupported`] naming `what`.
pub(crate) fn run_linker(
    runner: &dyn ProcessRunner,
    linker: &str,
    args: &[String],
    envs: &[(String, String)],
    what: &str,
) -> Result<String, HotpatchError> {
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let env: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let result = runner
        .run_streaming(linker, &argv, None, &env, &mut |_| {})
        .map_err(|err| HotpatchError::Process {
            detail: format!("failed to spawn linker `{linker}`: {err:#}"),
        })?;
    let combined = [result.stdout.trim(), result.stderr.trim()]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if !result.success {
        let detail = if combined.is_empty() {
            "no output".to_string()
        } else {
            tail_lines(&combined, 40)
        };
        return Err(HotpatchError::unsupported(format!(
            "{what} failed (`{linker}`): {detail}"
        )));
    }
    Ok(combined)
}

/// The address of the global definition of [`ANCHOR_SYMBOL`] in the image
/// at `path`, read from its symbol table — for Msvc, the anchor's RVA from
/// the image's own PDB ([`super::pe::anchor_address`], Windows hosts only).
/// An unreadable image, or none such symbol, is
/// [`HotpatchError::BuilderUnsupported`].
pub fn anchor_address(flavor: LinkerFlavor, path: &Path) -> Result<u64, HotpatchError> {
    use object::{Object, ObjectSymbol};

    if flavor == LinkerFlavor::Msvc {
        return super::pe::anchor_address(path);
    }

    let bytes = fs::read(path).map_err(|err| {
        HotpatchError::io(format!("reading linked image `{}`", path.display()), err)
    })?;
    let file = object::File::parse(bytes.as_slice()).map_err(|err| {
        HotpatchError::unsupported(format!(
            "linked image `{}` does not parse: {err}",
            path.display()
        ))
    })?;
    let wanted = flavor.object_symbol(ANCHOR_SYMBOL);
    file.symbols()
        .find(|symbol| {
            symbol.is_definition()
                && symbol.is_global()
                && symbol
                    .name_bytes()
                    .is_ok_and(|name| name == wanted.as_bytes())
        })
        .map(|symbol| symbol.address())
        .ok_or_else(|| {
            HotpatchError::unsupported(format!(
                "linked image `{}` defines no global `{wanted}`; the app must be built with \
                 `frust::app!` and the `hotpatch` feature",
                path.display()
            ))
        })
}

/// A path as a linker argument.
pub(crate) fn render(path: &Path) -> String {
    host_path::simplify(path).to_string_lossy().into_owned()
}

/// The fixed 64-bit FNV-1a hash: the archive name only has to change when
/// its inputs do, and must be stable across runs (unlike `std`'s
/// `DefaultHasher`).
struct Fnv1a64(u64);

impl Fnv1a64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0 ^= bytes.len() as u64;
        self.0 = self.0.wrapping_mul(Self::PRIME);
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// Test doubles shared with [`thin_link`](super::thin_link)'s tests.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::process::{Output, StreamHandle};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// One recorded invocation.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Call {
        pub cmd: String,
        pub args: Vec<String>,
        pub env: Vec<(String, String)>,
    }

    /// Records every invocation, answers `success`, and writes `image` to a
    /// streamed invocation's `-o` path (a stand-in for the linked output).
    pub struct RecordingRunner {
        pub calls: Mutex<Vec<Call>>,
        pub success: bool,
        pub stderr: String,
        pub image: Option<Vec<u8>>,
    }

    impl RecordingRunner {
        pub fn linking(image: Vec<u8>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                success: true,
                stderr: String::new(),
                image: Some(image),
            }
        }

        pub fn failing(stderr: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                success: false,
                stderr: stderr.to_string(),
                image: None,
            }
        }

        pub fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        /// The one call to `cmd`.
        pub fn only_call(&self, cmd: &str) -> Call {
            let calls: Vec<Call> = self.calls().into_iter().filter(|c| c.cmd == cmd).collect();
            assert_eq!(calls.len(), 1, "expected one `{cmd}` call: {calls:?}");
            calls.into_iter().next().unwrap()
        }

        fn record(&self, cmd: &str, args: &[&str], env: &[(&str, &str)]) -> Output {
            self.calls.lock().unwrap().push(Call {
                cmd: cmd.to_string(),
                args: args.iter().map(|a| a.to_string()).collect(),
                env: env
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            });
            Output {
                success: self.success,
                stdout: String::new(),
                stderr: self.stderr.clone(),
            }
        }
    }

    impl ProcessRunner for RecordingRunner {
        fn run(&self, cmd: &str, args: &[&str]) -> anyhow::Result<Output> {
            Ok(self.record(cmd, args, &[]))
        }

        fn run_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            _cwd: Option<&Path>,
            env: &[(&str, &str)],
            _on_line: &mut dyn FnMut(&str),
        ) -> anyhow::Result<Output> {
            let output = self.record(cmd, args, env);
            if let (Some(image), Some(pos)) = (&self.image, args.iter().position(|a| *a == "-o")) {
                fs::write(args[pos + 1], image)?;
            }
            if let (Some(image), Some(out)) = (
                &self.image,
                args.iter().find_map(|a| a.strip_prefix("/OUT:")),
            ) {
                fs::write(out, image)?;
            }
            Ok(output)
        }

        fn spawn_streaming(
            &self,
            _cmd: &str,
            _args: &[&str],
            _cwd: Option<&Path>,
            _env: &[(&str, &str)],
        ) -> anyhow::Result<StreamHandle> {
            anyhow::bail!("RecordingRunner does not spawn streams")
        }
    }

    /// A relocatable object in `flavor`'s format that defines `symbols`
    /// (exact symbol-table names) as global functions.
    pub fn image_defining(flavor: LinkerFlavor, symbols: &[&str]) -> Vec<u8> {
        use object::write::{Mangling, Object, StandardSection, Symbol, SymbolSection};
        use object::{SymbolFlags, SymbolKind, SymbolScope};

        let format = match flavor {
            LinkerFlavor::Darwin => object::BinaryFormat::MachO,
            LinkerFlavor::Gnu => object::BinaryFormat::Elf,
            LinkerFlavor::Msvc => object::BinaryFormat::Coff,
        };
        let mut obj = Object::new(
            format,
            object::Architecture::Aarch64,
            object::Endianness::Little,
        );
        obj.set_mangling(Mangling::None);
        let text = obj.section_id(StandardSection::Text);
        for name in symbols {
            let offset = obj.append_section_data(text, &[0xc0, 0x03, 0x5f, 0xd6], 4);
            obj.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value: offset,
                size: 4,
                kind: SymbolKind::Text,
                scope: SymbolScope::Dynamic,
                weak: false,
                section: SymbolSection::Section(text),
                flags: SymbolFlags::None,
            });
        }
        obj.write().unwrap()
    }

    /// A fresh, empty directory under the system temp dir.
    pub fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("frust-drive-link-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes an rlib-shaped archive holding `members` (name, bytes).
    pub fn write_rlib(path: &Path, members: &[(&str, &[u8])]) {
        let mut builder = ar::Builder::new(Vec::new());
        for (name, bytes) in members {
            let header = ar::Header::new(name.as_bytes().to_vec(), bytes.len() as u64);
            builder.append(&header, *bytes).unwrap();
        }
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, builder.into_inner().unwrap()).unwrap();
    }

    /// The member names of the archive at `path`, read back with `ar`.
    pub fn archive_members(path: &Path) -> Vec<String> {
        let bytes = fs::read(path).unwrap();
        let mut archive = ar::Archive::new(bytes.as_slice());
        let mut names = Vec::new();
        while let Some(entry) = archive.next_entry() {
            let entry = entry.unwrap();
            names.push(String::from_utf8(entry.header().identifier().to_vec()).unwrap());
        }
        names
    }

    pub fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    pub fn unsupported<T: std::fmt::Debug>(result: Result<T, HotpatchError>) -> String {
        match result {
            Err(HotpatchError::BuilderUnsupported { detail }) => detail,
            other => panic!("expected BuilderUnsupported, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    /// A fat-build capture: the tip's objects, two workspace rlibs under
    /// `target`, one of them bundling a native object, and a toolchain rlib.
    struct Fixture {
        target: PathBuf,
        archive_dir: PathBuf,
        exe: PathBuf,
        link_args: Vec<String>,
        dep_a: PathBuf,
        dep_native: PathBuf,
        std_rlib: PathBuf,
    }

    fn fixture(tag: &str) -> Fixture {
        let root = temp_dir(tag);
        let target = root.join("target");
        let deps = target.join("debug").join("deps");
        let dep_a = deps.join("libdep_a-1111.rlib");
        let dep_native = deps.join("libdep_native-2222.rlib");
        let std_rlib = root.join("toolchain").join("libstd-3333.rlib");
        write_rlib(
            &dep_a,
            &[
                ("lib.rmeta", b"meta"),
                ("dep_a-1111.dep_a.aaaa-cgu.0.rcgu.o", b"obj-a0"),
                ("dep_a-1111.dep_a.aaaa-cgu.1.rcgu.o", b"obj-a1"),
                ("lib.rmeta-link", b"link"),
            ],
        );
        write_rlib(
            &dep_native,
            &[
                ("lib.rmeta", b"meta"),
                ("dep_native-2222.dep_native.bbbb-cgu.0.rcgu.o", b"obj-n0"),
                ("shim.o", b"native"),
            ],
        );
        write_rlib(&std_rlib, &[("std-3333.std.cccc-cgu.0.rcgu.o", b"std")]);
        let tip_object = deps.join("app-4444.app.dddd-cgu.0.rcgu.o");
        let link_args = vec![
            "/t/rustcXYZ/symbols.o".to_string(),
            render(&tip_object),
            render(&dep_a),
            render(&dep_native),
            render(&std_rlib),
            "-lSystem".to_string(),
            "-arch".to_string(),
            "arm64".to_string(),
            "-o".to_string(),
            render(&deps.join("app-4444")),
            "-nodefaultlibs".to_string(),
        ];
        Fixture {
            archive_dir: target.join("frust-hotpatch").join("fat"),
            exe: target.join("debug").join("app"),
            target,
            link_args,
            dep_a,
            dep_native,
            std_rlib,
        }
    }

    #[test]
    fn flavor_follows_the_target_triple() {
        for triple in [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "aarch64-apple-ios-sim",
            "aarch64-apple-ios-macabi",
        ] {
            assert_eq!(
                LinkerFlavor::for_triple(triple).unwrap(),
                LinkerFlavor::Darwin,
                "{triple}"
            );
        }
        for triple in [
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-unknown-linux-musl",
            "aarch64-linux-android",
            "armv7-linux-androideabi",
        ] {
            assert_eq!(
                LinkerFlavor::for_triple(triple).unwrap(),
                LinkerFlavor::Gnu,
                "{triple}"
            );
        }
    }

    #[test]
    fn an_unknown_flavor_is_builder_unsupported() {
        for triple in [
            "",
            "x86_64",
            "x86_64-pc-windows-gnu",
            "aarch64-pc-windows-gnullvm",
            "x86_64-uwp-windows-msvc",
            "wasm32-unknown-unknown",
            "riscv32imc-unknown-none-elf",
            "x86_64-unknown-freebsd",
        ] {
            unsupported(LinkerFlavor::for_triple(triple));
        }
    }

    #[test]
    fn the_linker_is_cc_unless_a_driver_is_configured() {
        assert_eq!(linker_program(None).unwrap(), "cc");
        assert_eq!(
            linker_program(Some(Path::new("/ndk/bin/aarch64-linux-android24-clang"))).unwrap(),
            "/ndk/bin/aarch64-linux-android24-clang"
        );
        for raw in [
            "/usr/bin/ld64.lld",
            "mold",
            "C:/llvm/lld-link.exe",
            "rust-lld",
        ] {
            unsupported(linker_program(Some(Path::new(raw))));
        }
    }

    #[test]
    fn the_archive_packs_every_rcgu_object_of_the_target_dir_rlibs() {
        let fx = fixture("archive");
        let runner = RecordingRunner::linking(Vec::new());
        let archive = write_fat_archive(
            &runner,
            LinkerFlavor::Darwin,
            &fx.link_args,
            &fx.target,
            &fx.archive_dir,
        )
        .unwrap()
        .expect("objects were packed");
        let name = archive.path.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("libdeps-") && name.ends_with(".a"),
            "{name}"
        );
        assert_eq!(
            archive_members(&archive.path),
            strings(&[
                "dep_a-1111.dep_a.aaaa-cgu.0.rcgu.o",
                "dep_a-1111.dep_a.aaaa-cgu.1.rcgu.o",
                "dep_native-2222.dep_native.bbbb-cgu.0.rcgu.o",
            ])
        );
        assert_eq!(
            archive.kept_rlibs,
            vec![fx.dep_native.clone(), fx.std_rlib.clone()],
            "the bundling rlib and the toolchain rlib stay on the link line"
        );
        let ranlib = runner.only_call("ranlib");
        assert_eq!(ranlib.args, vec![render(&archive.path)]);
    }

    #[test]
    fn an_unchanged_dependency_set_reuses_the_archive_and_a_changed_one_replaces_it() {
        let fx = fixture("reuse");
        let runner = RecordingRunner::linking(Vec::new());
        let pack = || {
            write_fat_archive(
                &runner,
                LinkerFlavor::Gnu,
                &fx.link_args,
                &fx.target,
                &fx.archive_dir,
            )
            .unwrap()
            .unwrap()
        };
        let first = pack();
        assert_eq!(pack(), first);
        assert!(runner.calls().is_empty(), "Gnu archives are not ranlib'd");

        write_rlib(
            &fx.dep_a,
            &[("dep_a-1111.dep_a.aaaa-cgu.0.rcgu.o", b"edited-and-longer")],
        );
        let second = pack();
        assert_ne!(second.path, first.path);
        assert!(!first.path.exists(), "the stale archive is removed");
        assert_eq!(
            archive_members(&second.path),
            strings(&[
                "dep_a-1111.dep_a.aaaa-cgu.0.rcgu.o",
                "dep_native-2222.dep_native.bbbb-cgu.0.rcgu.o",
            ])
        );
    }

    #[test]
    fn no_packable_object_means_no_archive() {
        let fx = fixture("empty");
        let only_std: Vec<String> = fx
            .link_args
            .iter()
            .filter(|arg| !arg.contains("libdep_"))
            .cloned()
            .collect();
        let runner = RecordingRunner::linking(Vec::new());
        let archive = write_fat_archive(
            &runner,
            LinkerFlavor::Darwin,
            &only_std,
            &fx.target,
            &fx.archive_dir,
        )
        .unwrap();
        assert_eq!(archive, None);
    }

    #[test]
    fn a_corrupt_rlib_is_builder_unsupported() {
        let fx = fixture("corrupt");
        fs::write(&fx.dep_a, b"!<arch>\nnot a header").unwrap();
        let runner = RecordingRunner::linking(Vec::new());
        unsupported(write_fat_archive(
            &runner,
            LinkerFlavor::Darwin,
            &fx.link_args,
            &fx.target,
            &fx.archive_dir,
        ));
    }

    fn fat_link_of(fx: &Fixture, flavor: LinkerFlavor) -> (RecordingRunner, FatLinkOutput) {
        let image = image_defining(
            flavor,
            &[
                &flavor.object_symbol(ANCHOR_SYMBOL),
                &flavor.object_symbol("main"),
            ],
        );
        let runner = RecordingRunner::linking(image);
        let envs = vec![("CARGO_PKG_NAME".to_string(), "app".to_string())];
        let output = fat_link(
            &runner,
            &FatLinkRequest {
                flavor,
                linker: "cc",
                link_args: &fx.link_args,
                envs: &envs,
                target_dir: &fx.target,
                archive_dir: &fx.archive_dir,
                exe: &fx.exe,
            },
        )
        .unwrap();
        (runner, output)
    }

    #[test]
    fn darwin_fat_link_force_loads_the_archive_and_exports_the_anchor() {
        let fx = fixture("darwin");
        let (runner, output) = fat_link_of(&fx, LinkerFlavor::Darwin);
        let archive = output.archive.clone().unwrap();
        let link = runner.only_call("cc");
        let tip = fx.link_args[1].clone();
        assert_eq!(
            link.args,
            vec![
                "/t/rustcXYZ/symbols.o".to_string(),
                tip,
                "-Wl,-force_load".to_string(),
                render(&archive.path),
                render(&fx.dep_native),
                render(&fx.std_rlib),
                "-lSystem".to_string(),
                "-arch".to_string(),
                "arm64".to_string(),
                "-nodefaultlibs".to_string(),
                "-Wl,-exported_symbol,___frust_hotpatch_anchor".to_string(),
                "-o".to_string(),
                render(&fx.exe),
            ]
        );
        assert!(
            !link.args.contains(&render(&fx.dep_a)),
            "packed rlib dropped"
        );
        assert_eq!(
            link.env,
            vec![("CARGO_PKG_NAME".to_string(), "app".to_string())]
        );
        assert_eq!(
            archive_members(&archive.path).len(),
            3,
            "every .rcgu.o of the fixture rlibs"
        );
        assert_eq!(output.exe, fx.exe);
        assert_eq!(output.anchor_address, 0);
    }

    #[test]
    fn gnu_fat_link_whole_archives_the_archive_and_exports_the_anchor() {
        let fx = fixture("gnu");
        let (runner, output) = fat_link_of(&fx, LinkerFlavor::Gnu);
        let archive = output.archive.unwrap();
        let link = runner.only_call("cc");
        let at = link
            .args
            .iter()
            .position(|arg| arg == "-Wl,--whole-archive")
            .expect("whole-archive flag");
        assert_eq!(
            link.args[at..at + 5],
            [
                "-Wl,--whole-archive".to_string(),
                render(&archive.path),
                "-Wl,--no-whole-archive".to_string(),
                render(&fx.dep_native),
                render(&fx.std_rlib),
            ]
        );
        assert_eq!(
            link.args[at - 1],
            fx.link_args[1],
            "right after the last object"
        );
        assert_eq!(
            link.args[link.args.len() - 3..],
            [
                "-Wl,--export-dynamic-symbol,__frust_hotpatch_anchor".to_string(),
                "-o".to_string(),
                render(&fx.exe),
            ]
        );
        assert_eq!(
            link.args.iter().filter(|arg| *arg == "-o").count(),
            1,
            "the captured -o is replaced"
        );
        assert!(runner.calls().iter().all(|call| call.cmd != "ranlib"));
    }

    #[test]
    fn msvc_flavor_is_the_pc_windows_msvc_targets() {
        for triple in ["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"] {
            assert_eq!(
                LinkerFlavor::for_triple(triple).unwrap(),
                LinkerFlavor::Msvc,
                "{triple}"
            );
        }
        let msvc = LinkerFlavor::Msvc;
        assert_eq!(msvc.patch_extension(), "dll");
        assert_eq!(msvc.object_symbol(ANCHOR_SYMBOL), ANCHOR_SYMBOL);
        assert_eq!(msvc.anchor_export_arg(), "/EXPORT:__frust_hotpatch_anchor");
    }

    /// A captured MSVC fat-build line: rustc's `symbols.o`, the tip's
    /// objects, the rlibs, std's import libraries and `/OUT:`.
    fn msvc_link_args(fx: &Fixture) -> Vec<String> {
        let deps = fx.target.join("debug").join("deps");
        vec![
            "/NOLOGO".to_string(),
            "C:/t/rustcXYZ/symbols.o".to_string(),
            render(&deps.join("app-4444.app.aaaa-cgu.0.rcgu.o")),
            render(&deps.join("app-4444.app.bbbb-cgu.1.rcgu.o")),
            render(&fx.dep_a),
            render(&fx.dep_native),
            render(&fx.std_rlib),
            "kernel32.lib".to_string(),
            "/defaultlib:msvcrt".to_string(),
            "/NXCOMPAT".to_string(),
            format!("/LIBPATH:{}", render(&deps)),
            format!("/OUT:{}", render(&deps.join("app-4444.exe"))),
            "/OPT:NOREF,NOICF".to_string(),
            "/DEBUG".to_string(),
            "/PDBALTPATH:%_PDB%".to_string(),
        ]
    }

    #[test]
    fn msvc_fat_link_whole_archives_before_the_last_object_and_exports_the_anchor() {
        let fx = fixture("msvc");
        let link_args = msvc_link_args(&fx);
        let exe = fx.target.join("debug").join("app.exe");
        let runner = RecordingRunner::linking(b"MZ stand-in".to_vec());
        let request = FatLinkRequest {
            flavor: LinkerFlavor::Msvc,
            linker: "link.exe",
            link_args: &link_args,
            envs: &[],
            target_dir: &fx.target,
            archive_dir: &fx.archive_dir,
            exe: &exe,
        };
        // The stand-in image has no PDB, so reading its anchor is refused
        // (off Windows: no PDB reader at all) after the link ran.
        unsupported(fat_link(&runner, &request));
        let link = runner.only_call("link.exe");
        let archive = fs::read_dir(&fx.archive_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "a"))
            .expect("the fat archive was written");
        assert_eq!(
            link.args,
            vec![
                "/NOLOGO".to_string(),
                "C:/t/rustcXYZ/symbols.o".to_string(),
                link_args[2].clone(),
                format!("/WHOLEARCHIVE:{}", render(&archive)),
                render(&fx.dep_native),
                render(&fx.std_rlib),
                link_args[3].clone(),
                "kernel32.lib".to_string(),
                "/defaultlib:msvcrt".to_string(),
                "/NXCOMPAT".to_string(),
                link_args[10].clone(),
                "/OPT:NOREF,NOICF".to_string(),
                "/DEBUG".to_string(),
                "/PDBALTPATH:%_PDB%".to_string(),
                "/HIGHENTROPYVA:NO".to_string(),
                "/EXPORT:__frust_hotpatch_anchor".to_string(),
                format!("/OUT:{}", render(&exe)),
            ]
        );
        assert!(fs::read(&exe).is_ok(), "the link wrote /OUT:");
        assert!(
            runner.calls().iter().all(|call| call.cmd != "ranlib"),
            "only Darwin archives are ranlib'd"
        );
    }

    #[test]
    fn msvc_and_cc_dialects_do_not_mix() {
        let exe = Path::new("C:/t/app.exe");
        let detail = unsupported(fat_link_args(
            LinkerFlavor::Msvc,
            &strings(&["a.o", "-o", "/t/x"]),
            None,
            exe,
        ));
        assert!(detail.contains("-o"), "{detail}");
        unsupported(fat_link_args(
            LinkerFlavor::Msvc,
            &strings(&["a.o", "/OUT:a.exe", "/OUT:b.exe"]),
            None,
            exe,
        ));
        unsupported(fat_link_args(
            LinkerFlavor::Msvc,
            &strings(&["lib.rlib", "/OUT:a.exe"]),
            None,
            exe,
        ));
        let args = fat_link_args(
            LinkerFlavor::Msvc,
            &strings(&["a.o", "libdep.rlib", "/OUT:a.exe"]),
            None,
            exe,
        )
        .unwrap();
        assert_eq!(
            args,
            strings(&[
                "a.o",
                "libdep.rlib",
                "/HIGHENTROPYVA:NO",
                "/EXPORT:__frust_hotpatch_anchor",
                "/OUT:C:/t/app.exe",
            ])
        );
    }

    #[test]
    fn a_fat_link_without_an_archive_keeps_the_captured_rlibs() {
        let fx = fixture("no-archive");
        let args = fat_link_args(LinkerFlavor::Gnu, &fx.link_args, None, &fx.exe).unwrap();
        assert!(args.contains(&render(&fx.dep_a)));
        assert!(!args.iter().any(|arg| arg.contains("whole-archive")));
        assert!(args.contains(&"-Wl,--export-dynamic-symbol,__frust_hotpatch_anchor".to_string()));
    }

    #[test]
    fn malformed_captures_are_builder_unsupported() {
        let exe = Path::new("/t/app");
        unsupported(fat_link_args(LinkerFlavor::Darwin, &[], None, exe));
        unsupported(fat_link_args(
            LinkerFlavor::Darwin,
            &strings(&["libfoo.rlib", "-o", "/t/x"]),
            None,
            exe,
        ));
        unsupported(fat_link_args(
            LinkerFlavor::Darwin,
            &strings(&["a.o", "-lc"]),
            None,
            exe,
        ));
        unsupported(fat_link_args(
            LinkerFlavor::Gnu,
            &strings(&["a.o", "/OUT:x.exe", "-o", "/t/x"]),
            None,
            exe,
        ));
    }

    #[test]
    fn an_image_without_the_anchor_is_builder_unsupported() {
        let fx = fixture("no-anchor");
        let runner = RecordingRunner::linking(image_defining(LinkerFlavor::Darwin, &["_main"]));
        let detail = unsupported(fat_link(
            &runner,
            &FatLinkRequest {
                flavor: LinkerFlavor::Darwin,
                linker: "cc",
                link_args: &fx.link_args,
                envs: &[],
                target_dir: &fx.target,
                archive_dir: &fx.archive_dir,
                exe: &fx.exe,
            },
        ));
        assert!(detail.contains("___frust_hotpatch_anchor"), "{detail}");
    }

    #[test]
    fn a_failing_linker_is_builder_unsupported_and_a_missing_one_a_process_error() {
        let fx = fixture("link-fail");
        let request = FatLinkRequest {
            flavor: LinkerFlavor::Gnu,
            linker: "cc",
            link_args: &fx.link_args,
            envs: &[],
            target_dir: &fx.target,
            archive_dir: &fx.archive_dir,
            exe: &fx.exe,
        };
        let failing = RecordingRunner::failing("undefined reference to `foo'");
        let detail = unsupported(fat_link(&failing, &request));
        assert!(detail.contains("undefined reference"), "{detail}");

        let missing = crate::process::FakeProcessRunner::new().missing("cc");
        assert!(matches!(
            fat_link(&missing, &request),
            Err(HotpatchError::Process { .. })
        ));
    }

    /// A real fat link on the host: rustc builds a dependency rlib and the
    /// tip (with the fat build's `-Csave-temps=true -Clink-dead-code`),
    /// a capture script stands in as the no-link linker, and [`fat_link`]
    /// links the image with the host `cc`. The dependency's mangled `pub fn`
    /// is never referenced (unlike a `no_mangle` export, which rustc's
    /// `symbols.o` references), so it is present only because the archive
    /// was force-loaded.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_real_fat_link_exports_the_anchor_and_keeps_unreferenced_dependency_code() {
        use crate::process::RealProcessRunner;
        use object::{Object, ObjectSymbol};
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("real");
        let deps = root.join("target").join("debug").join("deps");
        fs::create_dir_all(&deps).unwrap();
        fs::write(
            root.join("dep.rs"),
            "#[inline(never)]\npub fn frust_fixture_unreferenced() -> u32 { 7 }\n",
        )
        .unwrap();
        fs::write(
            root.join("main.rs"),
            "extern crate dep;\n#[unsafe(no_mangle)]\npub extern \"C\" fn __frust_hotpatch_anchor() {}\n\
             fn main() { __frust_hotpatch_anchor(); }\n",
        )
        .unwrap();
        let args_file = root.join("link-args.txt");
        let script = root.join("frust");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n\
                 while [ $# -gt 0 ]; do if [ \"$1\" = \"-o\" ]; then : > \"$2\"; fi; shift; done\n",
                args_file.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let runner = RealProcessRunner;
        let rustc = |args: &[&str]| {
            let out = runner.run("rustc", args).unwrap();
            assert!(out.success, "rustc {args:?}: {}", out.stderr);
        };
        let deps_s = render(&deps);
        rustc(&[
            "--edition=2024",
            "--crate-type=rlib",
            "--crate-name=dep",
            "-Cextra-filename=-1111",
            "--out-dir",
            &deps_s,
            &render(&root.join("dep.rs")),
        ]);
        rustc(&[
            "--edition=2024",
            "--crate-type=bin",
            "--crate-name=app",
            "-Cextra-filename=-2222",
            "-Csave-temps=true",
            "-Clink-dead-code",
            &format!("-Clinker={}", render(&script)),
            "--extern",
            &format!("dep={}", render(&deps.join("libdep-1111.rlib"))),
            "--out-dir",
            &deps_s,
            &render(&root.join("main.rs")),
        ]);
        let captured: Vec<String> = fs::read_to_string(&args_file)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        let link_args = super::super::link_intercept::expand_response_files(&captured).unwrap();

        let target = root.join("target");
        let exe = target.join("debug").join("app");
        let output = fat_link(
            &runner,
            &FatLinkRequest {
                flavor: LinkerFlavor::Darwin,
                linker: DEFAULT_LINKER,
                link_args: &link_args,
                envs: &[],
                target_dir: &target,
                archive_dir: &target.join("frust-hotpatch").join("fat"),
                exe: &exe,
            },
        )
        .unwrap();
        let archive = output.archive.expect("the dep rlib was packed");
        assert_eq!(archive_members(&archive.path).len(), 1);
        assert_ne!(output.anchor_address, 0);

        let bytes = fs::read(&exe).unwrap();
        let file = object::File::parse(bytes.as_slice()).unwrap();
        assert_eq!(file.kind(), object::ObjectKind::Executable);
        let defined: Vec<String> = file
            .symbols()
            .filter(|s| s.is_definition())
            .filter_map(|s| s.name().ok().map(str::to_string))
            .collect();
        assert!(
            defined
                .iter()
                .any(|s| s.contains("frust_fixture_unreferenced")),
            "the force-loaded dependency code is linked"
        );
        let exported: Vec<String> = file
            .exports()
            .unwrap()
            .iter()
            .map(|e| String::from_utf8_lossy(e.name()).into_owned())
            .collect();
        assert!(
            exported.contains(&"___frust_hotpatch_anchor".to_string()),
            "{exported:?}"
        );
        let run = runner.run(&render(&exe), &[]).unwrap();
        assert!(run.success, "the fat image runs: {}", run.stderr);
    }
}
