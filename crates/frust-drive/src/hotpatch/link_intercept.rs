//! Linker interception: `frust` stands in as rustc's linker
//! (`-Clinker=<frust>`) for fat and thin builds, and environment variables
//! select what it does with the link step rustc hands it.
//!
//! - **No-link** ([`LinkMode::NoLink`], fat and thin desktop builds): write
//!   the linker arguments to [`ENV_ARGS_FILE`] and emit an empty object at
//!   the `-o`/`/OUT:` path, so rustc's post-link steps find a file and
//!   succeed. The builder performs the real link itself, from those
//!   arguments, once it has every object it needs.
//! - **Proxy** ([`LinkMode::Proxy`]): capture the arguments the same way,
//!   then forward the untouched invocation to a real linker (Android's NDK
//!   clang, where the build must produce a real library).
//!
//! The design is dioxus-cli 0.7.10's `cli/link.rs` without `target-lexicon`:
//! the empty object is written in the host's own object format, like dx's
//! `Triple::host()`, and the output path is read from the arguments rather
//! than inferred from a target triple.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::host_path;
use crate::process::ProcessRunner;

use super::HotpatchError;

/// Selects the link action: [`LinkMode::NoLink`] (`no-link`) or
/// [`LinkMode::Proxy`] (`proxy`). Unset means `frust` was not asked to act
/// as a linker.
pub const ENV_LINK: &str = "FRUST_HOTPATCH_LINK";
/// Where the captured linker arguments are written (a JSON string array).
pub const ENV_ARGS_FILE: &str = "FRUST_HOTPATCH_LINK_ARGS_FILE";
/// Optional: where a linker failure or a proxied linker's warnings are
/// written, for the driving process to surface (rustc itself discards a
/// successful linker's output).
pub const ENV_ERR_FILE: &str = "FRUST_HOTPATCH_LINK_ERR_FILE";
/// The real linker a [`LinkMode::Proxy`] link forwards to.
pub const ENV_LINKER: &str = "FRUST_HOTPATCH_LINKER";

const MODE_NO_LINK: &str = "no-link";
const MODE_PROXY: &str = "proxy";

/// What `frust` does with an intercepted link step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkMode {
    /// Capture the arguments and emit an empty object; nothing is linked.
    NoLink,
    /// Capture the arguments and forward the invocation to `linker`.
    Proxy { linker: PathBuf },
}

/// A link action, as carried from the driving process to the `frust`
/// linker process through the environment ([`LinkAction::env_vars`] /
/// [`LinkAction::from_lookup`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkAction {
    pub mode: LinkMode,
    pub args_file: PathBuf,
    pub err_file: Option<PathBuf>,
}

impl LinkAction {
    /// Reads the action from an environment lookup. `Ok(None)` when
    /// [`ENV_LINK`] is unset. An unknown mode, a missing args file, or a
    /// linker that contradicts the mode is
    /// [`HotpatchError::BuilderUnsupported`].
    pub fn from_lookup(
        get: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, HotpatchError> {
        let Some(mode) = get(ENV_LINK) else {
            return Ok(None);
        };
        let args_file = get(ENV_ARGS_FILE)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| {
                HotpatchError::unsupported(format!(
                    "{ENV_LINK}={mode} is set without {ENV_ARGS_FILE}"
                ))
            })?;
        let err_file = get(ENV_ERR_FILE)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        let linker = get(ENV_LINKER).filter(|path| !path.is_empty());
        let mode = match (mode.as_str(), linker) {
            (MODE_NO_LINK, None) => LinkMode::NoLink,
            (MODE_PROXY, Some(linker)) => LinkMode::Proxy {
                linker: PathBuf::from(linker),
            },
            (MODE_NO_LINK, Some(linker)) => {
                return Err(HotpatchError::unsupported(format!(
                    "{ENV_LINK}={MODE_NO_LINK} names a real linker ({ENV_LINKER}={linker})"
                )));
            }
            (MODE_PROXY, None) => {
                return Err(HotpatchError::unsupported(format!(
                    "{ENV_LINK}={MODE_PROXY} is set without {ENV_LINKER}"
                )));
            }
            (other, _) => {
                return Err(HotpatchError::unsupported(format!(
                    "unknown {ENV_LINK} mode `{other}` (expected `{MODE_NO_LINK}` or `{MODE_PROXY}`)"
                )));
            }
        };
        Ok(Some(Self {
            mode,
            args_file,
            err_file,
        }))
    }

    /// The environment a build must carry for its `frust` linker process to
    /// read this action back with [`from_lookup`](Self::from_lookup). Paths
    /// pass through [`host_path::simplify`] so a Windows verbatim prefix
    /// never reaches the child.
    pub fn env_vars(&self) -> Vec<(String, String)> {
        let render = |path: &Path| host_path::simplify(path).to_string_lossy().into_owned();
        let mut vars = vec![
            (
                ENV_LINK.to_string(),
                match self.mode {
                    LinkMode::NoLink => MODE_NO_LINK,
                    LinkMode::Proxy { .. } => MODE_PROXY,
                }
                .to_string(),
            ),
            (ENV_ARGS_FILE.to_string(), render(&self.args_file)),
        ];
        if let Some(err_file) = &self.err_file {
            vars.push((ENV_ERR_FILE.to_string(), render(err_file)));
        }
        if let LinkMode::Proxy { linker } = &self.mode {
            vars.push((ENV_LINKER.to_string(), render(linker)));
        }
        vars
    }
}

/// The `-Clinker=<frust>` rustc argument that routes a build's link step
/// through `frust_exe`.
pub fn linker_arg(frust_exe: &Path) -> String {
    format!("-Clinker={}", host_path::simplify(frust_exe).display())
}

/// Whether `args` (an invocation's arguments, program name excluded) look
/// like a link step: an argument ending in `.o`, or lld's `-flavor`,
/// directly or inside an `@` response file.
pub fn has_link_indicators(args: &[String]) -> Result<bool, HotpatchError> {
    Ok(expand_response_files(args)?
        .iter()
        .any(|arg| arg.ends_with(".o") || arg == "-flavor"))
}

/// `args` with every `@<file>` response file replaced by the arguments it
/// holds, in the host's response-file syntax. As with gcc and ld, an `@`
/// argument naming no existing file is kept literally (macOS's
/// `@loader_path`-style arguments are not response files); a file that
/// exists but cannot be read or decoded is
/// [`HotpatchError::BuilderUnsupported`].
pub fn expand_response_files(args: &[String]) -> Result<Vec<String>, HotpatchError> {
    let mut expanded = Vec::with_capacity(args.len());
    for arg in args {
        let Some(path) = arg.strip_prefix('@') else {
            expanded.push(arg.clone());
            continue;
        };
        match std::fs::read(path) {
            Ok(bytes) => expanded.extend(parse_response_file(&bytes, cfg!(windows))?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => expanded.push(arg.clone()),
            Err(err) => {
                return Err(HotpatchError::unsupported(format!(
                    "unreadable linker response file `{path}`: {err}"
                )));
            }
        }
    }
    Ok(expanded)
}

/// Decodes a rustc-written linker response file: one argument per line,
/// UTF-8 or (with or without a BOM) UTF-16LE. `msvc_quoting` selects the
/// MSVC form rustc writes (each argument in double quotes, `"` escaped as
/// `\"`); otherwise the GNU form (`\` escapes the next character).
pub fn parse_response_file(bytes: &[u8], msvc_quoting: bool) -> Result<Vec<String>, HotpatchError> {
    let content = decode_response_file(bytes)?;
    let mut args = Vec::new();
    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        args.push(if msvc_quoting {
            unquote_msvc(line)
        } else {
            unescape_gnu(line)?
        });
    }
    Ok(args)
}

fn decode_response_file(bytes: &[u8]) -> Result<String, HotpatchError> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16le(rest);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text.to_string()),
        Err(_) => decode_utf16le(bytes),
    }
}

fn decode_utf16le(bytes: &[u8]) -> Result<String, HotpatchError> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return Err(HotpatchError::unsupported(
            "linker response file is neither UTF-8 nor UTF-16LE",
        ));
    }
    let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    String::from_utf16(&units).map_err(|_| {
        HotpatchError::unsupported("linker response file is neither UTF-8 nor UTF-16LE")
    })
}

fn unescape_gnu(line: &str) -> Result<String, HotpatchError> {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let escaped = chars.next().ok_or_else(|| {
                HotpatchError::unsupported(format!(
                    "linker response file line ends in a lone `\\`: `{line}`"
                ))
            })?;
            out.push(escaped);
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

fn unquote_msvc(line: &str) -> String {
    let line = line.trim();
    match line
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        Some(inner) => inner.replace("\\\"", "\""),
        None => line.to_string(),
    }
}

/// The output path of a link invocation (`args` already expanded): the
/// argument after `-o`, or an MSVC `/OUT:<path>`. None, more than one, or
/// both forms at once is [`HotpatchError::BuilderUnsupported`].
pub fn output_path(args: &[String]) -> Result<PathBuf, HotpatchError> {
    let mut found = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-o" {
            let path = iter
                .next()
                .ok_or_else(|| HotpatchError::unsupported("linker `-o` has no output path"))?;
            found.push(path.clone());
        } else if let Some(path) = arg.strip_prefix("/OUT:") {
            found.push(path.to_string());
        }
    }
    match found.as_slice() {
        [single] if !single.is_empty() => Ok(PathBuf::from(single)),
        [] => Err(HotpatchError::unsupported(
            "link invocation names no output (`-o` or `/OUT:`)",
        )),
        _ => Err(HotpatchError::unsupported(format!(
            "link invocation names an ambiguous output: {found:?}"
        ))),
    }
}

/// An empty relocatable object in `format` for `arch` (little-endian), the
/// stand-in a no-link step leaves where rustc expects its linked output.
pub fn empty_object(
    format: object::BinaryFormat,
    arch: object::Architecture,
) -> Result<Vec<u8>, HotpatchError> {
    object::write::Object::new(format, arch, object::Endianness::Little)
        .write()
        .map_err(|err| HotpatchError::unsupported(format!("cannot emit an empty object: {err}")))
}

/// [`empty_object`] in the host's own object format and architecture.
pub fn host_empty_object() -> Result<Vec<u8>, HotpatchError> {
    let format = if cfg!(target_vendor = "apple") {
        object::BinaryFormat::MachO
    } else if cfg!(windows) {
        object::BinaryFormat::Coff
    } else {
        object::BinaryFormat::Elf
    };
    let arch = if cfg!(target_arch = "aarch64") {
        object::Architecture::Aarch64
    } else if cfg!(target_arch = "x86_64") {
        object::Architecture::X86_64
    } else if cfg!(target_arch = "x86") {
        object::Architecture::I386
    } else if cfg!(target_arch = "arm") {
        object::Architecture::Arm
    } else if cfg!(target_arch = "riscv64") {
        object::Architecture::Riscv64
    } else if cfg!(target_arch = "loongarch64") {
        object::Architecture::LoongArch64
    } else {
        return Err(HotpatchError::unsupported(format!(
            "no empty-object format for host architecture `{}`",
            std::env::consts::ARCH
        )));
    };
    empty_object(format, arch)
}

/// Writes captured linker arguments to `path` as a JSON string array,
/// creating its parent directory.
pub fn write_link_args(path: &Path, args: &[String]) -> Result<(), HotpatchError> {
    let json = serde_json::to_vec(args).map_err(|err| {
        HotpatchError::unsupported(format!("cannot serialize linker arguments: {err}"))
    })?;
    write_creating_parent(path, &json, "writing captured linker arguments")
}

/// Reads arguments written by [`write_link_args`]. Anything but a JSON
/// string array is [`HotpatchError::BuilderUnsupported`].
pub fn read_link_args(path: &Path) -> Result<Vec<String>, HotpatchError> {
    let bytes = std::fs::read(path).map_err(|err| {
        HotpatchError::io(
            format!("reading captured linker arguments `{}`", path.display()),
            err,
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|err| {
        HotpatchError::unsupported(format!(
            "malformed captured linker arguments `{}`: {err}",
            path.display()
        ))
    })
}

/// Runs one intercepted link step. `args` are the linker arguments exactly
/// as rustc passed them (program name excluded, response files unexpanded).
/// The expanded arguments are always captured to the action's args file
/// first. Returns whether the step succeeded: a proxied linker's own
/// failure is `Ok(false)`, its output passed through to `out`/`err` for
/// rustc to report.
pub fn run_link(
    runner: &dyn ProcessRunner,
    action: &LinkAction,
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<bool, HotpatchError> {
    let expanded = expand_response_files(args)?;
    write_link_args(&action.args_file, &expanded)?;
    match &action.mode {
        LinkMode::NoLink => {
            let output = output_path(&expanded)?;
            let bytes = host_empty_object()?;
            write_creating_parent(&output, &bytes, "writing the no-link stand-in object")?;
            Ok(true)
        }
        LinkMode::Proxy { linker } => {
            let program = linker.to_string_lossy();
            let argv: Vec<&str> = args.iter().map(String::as_str).collect();
            let result = runner
                .run(&program, &argv)
                .map_err(|e| HotpatchError::Process {
                    detail: format!("failed to spawn linker `{program}`: {e:#}"),
                })?;
            out.write_all(result.stdout.as_bytes())
                .and_then(|()| err.write_all(result.stderr.as_bytes()))
                .map_err(|e| HotpatchError::io("forwarding linker output", e))?;
            let report = if result.success {
                "Linker warnings"
            } else {
                "Linker error"
            };
            let has_report =
                !result.success || !result.stdout.is_empty() || !result.stderr.is_empty();
            if let Some(err_file) = action.err_file.as_ref().filter(|_| has_report) {
                let text = format!("{report}: {}\n{}", result.stdout, result.stderr);
                write_creating_parent(err_file, text.as_bytes(), "writing the linker report")?;
            }
            Ok(result.success)
        }
    }
}

fn write_creating_parent(path: &Path, bytes: &[u8], what: &str) -> Result<(), HotpatchError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|err| {
            HotpatchError::io(format!("{what}: creating `{}`", parent.display()), err)
        })?;
    }
    std::fs::write(path, bytes)
        .map_err(|err| HotpatchError::io(format!("{what}: `{}`", path.display()), err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProcessRunner, Output};
    use std::collections::HashMap;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-link-intercept-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn unsupported(result: Result<impl std::fmt::Debug, HotpatchError>) -> String {
        match result {
            Err(HotpatchError::BuilderUnsupported { detail }) => detail,
            other => panic!("expected BuilderUnsupported, got {other:?}"),
        }
    }

    #[test]
    fn link_indicators_are_found_in_direct_object_and_flavor_args() {
        assert!(has_link_indicators(&strings(&["-arch", "arm64", "/t/app.app.0.rcgu.o"])).unwrap());
        assert!(has_link_indicators(&strings(&["-flavor", "gnu", "-o", "out"])).unwrap());
        assert!(!has_link_indicators(&strings(&["-vV"])).unwrap());
        assert!(!has_link_indicators(&strings(&["src/main.rs", "--edition=2024"])).unwrap());
    }

    #[test]
    fn link_indicators_are_found_inside_a_response_file() {
        let dir = temp_dir("rsp");
        let rsp = dir.join("linker-arguments");
        fs::write(
            &rsp,
            "-arch\narm64\n/t/with\\ space/app.0.rcgu.o\n-o\n/t/app\n",
        )
        .unwrap();
        let args = vec![format!("@{}", rsp.display())];
        assert!(has_link_indicators(&args).unwrap());
        let expanded = expand_response_files(&args).unwrap();
        assert_eq!(
            expanded,
            strings(&[
                "-arch",
                "arm64",
                "/t/with space/app.0.rcgu.o",
                "-o",
                "/t/app"
            ])
        );

        let quiet = dir.join("no-objects");
        fs::write(&quiet, "--version\n").unwrap();
        assert!(!has_link_indicators(&[format!("@{}", quiet.display())]).unwrap());
    }

    #[test]
    fn an_at_argument_naming_no_file_is_kept_literally() {
        let args = strings(&["-rpath", "@loader_path/../Frameworks"]);
        assert_eq!(expand_response_files(&args).unwrap(), args);
    }

    #[test]
    fn response_files_decode_utf16le_and_msvc_quoting() {
        let text = "\"/OUT:C:\\t\\app.exe\"\n\"C:\\t\\a \\\"b\\\".o\"\n";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(
            parse_response_file(&bytes, true).unwrap(),
            strings(&["/OUT:C:\\t\\app.exe", "C:\\t\\a \"b\".o"])
        );
    }

    #[test]
    fn malformed_response_files_are_builder_unsupported() {
        assert!(unsupported(parse_response_file(b"-o\nout\\", false)).contains("lone"));
        assert!(unsupported(parse_response_file(&[0xFF, 0xFE, 0x41], false)).contains("UTF-16LE"));
    }

    #[test]
    fn output_path_reads_dash_o_or_msvc_out_and_refuses_ambiguity() {
        assert_eq!(
            output_path(&strings(&["a.o", "-o", "/t/app"])).unwrap(),
            PathBuf::from("/t/app")
        );
        assert_eq!(
            output_path(&strings(&["/OUT:C:/t/app.exe"])).unwrap(),
            PathBuf::from("C:/t/app.exe")
        );
        unsupported(output_path(&strings(&["a.o"])));
        unsupported(output_path(&strings(&["a.o", "-o"])));
        unsupported(output_path(&strings(&["-o", "x", "-o", "y"])));
        unsupported(output_path(&strings(&["-o", "x", "/OUT:y"])));
    }

    #[test]
    fn link_action_reads_back_what_env_vars_writes() {
        let no_link = LinkAction {
            mode: LinkMode::NoLink,
            args_file: PathBuf::from("/t/link-args.json"),
            err_file: Some(PathBuf::from("/t/link-err.txt")),
        };
        let proxy = LinkAction {
            mode: LinkMode::Proxy {
                linker: PathBuf::from("/ndk/bin/aarch64-linux-android24-clang"),
            },
            args_file: PathBuf::from("/t/link-args.json"),
            err_file: None,
        };
        for action in [no_link, proxy] {
            let vars = action.env_vars();
            let pairs: Vec<(&str, &str)> =
                vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            assert_eq!(
                LinkAction::from_lookup(lookup(&pairs)).unwrap(),
                Some(action)
            );
        }
        assert_eq!(LinkAction::from_lookup(lookup(&[])).unwrap(), None);
    }

    #[test]
    fn contradictory_link_env_is_builder_unsupported() {
        unsupported(LinkAction::from_lookup(lookup(&[(ENV_LINK, "no-link")])));
        unsupported(LinkAction::from_lookup(lookup(&[
            (ENV_LINK, "relink"),
            (ENV_ARGS_FILE, "/a"),
        ])));
        unsupported(LinkAction::from_lookup(lookup(&[
            (ENV_LINK, "proxy"),
            (ENV_ARGS_FILE, "/a"),
        ])));
        unsupported(LinkAction::from_lookup(lookup(&[
            (ENV_LINK, "no-link"),
            (ENV_ARGS_FILE, "/a"),
            (ENV_LINKER, "/usr/bin/cc"),
        ])));
    }

    #[test]
    fn linker_arg_names_the_frust_executable() {
        assert_eq!(
            linker_arg(Path::new("/opt/frust/bin/frust")),
            "-Clinker=/opt/frust/bin/frust"
        );
    }

    #[test]
    fn no_link_writes_the_arg_file_and_a_loadable_empty_object() {
        let dir = temp_dir("nolink");
        let rsp = dir.join("linker-arguments");
        let exe = dir.join("deps").join("app-0123");
        fs::write(&rsp, format!("-o\n{}\n", exe.display())).unwrap();
        let action = LinkAction {
            mode: LinkMode::NoLink,
            args_file: dir.join("scope").join("link-args.json"),
            err_file: None,
        };
        let args = vec![
            "/t/app.app.0.rcgu.o".to_string(),
            format!("@{}", rsp.display()),
        ];
        let (mut out, mut err) = (Vec::new(), Vec::new());
        // No process is spawned in no-link mode: an empty fake would error.
        let ok = run_link(
            &FakeProcessRunner::new(),
            &action,
            &args,
            &mut out,
            &mut err,
        )
        .unwrap();
        assert!(ok);
        assert_eq!(
            read_link_args(&action.args_file).unwrap(),
            vec![
                "/t/app.app.0.rcgu.o".to_string(),
                "-o".to_string(),
                exe.display().to_string()
            ]
        );
        let bytes = fs::read(&exe).unwrap();
        let parsed = object::File::parse(&*bytes).expect("the stand-in parses as an object");
        use object::Object;
        assert_eq!(parsed.kind(), object::ObjectKind::Relocatable);
        assert!(out.is_empty() && err.is_empty());
    }

    #[test]
    fn empty_objects_parse_for_each_desktop_format() {
        use object::Object;
        for (format, arch) in [
            (object::BinaryFormat::MachO, object::Architecture::Aarch64),
            (object::BinaryFormat::Elf, object::Architecture::X86_64),
            (object::BinaryFormat::Coff, object::Architecture::X86_64),
        ] {
            let bytes = empty_object(format, arch).unwrap();
            let parsed = object::File::parse(&*bytes).unwrap();
            assert_eq!(parsed.format(), format);
            assert_eq!(parsed.architecture(), arch);
        }
    }

    #[test]
    fn proxy_forwards_the_untouched_invocation_and_captures_the_args() {
        let dir = temp_dir("proxy");
        let action = LinkAction {
            mode: LinkMode::Proxy {
                linker: PathBuf::from("/ndk/clang"),
            },
            args_file: dir.join("link-args.json"),
            err_file: Some(dir.join("link-err.txt")),
        };
        let args = strings(&["/t/lib.0.rcgu.o", "-shared", "-o", "/t/libapp.so"]);
        let runner = FakeProcessRunner::new().with(
            "/ndk/clang /t/lib.0.rcgu.o -shared -o /t/libapp.so",
            Output {
                success: true,
                stdout: String::new(),
                stderr: "warning: deprecated flag\n".to_string(),
            },
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert!(run_link(&runner, &action, &args, &mut out, &mut err).unwrap());
        assert_eq!(read_link_args(&action.args_file).unwrap(), args);
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "warning: deprecated flag\n"
        );
        let report = fs::read_to_string(dir.join("link-err.txt")).unwrap();
        assert!(report.starts_with("Linker warnings"), "{report}");
        assert!(
            !dir.join("libapp.so").exists(),
            "proxy mode writes no stand-in"
        );
    }

    #[test]
    fn a_failing_proxied_linker_is_a_failed_step_not_a_builder_error() {
        let dir = temp_dir("proxy-fail");
        let action = LinkAction {
            mode: LinkMode::Proxy {
                linker: PathBuf::from("/ndk/clang"),
            },
            args_file: dir.join("link-args.json"),
            err_file: Some(dir.join("link-err.txt")),
        };
        let args = strings(&["a.o", "-o", "out"]);
        let runner = FakeProcessRunner::new().with(
            "/ndk/clang a.o -o out",
            Output {
                success: false,
                stdout: String::new(),
                stderr: "undefined symbol: foo\n".to_string(),
            },
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert!(!run_link(&runner, &action, &args, &mut out, &mut err).unwrap());
        let report = fs::read_to_string(dir.join("link-err.txt")).unwrap();
        assert!(report.starts_with("Linker error") && report.contains("undefined symbol: foo"));

        let missing = FakeProcessRunner::new().missing("/ndk/clang");
        assert!(matches!(
            run_link(&missing, &action, &args, &mut out, &mut err),
            Err(HotpatchError::Process { .. })
        ));
    }

    #[test]
    fn malformed_captured_link_args_are_builder_unsupported() {
        let dir = temp_dir("bad-args");
        let path = dir.join("link-args.json");
        fs::write(&path, "-o\nout\n").unwrap();
        unsupported(read_link_args(&path));
    }
}
