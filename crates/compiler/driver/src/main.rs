//! Unified `pop` command and build orchestration.

#![allow(
    clippy::map_unwrap_or,
    clippy::option_option,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use pop_backend_api::RuntimeProfile;
use pop_backend_c::{CLoweringOptions, lower_mir_to_c};
use pop_backend_llvm::{
    BpfLoweringOptions, BpfProgramKind, LlvmLoweringOptions, lower_mir_to_bpf_module,
    lower_mir_to_llvm_ir,
};
use pop_documentation_generator::{DocumentationMember, render_xml};
use pop_driver::{
    CheckedDocumentation, FrontEndBubbleInput, FrontEndModule, PoplibDependency, PoplibEmission,
    ReferenceFunction, ReferenceMetadata, ReferenceType, analyze_bubble, emit_poplib,
    encode_reference_metadata, load_poplib,
};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId, SymbolId};
use pop_mir::{lower_hir_bubble, optimize_mir};
use pop_projects::{
    BubbleKind, BubbleLock, DependencySource, LockMode, LockedBubble, LockedBubbleIdentity,
    LockedPackage, LockedSource, WorkspaceManifest, apply_lock_policy,
    discover_conventional_bubbles, discover_workspace_members, encode_lock, parse_package_manifest,
    parse_workspace_manifest, sha256_hex,
};
use pop_resolve::Visibility;
use pop_source::SourceFile;
use pop_target::TargetSpec;
use pop_types::SemanticType;

const INTERNAL_BUBBLE: BubbleId = BubbleId::from_raw(1);
const STANDARD_BUBBLE: BubbleId = BubbleId::from_raw(2);
const FIRST_PACKAGE_BUBBLE: u32 = 3;

const USAGE: &str = "\
Usage:
    pop check <source.pop> [--dump <hir|mir|ll>]...
    pop check --manifestPath <bubble.toml>
    pop build <source.pop> --output <executable>
    pop build <source.pop> --target bpfel-unknown-none --runtime-profile linux-ebpf --bpf-program xdp --emit-object <object.o>
    pop build --manifestPath <bubble.toml>
    pop documentation --manifestPath <bubble.toml>
    pop transpile <source.pop> --to c
    pop run <source.pop> [-- <arguments>...]
    pop run --manifestPath <bubble.toml> [-- <arguments>...]

The direct source path is a bootstrap compiler inspection mode. It checks one
Module in an ephemeral Bubble and does not define Package or Bubble identity.
IR dumps are deterministic debug text for this compiler version, not stable
serialization formats.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DumpKind {
    Hir,
    Mir,
    Ll,
}

#[derive(Debug, Eq, PartialEq)]
enum CommandLine {
    Help,
    Check {
        source_path: PathBuf,
        dumps: Vec<DumpKind>,
    },
    PackageCheck {
        manifest_path: PathBuf,
        lock_mode: LockMode,
    },
    Build {
        source_path: PathBuf,
        output_path: PathBuf,
    },
    BuildBpf {
        source_path: PathBuf,
        target: String,
        runtime_profile: RuntimeProfile,
        program: BpfProgramKind,
        output_path: PathBuf,
    },
    PackageBuild {
        manifest_path: PathBuf,
        lock_mode: LockMode,
    },
    Documentation {
        manifest_path: PathBuf,
        lock_mode: LockMode,
    },
    TranspileToC {
        source_path: PathBuf,
    },
    Run {
        source_path: PathBuf,
        arguments: Vec<OsString>,
    },
    PackageRun {
        manifest_path: PathBuf,
        lock_mode: LockMode,
        arguments: Vec<OsString>,
    },
}

fn main() -> ExitCode {
    match parse_arguments(std::env::args_os().skip(1)) {
        Ok(CommandLine::Help) => write_help(),
        Ok(CommandLine::Check { source_path, dumps }) => check_source(&source_path, &dumps),
        Ok(CommandLine::PackageCheck {
            manifest_path,
            lock_mode,
        }) => check_manifest(&manifest_path, lock_mode),
        Ok(CommandLine::Build {
            source_path,
            output_path,
        }) => build_source(&source_path, &output_path),
        Ok(CommandLine::BuildBpf {
            source_path,
            target,
            runtime_profile,
            program,
            output_path,
        }) => build_bpf_source(
            &source_path,
            &target,
            runtime_profile,
            program,
            &output_path,
        ),
        Ok(CommandLine::PackageBuild {
            manifest_path,
            lock_mode,
        }) => build_manifest(&manifest_path, lock_mode)
            .map_or(ExitCode::FAILURE, |_| ExitCode::SUCCESS),
        Ok(CommandLine::Documentation {
            manifest_path,
            lock_mode,
        }) => document_manifest(&manifest_path, lock_mode),
        Ok(CommandLine::TranspileToC { source_path }) => transpile_source_to_c(&source_path),
        Ok(CommandLine::Run {
            source_path,
            arguments,
        }) => run_source(&source_path, &arguments),
        Ok(CommandLine::PackageRun {
            manifest_path,
            lock_mode,
            arguments,
        }) => run_manifest(&manifest_path, lock_mode, &arguments),
        Err(error) => {
            eprintln!("pop: {error}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn parse_arguments(arguments: impl IntoIterator<Item = OsString>) -> Result<CommandLine, String> {
    let mut arguments = arguments.into_iter();
    let Some(command) = arguments.next() else {
        return Err("missing command".to_owned());
    };
    if command == "--help" || command == "-h" {
        return Ok(CommandLine::Help);
    }
    if command == "build" {
        return parse_build_arguments(arguments);
    }
    if command == "transpile" {
        return parse_transpile_arguments(arguments);
    }
    if command == "documentation" {
        return parse_documentation_arguments(arguments);
    }
    if command == "run" {
        return parse_run_arguments(arguments);
    }
    if command != "check" {
        return Err(format!(
            "unsupported command `{}`",
            command.to_string_lossy()
        ));
    }

    parse_check_arguments(arguments)
}

fn parse_documentation_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, String> {
    let Some(option) = arguments.next() else {
        return Err("`pop documentation` requires `--manifestPath <bubble.toml>`".to_owned());
    };
    if option != "--manifestPath" {
        return Err(format!(
            "unsupported option `{}`; expected --manifestPath",
            option.to_string_lossy()
        ));
    }
    let manifest_path = required_manifest_path(arguments.next(), "documentation")?;
    let lock_mode = parse_lock_controls(arguments)?;
    Ok(CommandLine::Documentation {
        manifest_path,
        lock_mode,
    })
}

fn parse_check_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, String> {
    let Some(first) = arguments.next() else {
        return Err("`pop check` requires a .pop source path or `--manifestPath`".to_owned());
    };
    if first == "--manifestPath" {
        let manifest_path = required_manifest_path(arguments.next(), "check")?;
        let lock_mode = parse_lock_controls(arguments)?;
        return Ok(CommandLine::PackageCheck {
            manifest_path,
            lock_mode,
        });
    }

    let mut source_path = Some(required_source_path(Some(first), "check")?);
    let mut dumps = Vec::new();
    while let Some(argument) = arguments.next() {
        if argument == "--help" || argument == "-h" {
            return Ok(CommandLine::Help);
        }
        if argument == "--dump" {
            let Some(kind) = arguments.next() else {
                return Err("`--dump` requires hir|mir|ll".to_owned());
            };
            let kind = parse_dump_kind(&kind)?;
            if !dumps.contains(&kind) {
                dumps.push(kind);
            }
            continue;
        }
        if argument.to_string_lossy().starts_with('-') {
            return Err(format!(
                "unsupported option `{}`",
                argument.to_string_lossy()
            ));
        }
        if source_path.replace(PathBuf::from(argument)).is_some() {
            return Err("`pop check` accepts exactly one source path in bootstrap mode".to_owned());
        }
    }

    let source_path =
        source_path.ok_or_else(|| "`pop check` requires a .pop source path".to_owned())?;
    if source_path.extension() != Some(OsStr::new("pop")) {
        return Err("`pop check` requires a source path with the .pop extension".to_owned());
    }
    Ok(CommandLine::Check { source_path, dumps })
}

fn parse_transpile_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, String> {
    let source_path = required_source_path(arguments.next(), "transpile")?;
    let Some(option) = arguments.next() else {
        return Err("`pop transpile` requires `--to c`".to_owned());
    };
    if option != "--to" {
        return Err(format!("unsupported option `{}`", option.to_string_lossy()));
    }
    let Some(target) = arguments.next() else {
        return Err("`--to` requires a backend source format; expected c".to_owned());
    };
    if target != "c" {
        return Err(format!(
            "unsupported transpilation target `{}`; expected c",
            target.to_string_lossy()
        ));
    }
    if arguments.next().is_some() {
        return Err("`pop transpile` received unexpected arguments".to_owned());
    }
    Ok(CommandLine::TranspileToC { source_path })
}

fn parse_build_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, String> {
    let first = arguments.next();
    if first.as_deref() == Some(OsStr::new("--manifestPath")) {
        let manifest_path = required_manifest_path(arguments.next(), "build")?;
        let lock_mode = parse_lock_controls(arguments)?;
        return Ok(CommandLine::PackageBuild {
            manifest_path,
            lock_mode,
        });
    }
    let source_path = required_source_path(first, "build")?;
    let Some(option) = arguments.next() else {
        return Err(
            "`pop build` requires `--output <executable>` or `--target <triple>`".to_owned(),
        );
    };
    if option == "--target" {
        let target = arguments
            .next()
            .ok_or_else(|| "`--target` requires a target triple".to_owned())?
            .to_string_lossy()
            .into_owned();
        let Some(runtime_option) = arguments.next() else {
            return Err("BPF builds require `--runtime-profile linux-ebpf`".to_owned());
        };
        if runtime_option != "--runtime-profile" {
            return Err(format!(
                "unsupported option `{}`; expected --runtime-profile",
                runtime_option.to_string_lossy()
            ));
        }
        let runtime_profile = arguments
            .next()
            .ok_or_else(|| "`--runtime-profile` requires a profile name".to_owned())
            .and_then(|profile| {
                RuntimeProfile::parse(&profile.to_string_lossy()).map_err(|error| error.to_string())
            })?;
        let Some(program_option) = arguments.next() else {
            return Err("BPF builds require `--bpf-program xdp`".to_owned());
        };
        if program_option != "--bpf-program" {
            return Err(format!(
                "unsupported option `{}`; expected --bpf-program",
                program_option.to_string_lossy()
            ));
        }
        let program = match arguments.next().as_deref() {
            Some(value) if value == OsStr::new("xdp") => BpfProgramKind::Xdp,
            Some(value) => {
                return Err(format!(
                    "unsupported BPF program `{}`; expected xdp",
                    value.to_string_lossy()
                ));
            }
            None => return Err("`--bpf-program` requires xdp".to_owned()),
        };
        let Some(output_option) = arguments.next() else {
            return Err("BPF builds require `--emit-object <object.o>`".to_owned());
        };
        if output_option != "--emit-object" {
            return Err(format!(
                "unsupported option `{}`; expected --emit-object",
                output_option.to_string_lossy()
            ));
        }
        let output_path = arguments
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| "`--emit-object` requires an object path".to_owned())?;
        if arguments.next().is_some() {
            return Err("`pop build` received unexpected arguments".to_owned());
        }
        return Ok(CommandLine::BuildBpf {
            source_path,
            target,
            runtime_profile,
            program,
            output_path,
        });
    }
    if option != "--output" {
        return Err(format!("unsupported option `{}`", option.to_string_lossy()));
    }
    let output_path = arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "`--output` requires an executable path".to_owned())?;
    if arguments.next().is_some() {
        return Err("`pop build` received unexpected arguments".to_owned());
    }
    Ok(CommandLine::Build {
        source_path,
        output_path,
    })
}

fn required_manifest_path(argument: Option<OsString>, command: &str) -> Result<PathBuf, String> {
    let path = argument
        .map(PathBuf::from)
        .ok_or_else(|| format!("`pop {command} --manifestPath` requires a bubble.toml path"))?;
    if path.file_name() != Some(OsStr::new("bubble.toml")) {
        return Err(format!(
            "`pop {command} --manifestPath` requires a path named bubble.toml"
        ));
    }
    Ok(path)
}

fn parse_run_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, String> {
    let Some(first) = arguments.next() else {
        return Err("`pop run` requires a source path or `--manifestPath`".to_owned());
    };
    if first == "--manifestPath" {
        let manifest_path = required_manifest_path(arguments.next(), "run")?;
        let remaining = arguments.collect::<Vec<_>>();
        let separator = remaining.iter().position(|argument| argument == "--");
        let (controls, program_arguments) = separator.map_or_else(
            || (remaining.as_slice(), Vec::new()),
            |separator| (&remaining[..separator], remaining[separator + 1..].to_vec()),
        );
        let lock_mode = parse_lock_controls(controls.iter().cloned())?;
        return Ok(CommandLine::PackageRun {
            manifest_path,
            lock_mode,
            arguments: program_arguments,
        });
    }
    let source_path = required_source_path(Some(first), "run")?;
    let program_arguments = parse_program_arguments(arguments)?;
    Ok(CommandLine::Run {
        source_path,
        arguments: program_arguments,
    })
}

fn parse_lock_controls(arguments: impl IntoIterator<Item = OsString>) -> Result<LockMode, String> {
    let mut locked = false;
    let mut offline = false;
    for argument in arguments {
        match argument.to_str() {
            Some("--locked") => locked = true,
            Some("--offline") => offline = true,
            Some("--frozen") => {
                locked = true;
                offline = true;
            }
            _ => {
                return Err(format!(
                    "unsupported manifest option `{}`; expected --locked, --offline, or --frozen",
                    argument.to_string_lossy()
                ));
            }
        }
    }
    Ok(match (locked, offline) {
        (false, false) => LockMode::Normal,
        (true, false) => LockMode::Locked,
        (false, true) => LockMode::Offline,
        (true, true) => LockMode::Frozen,
    })
}

fn parse_program_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<Vec<OsString>, String> {
    let Some(separator) = arguments.next() else {
        return Ok(Vec::new());
    };
    if separator != "--" {
        return Err(format!(
            "unsupported option `{}`; program arguments must follow `--`",
            separator.to_string_lossy()
        ));
    }
    Ok(arguments.collect())
}

fn required_source_path(argument: Option<OsString>, command: &str) -> Result<PathBuf, String> {
    let path = argument
        .map(PathBuf::from)
        .ok_or_else(|| format!("`pop {command}` requires a .pop source path"))?;
    if path.extension() != Some(OsStr::new("pop")) {
        return Err(format!("`pop {command}` requires a .pop source path"));
    }
    Ok(path)
}

fn parse_dump_kind(kind: &OsStr) -> Result<DumpKind, String> {
    match kind.to_str() {
        Some("hir") => Ok(DumpKind::Hir),
        Some("mir") => Ok(DumpKind::Mir),
        Some("ll") => Ok(DumpKind::Ll),
        _ => Err(format!(
            "unsupported dump kind `{}`; expected hir|mir|ll",
            kind.to_string_lossy()
        )),
    }
}

fn write_help() -> ExitCode {
    if let Err(error) = writeln!(io::stdout().lock(), "{USAGE}") {
        eprintln!("pop: could not write help: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn check_source(source_path: &PathBuf, dumps: &[DumpKind]) -> ExitCode {
    let source_text = match fs::read_to_string(source_path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("pop: could not read `{}`: {error}", source_path.display());
            return ExitCode::FAILURE;
        }
    };
    let source = match SourceFile::new(
        FileId::from_raw(0),
        source_path.to_string_lossy().into_owned(),
        source_text,
    ) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("pop: could not load `{}`: {error}", source_path.display());
            return ExitCode::FAILURE;
        }
    };
    let Some((standard, _)) = lower_toolchain_standard() else {
        return ExitCode::FAILURE;
    };
    let result = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(FIRST_PACKAGE_BUBBLE),
            NamespaceId::from_raw(FIRST_PACKAGE_BUBBLE),
            vec![STANDARD_BUBBLE],
            vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
        )
        .with_reference_metadata(vec![standard.metadata]),
    );
    if !result.diagnostics().is_empty() {
        return write_diagnostics(&result.diagnostic_snapshot());
    }
    let Some(hir) = result.hir() else {
        eprintln!("pop: internal compiler error: successful analysis did not publish HIR");
        return ExitCode::from(101);
    };
    let mir = match lower_hir_bubble(hir, result.types()) {
        Ok(mir) => mir,
        Err(errors) => {
            eprintln!("pop: internal compiler error: canonical MIR verification failed");
            for error in errors {
                eprintln!("  {error:?}");
            }
            return ExitCode::from(101);
        }
    };
    let llvm = if dumps.contains(&DumpKind::Ll) {
        let module = match lower_mir_to_llvm_ir(
            &mir,
            result.types(),
            &native_target(),
            LlvmLoweringOptions::default(),
        ) {
            Ok(module) => module,
            Err(error) => {
                eprintln!("pop: internal compiler error: LLVM lowering failed: {error}");
                return ExitCode::from(101);
            }
        };
        if let Err(error) = module.verify() {
            eprintln!("pop: internal compiler error: {error}");
            return ExitCode::from(101);
        }
        Some(module)
    } else {
        None
    };

    let mut output = String::new();
    for dump in dumps {
        match dump {
            DumpKind::Hir => output.push_str(&hir.dump(result.types())),
            DumpKind::Mir => output.push_str(&mir.dump()),
            DumpKind::Ll => output.push_str(
                &llvm
                    .as_ref()
                    .expect("requested LLVM dump was lowered and verified")
                    .to_string(),
            ),
        }
    }
    write_output(&output)
}

fn native_target() -> TargetSpec {
    TargetSpec::for_triple("x86_64-unknown-linux-gnu")
        .expect("repository native target is complete")
}

fn write_diagnostics(diagnostics: &str) -> ExitCode {
    if let Err(error) = io::stderr().lock().write_all(diagnostics.as_bytes()) {
        eprintln!("pop: could not write diagnostics: {error}");
    }
    ExitCode::FAILURE
}

fn write_output(output: &str) -> ExitCode {
    if let Err(error) = io::stdout().lock().write_all(output.as_bytes()) {
        eprintln!("pop: could not write output: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn build_source(source_path: &Path, output_path: &Path) -> ExitCode {
    let Some(program) = lower_native_source(source_path) else {
        return ExitCode::FAILURE;
    };
    let Some((_, standard)) = lower_toolchain_standard() else {
        return ExitCode::FAILURE;
    };
    let target = native_target();
    let module = match lower_mir_to_llvm_ir(
        &program.mir,
        &program.types,
        &target,
        LlvmLoweringOptions::default().with_entry_point(
            program
                .entry
                .expect("standalone executable has a verified entry"),
        ),
    ) {
        Ok(module) => module,
        Err(error) => {
            eprintln!("pop: LLVM lowering failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let object_path = std::env::temp_dir().join(format!("pop-native-{}.o", std::process::id()));
    let standard_object_path =
        std::env::temp_dir().join(format!("pop-standard-{}.o", std::process::id()));
    if let Err(error) = module.emit_object(&object_path) {
        eprintln!("pop: {error}");
        return ExitCode::FAILURE;
    }
    if emit_native_object(&standard.program, &standard_object_path).is_none() {
        let _ = fs::remove_file(&object_path);
        return ExitCode::FAILURE;
    }
    let result = link_native_executable(
        &[object_path.clone(), standard_object_path.clone()],
        output_path,
    );
    let _ = fs::remove_file(object_path);
    let _ = fs::remove_file(standard_object_path);
    result
}

fn build_bpf_source(
    source_path: &Path,
    target_triple: &str,
    runtime_profile: RuntimeProfile,
    program: BpfProgramKind,
    output_path: &Path,
) -> ExitCode {
    let target = match TargetSpec::for_triple(target_triple) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("pop: {error}: `{target_triple}`");
            return ExitCode::FAILURE;
        }
    };
    let Some(program_mir) = lower_native_source(source_path) else {
        return ExitCode::FAILURE;
    };
    let Some(entry) = program_mir.entry else {
        eprintln!("pop: BPF build requires an explicit entry point");
        return ExitCode::FAILURE;
    };
    let options = match program {
        BpfProgramKind::Xdp => BpfLoweringOptions::xdp(entry).with_runtime_profile(runtime_profile),
    };
    let module =
        match lower_mir_to_bpf_module(&program_mir.mir, &program_mir.types, &target, options) {
            Ok(module) => module,
            Err(error) => {
                eprintln!("pop: {}: {error}", error.diagnostic_code());
                return ExitCode::FAILURE;
            }
        };
    if let Err(error) = module.emit_object(output_path) {
        eprintln!("pop: {}: {error}", error.diagnostic_code());
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn transpile_source_to_c(source_path: &Path) -> ExitCode {
    let Some(program) = lower_native_source(source_path) else {
        return ExitCode::FAILURE;
    };
    let NativeProgram {
        mir, types, entry, ..
    } = program;
    let options = CLoweringOptions::default()
        .with_entry_point(entry.expect("standalone transpilation has a verified entry"));
    let translation = match lower_mir_to_c(&mir, &types, options) {
        Ok(translation) => translation,
        Err(error) => {
            eprintln!("pop: C lowering failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    write_output(translation.as_str())
}

fn run_source(source_path: &Path, arguments: &[OsString]) -> ExitCode {
    let executable = std::env::temp_dir().join(format!("pop-run-{}", std::process::id()));
    let build = build_source(source_path, &executable);
    if build != ExitCode::SUCCESS {
        return build;
    }
    let status = Command::new(&executable).args(arguments).status();
    let _ = fs::remove_file(&executable);
    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(
            status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or(1),
        ),
        Err(error) => {
            eprintln!("pop: could not execute native program: {error}");
            ExitCode::FAILURE
        }
    }
}

struct LoweredPackage {
    root: PathBuf,
    bubbles: Vec<LoweredPackageBubble>,
}

struct LoweredPackageBubble {
    bubble: BubbleId,
    package: String,
    version: String,
    source_sha256: String,
    edition: String,
    name: String,
    kind: BubbleKind,
    root_package: bool,
    dependencies: Vec<PoplibDependency>,
    program: NativeProgram,
}

fn lower_package(manifest_path: &Path) -> Option<LoweredPackage> {
    let manifest_path = fs::canonicalize(manifest_path)
        .map_err(|error| {
            eprintln!(
                "pop: could not resolve `{}`: {error}",
                manifest_path.display()
            );
        })
        .ok()?;
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let (standard, standard_bubble) = lower_toolchain_standard()?;
    let mut state = PackageLoweringState {
        next_bubble: FIRST_PACKAGE_BUBBLE,
        visiting: BTreeSet::new(),
        resolved: BTreeMap::new(),
        bubbles: vec![standard_bubble],
        standard,
    };
    lower_package_recursive(&manifest_path, true, &mut state)?;
    Some(LoweredPackage {
        root: package_root.to_path_buf(),
        bubbles: state.bubbles,
    })
}

#[derive(Clone)]
struct ResolvedPackageLibrary {
    package: String,
    version: String,
    source_sha256: String,
    bubble: String,
    public_api_sha256: String,
    metadata: ReferenceMetadata,
}

impl ResolvedPackageLibrary {
    fn artifact_dependency(&self) -> PoplibDependency {
        PoplibDependency::new(
            &self.package,
            &self.version,
            &self.source_sha256,
            &self.bubble,
            BubbleKind::Library,
            &self.public_api_sha256,
        )
    }
}

struct PackageLoweringState {
    next_bubble: u32,
    visiting: BTreeSet<PathBuf>,
    resolved: BTreeMap<PathBuf, Option<ResolvedPackageLibrary>>,
    bubbles: Vec<LoweredPackageBubble>,
    standard: ResolvedPackageLibrary,
}

fn lower_package_recursive(
    manifest_path: &Path,
    root_package: bool,
    state: &mut PackageLoweringState,
) -> Option<Option<ResolvedPackageLibrary>> {
    if let Some(resolved) = state.resolved.get(manifest_path) {
        return Some(resolved.clone());
    }
    if !state.visiting.insert(manifest_path.to_path_buf()) {
        eprintln!(
            "pop: Package dependency cycle includes `{}`",
            manifest_path.display()
        );
        return None;
    }
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let manifest_text = fs::read_to_string(manifest_path)
        .map_err(|error| {
            eprintln!("pop: could not read `{}`: {error}", manifest_path.display());
        })
        .ok()?;
    let manifest = parse_package_manifest(&manifest_text)
        .map_err(|error| eprintln!("pop: {error}"))
        .ok()?;

    let mut external_libraries = vec![state.standard.clone()];
    for requirement in manifest.dependencies() {
        let dependency_manifest = match requirement.source() {
            DependencySource::LocalPath(path) => package_root.join(path).join("bubble.toml"),
            DependencySource::Registry => {
                eprintln!(
                    "pop: registry dependency `{}` requires a resolved bubble.lock",
                    requirement.alias()
                );
                return None;
            }
            DependencySource::ExactGit { .. } => {
                eprintln!(
                    "pop: exact-Git dependency `{}` requires a resolved bubble.lock",
                    requirement.alias()
                );
                return None;
            }
            DependencySource::Workspace => {
                eprintln!(
                    "pop: workspace-inherited dependency `{}` requires a Workspace root",
                    requirement.alias()
                );
                return None;
            }
        };
        let dependency_manifest = fs::canonicalize(&dependency_manifest)
            .map_err(|error| {
                eprintln!(
                    "pop: could not resolve dependency `{}` at `{}`: {error}",
                    requirement.alias(),
                    dependency_manifest.display()
                );
            })
            .ok()?;
        let Some(library) = lower_package_recursive(&dependency_manifest, false, state)? else {
            eprintln!(
                "pop: dependency `{}` has no public library Bubble",
                requirement.alias()
            );
            return None;
        };
        if requirement
            .version_requirement()
            .is_some_and(|required| required != library.version)
        {
            eprintln!(
                "pop: dependency `{}` requires version {}, but {} was resolved",
                requirement.alias(),
                requirement.version_requirement().unwrap_or(""),
                library.version
            );
            return None;
        }
        if requirement
            .bubble()
            .is_some_and(|selected| selected != library.bubble)
        {
            eprintln!(
                "pop: dependency `{}` selects Bubble {}, but the Package publishes {}",
                requirement.alias(),
                requirement.bubble().unwrap_or(""),
                library.bubble
            );
            return None;
        }
        external_libraries.push(library);
    }

    let source_paths = collect_package_sources(package_root).ok()?;
    let source_sha256 = package_content_hash(manifest_path, &source_paths)?;
    let external_metadata = external_libraries
        .iter()
        .map(|library| library.metadata.clone())
        .collect::<Vec<_>>();
    let artifact_dependencies = external_libraries
        .iter()
        .map(ResolvedPackageLibrary::artifact_dependency)
        .collect::<Vec<_>>();
    let relative_paths: Vec<_> = source_paths.keys().map(String::as_str).collect();
    let bubbles = discover_conventional_bubbles(&manifest, &relative_paths)
        .map_err(|error| eprintln!("pop: {error}"))
        .ok()?;
    let selected: Vec<_> = bubbles
        .iter()
        .filter(|bubble| {
            bubble.kind() == BubbleKind::Library
                || root_package && bubble.kind() == BubbleKind::Binary
        })
        .collect();
    if selected.is_empty() {
        eprintln!("pop: Package has no selected library or binary Bubbles");
        return None;
    }

    let mut library = None;
    for bubble in selected {
        let bubble_id = BubbleId::from_raw(state.next_bubble);
        state.next_bubble = state.next_bubble.checked_add(1)?;
        let modules = bubble
            .modules()
            .iter()
            .map(|relative| {
                let source = source_paths.get(relative).cloned().ok_or_else(|| {
                    eprintln!("pop: discovered Module `{relative}` is missing");
                })?;
                Ok((PathBuf::from(relative), source))
            })
            .collect::<Result<Vec<_>, ()>>()
            .ok()?;
        let mut dependency_metadata = external_metadata.clone();
        if bubble.depends_on_library() {
            dependency_metadata.push(
                library
                    .as_ref()
                    .map(|library: &ResolvedPackageLibrary| library.metadata.clone())
                    .expect("sorted conventional discovery lowers the library first"),
            );
        }
        let program = lower_native_bubble(
            bubble_id,
            &modules,
            bubble.kind() == BubbleKind::Binary,
            dependency_metadata,
            Vec::new(),
        )?;
        if bubble.kind() == BubbleKind::Library {
            let reference = encode_reference_metadata(&program.reference_metadata)
                .map_err(|error| eprintln!("pop: reference metadata encoding failed: {error}"))
                .ok()?;
            library = Some(ResolvedPackageLibrary {
                package: manifest.name().to_owned(),
                version: manifest.version().to_owned(),
                source_sha256: source_sha256.clone(),
                bubble: bubble.name().to_owned(),
                public_api_sha256: sha256_hex(&reference),
                metadata: program.reference_metadata.clone(),
            });
        }
        state.bubbles.push(LoweredPackageBubble {
            bubble: bubble_id,
            package: manifest.name().to_owned(),
            version: manifest.version().to_owned(),
            source_sha256: source_sha256.clone(),
            edition: manifest.edition().to_owned(),
            name: bubble.name().to_owned(),
            kind: bubble.kind(),
            root_package,
            dependencies: artifact_dependencies.clone(),
            program,
        });
    }

    state.visiting.remove(manifest_path);
    state
        .resolved
        .insert(manifest_path.to_path_buf(), library.clone());
    Some(library)
}

fn check_manifest(manifest_path: &Path, lock_mode: LockMode) -> ExitCode {
    let Some(selection) = manifest_selection(manifest_path) else {
        return ExitCode::FAILURE;
    };
    if prepare_lock(&selection, lock_mode).is_none() {
        return ExitCode::FAILURE;
    }
    if selection
        .packages
        .iter()
        .all(|manifest| lower_package(manifest).is_some())
    {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn build_manifest(manifest_path: &Path, lock_mode: LockMode) -> Option<Vec<PathBuf>> {
    let selection = manifest_selection(manifest_path)?;
    prepare_lock(&selection, lock_mode)?;
    let shared_output = selection
        .workspace_root
        .as_ref()
        .map(|root| root.join("target/debug"));
    let mut executables = Vec::new();
    for manifest in selection.packages {
        executables.extend(build_package_to(&manifest, shared_output.as_deref())?);
    }
    Some(executables)
}

fn document_manifest(manifest_path: &Path, lock_mode: LockMode) -> ExitCode {
    let Some(selection) = manifest_selection(manifest_path) else {
        return ExitCode::FAILURE;
    };
    if prepare_lock(&selection, lock_mode).is_none() {
        return ExitCode::FAILURE;
    }
    let output_root = selection.workspace_root.clone().unwrap_or_else(|| {
        selection.packages[0]
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    });
    let output_root = output_root.join("target/documentation");
    let mut emitted = 0usize;
    for manifest in selection.packages {
        let Some(package) = lower_package(&manifest) else {
            return ExitCode::FAILURE;
        };
        for bubble in package
            .bubbles
            .iter()
            .filter(|bubble| bubble.root_package && bubble.kind == BubbleKind::Library)
        {
            let members = documentation_members(&bubble.program);
            let xml = match render_xml(&bubble.name, &members) {
                Ok(xml) => xml,
                Err(error) => {
                    eprintln!("pop: documentation output failed: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let directory = output_root.join(&bubble.name);
            if let Err(error) = fs::create_dir_all(&directory) {
                eprintln!(
                    "pop: could not create documentation output `{}`: {error}",
                    directory.display()
                );
                return ExitCode::FAILURE;
            }
            let output = directory.join("documentation.xml");
            if let Err(error) = fs::write(&output, xml) {
                eprintln!(
                    "pop: could not write documentation output `{}`: {error}",
                    output.display()
                );
                return ExitCode::FAILURE;
            }
            emitted += 1;
        }
    }
    if emitted == 0 {
        eprintln!("pop: `pop documentation` requires a selected library Bubble");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn documentation_members(program: &NativeProgram) -> Vec<DocumentationMember> {
    let documentation: BTreeMap<_, _> = program
        .checked_documentation
        .iter()
        .map(|documentation| (documentation.identity(), documentation.fragment()))
        .collect();
    program
        .reference_metadata
        .functions()
        .iter()
        .filter_map(|function| {
            documentation.get(&function.identity()).map(|fragment| {
                DocumentationMember::new(documentation_member_id(function), (*fragment).clone())
            })
        })
        .collect()
}

fn documentation_member_id(function: &ReferenceFunction) -> String {
    let type_parameters = function
        .type_parameters()
        .iter()
        .map(|parameter| parameter.name())
        .collect::<Vec<_>>();
    let parameters = function
        .parameters()
        .iter()
        .map(|parameter| reference_type_text(parameter.parameter_type(), &type_parameters))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "function:{}.{}({parameters})",
        function.namespace(),
        function.name()
    )
}

fn reference_type_text(reference: &ReferenceType, type_parameters: &[&str]) -> String {
    match reference {
        ReferenceType::Primitive(primitive) => pop_types::PrimitiveType::source_schema()
            .iter()
            .copied()
            .find(|entry| entry.primitive() == *primitive && !entry.is_alias())
            .map_or_else(
                || format!("{primitive:?}"),
                |entry| entry.canonical_name().to_owned(),
            ),
        ReferenceType::TypeParameter(index) => type_parameters
            .get(usize::from(*index))
            .map_or_else(|| format!("T{index}"), |name| (*name).to_owned()),
        ReferenceType::Tuple(elements) => format!(
            "({})",
            elements
                .iter()
                .map(|element| reference_type_text(element, type_parameters))
                .collect::<Vec<_>>()
                .join(",")
        ),
        ReferenceType::Function {
            parameters,
            results,
            ..
        } => format!(
            "function({})->({})",
            parameters
                .iter()
                .map(|parameter| reference_type_text(parameter, type_parameters))
                .collect::<Vec<_>>()
                .join(","),
            results
                .iter()
                .map(|result| reference_type_text(result, type_parameters))
                .collect::<Vec<_>>()
                .join(",")
        ),
        ReferenceType::Array(element) => {
            format!("Array<{}>", reference_type_text(element, type_parameters))
        }
        ReferenceType::Table { key, value } => format!(
            "Table<{},{}>",
            reference_type_text(key, type_parameters),
            reference_type_text(value, type_parameters)
        ),
        ReferenceType::Optional(element) => {
            format!("{}?", reference_type_text(element, type_parameters))
        }
        ReferenceType::Builtin {
            definition,
            arguments,
        } => {
            let arguments = arguments
                .iter()
                .map(|argument| reference_type_text(argument, type_parameters))
                .collect::<Vec<_>>()
                .join(",");
            format!("Builtin{}<{arguments}>", definition.raw())
        }
        ReferenceType::Union(elements) => elements
            .iter()
            .map(|element| reference_type_text(element, type_parameters))
            .collect::<Vec<_>>()
            .join("|"),
    }
}

fn build_package_to(
    manifest_path: &Path,
    selected_output_root: Option<&Path>,
) -> Option<Vec<PathBuf>> {
    let package = lower_package(manifest_path)?;

    let output_root = selected_output_root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| package.root.join("target/debug"));
    let dependency_root = output_root.join("deps");
    fs::create_dir_all(&dependency_root)
        .map_err(|error| eprintln!("pop: could not create build output: {error}"))
        .ok()?;
    let mut library_objects = Vec::new();
    let mut binary_objects = Vec::new();
    for bubble in &package.bubbles {
        let suffix = if bubble.kind == BubbleKind::Library {
            "library"
        } else {
            "binary"
        };
        let object = dependency_root.join(format!(
            "{}.b{}.{}.o",
            bubble.name,
            bubble.bubble.raw(),
            suffix
        ));
        let emission_object = dependency_root.join(format!(
            "{}.b{}.{}.emission.o",
            bubble.name,
            bubble.bubble.raw(),
            suffix
        ));
        let lowering_output = if bubble.kind == BubbleKind::Library {
            &emission_object
        } else {
            &object
        };
        emit_native_object(&bubble.program, lowering_output)?;
        if bubble.kind == BubbleKind::Library {
            let documentation = render_xml(&bubble.name, &documentation_members(&bubble.program))
                .map_err(|error| eprintln!("pop: documentation output failed: {error}"))
                .ok()?;
            let implementation = fs::read(&emission_object)
                .map_err(|error| eprintln!("pop: could not read native object: {error}"))
                .ok()?;
            let target = native_target();
            let emission = PoplibEmission::new(
                &bubble.package,
                &bubble.version,
                &bubble.source_sha256,
                &bubble.name,
                bubble.kind,
                &bubble.edition,
                bubble.program.reference_metadata.clone(),
            )
            .with_dependencies(bubble.dependencies.clone())
            .with_documentation(documentation.into_bytes())
            .with_target_implementation(target.triple(), implementation);
            let artifact = dependency_root.join(format!("{}.poplib", bubble.name));
            emit_poplib(&artifact, &emission)
                .map_err(|error| eprintln!("pop: library artifact emission failed: {error}"))
                .ok()?;
            let loaded = load_poplib(&artifact)
                .map_err(|error| eprintln!("pop: emitted library verification failed: {error:?}"))
                .ok()?;
            let (selected_target, selected_implementation) =
                loaded.target_implementation().or_else(|| {
                    eprintln!("pop: library artifact has no target implementation");
                    None
                })?;
            if selected_target != target.triple() {
                eprintln!(
                    "pop: library target mismatch: expected {}, found {selected_target}",
                    target.triple()
                );
                return None;
            }
            fs::write(&object, selected_implementation)
                .map_err(|error| eprintln!("pop: could not select library object: {error}"))
                .ok()?;
            let _ = fs::remove_file(&emission_object);
            library_objects.push(object);
        } else if bubble.root_package {
            binary_objects.push((bubble, object));
        }
    }

    let mut executables = Vec::new();
    for (bubble, object) in binary_objects {
        let mut objects = vec![object];
        objects.extend(library_objects.iter().cloned());
        let executable = output_root.join(&bubble.name);
        if link_native_executable(&objects, &executable) != ExitCode::SUCCESS {
            return None;
        }
        executables.push(executable);
    }
    Some(executables)
}

fn run_manifest(manifest_path: &Path, lock_mode: LockMode, arguments: &[OsString]) -> ExitCode {
    let Some(executables) = build_manifest(manifest_path, lock_mode) else {
        return ExitCode::FAILURE;
    };
    let [executable] = executables.as_slice() else {
        eprintln!("pop: `pop run` requires exactly one discovered binary Bubble");
        return ExitCode::FAILURE;
    };
    execute_native(executable, arguments)
}

struct ManifestSelection {
    workspace_root: Option<PathBuf>,
    packages: Vec<PathBuf>,
}

#[derive(Clone)]
struct ResolvedLockPackage {
    name: String,
    version: String,
    library: Option<LockedBubbleIdentity>,
}

struct LockResolutionState {
    root: PathBuf,
    selected_roots: BTreeSet<PathBuf>,
    visiting: BTreeSet<PathBuf>,
    resolved: BTreeMap<PathBuf, ResolvedLockPackage>,
    packages: Vec<LockedPackage>,
    bubbles: Vec<LockedBubble>,
}

fn prepare_lock(selection: &ManifestSelection, mode: LockMode) -> Option<()> {
    let root = selection.workspace_root.clone().unwrap_or_else(|| {
        selection.packages[0]
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    });
    let lock = resolve_selection_lock(selection, &root)?;
    let proposed = encode_lock(&lock)
        .map_err(|error| eprintln!("pop: {error}"))
        .ok()?;
    let lock_path = root.join("bubble.lock");
    let existing = match fs::read(&lock_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            eprintln!("pop: could not read `{}`: {error}", lock_path.display());
            return None;
        }
    };
    let changed = apply_lock_policy(existing.as_deref(), &proposed, mode, false)
        .map_err(|error| eprintln!("pop: {error}"))
        .ok()?;
    if changed {
        write_lock_atomically(&lock_path, &proposed)?;
    }
    Some(())
}

fn resolve_selection_lock(selection: &ManifestSelection, root: &Path) -> Option<BubbleLock> {
    let selected_roots = selection
        .packages
        .iter()
        .map(|manifest| fs::canonicalize(manifest).ok())
        .collect::<Option<BTreeSet<_>>>()?;
    let mut state = LockResolutionState {
        root: fs::canonicalize(root).ok()?,
        selected_roots,
        visiting: BTreeSet::new(),
        resolved: BTreeMap::new(),
        packages: Vec::new(),
        bubbles: Vec::new(),
    };
    let roots = state.selected_roots.iter().cloned().collect::<Vec<_>>();
    for manifest in roots {
        resolve_lock_package(&manifest, &mut state)?;
    }
    BubbleLock::new("1", native_target().triple(), state.packages, state.bubbles)
        .map_err(|error| eprintln!("pop: {error}"))
        .ok()
}

fn resolve_lock_package(
    manifest_path: &Path,
    state: &mut LockResolutionState,
) -> Option<ResolvedLockPackage> {
    let manifest_path = fs::canonicalize(manifest_path).ok()?;
    if let Some(resolved) = state.resolved.get(&manifest_path) {
        return Some(resolved.clone());
    }
    if !state.visiting.insert(manifest_path.clone()) {
        eprintln!(
            "pop: Package dependency cycle includes `{}`",
            manifest_path.display()
        );
        return None;
    }
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|error| eprintln!("pop: could not read `{}`: {error}", manifest_path.display()))
        .ok()?;
    let manifest = parse_package_manifest(&manifest_text)
        .map_err(|error| eprintln!("pop: {error}"))
        .ok()?;

    let mut external_libraries = Vec::new();
    for requirement in manifest.dependencies() {
        let dependency_manifest = match requirement.source() {
            DependencySource::LocalPath(path) => package_root.join(path).join("bubble.toml"),
            DependencySource::Registry => {
                eprintln!(
                    "pop: registry dependency `{}` is not available without registry resolution",
                    requirement.alias()
                );
                return None;
            }
            DependencySource::ExactGit { .. } => {
                eprintln!(
                    "pop: exact-Git dependency `{}` is not available without Git resolution",
                    requirement.alias()
                );
                return None;
            }
            DependencySource::Workspace => {
                eprintln!(
                    "pop: workspace dependency `{}` has no inherited resolution entry",
                    requirement.alias()
                );
                return None;
            }
        };
        let dependency = resolve_lock_package(&dependency_manifest, state)?;
        if requirement
            .version_requirement()
            .is_some_and(|required| required != dependency.version)
        {
            eprintln!("pop: dependency version mismatch for `{}`", dependency.name);
            return None;
        }
        let library = dependency.library.clone().or_else(|| {
            eprintln!(
                "pop: dependency `{}` has no library Bubble",
                dependency.name
            );
            None
        })?;
        external_libraries.push(library);
    }

    let source_paths = collect_package_sources(package_root).ok()?;
    let relative_paths = source_paths.keys().map(String::as_str).collect::<Vec<_>>();
    let discovered = discover_conventional_bubbles(&manifest, &relative_paths)
        .map_err(|error| eprintln!("pop: {error}"))
        .ok()?;
    let selected_root = state.selected_roots.contains(&manifest_path);
    let mut library = None;
    for bubble in discovered.iter().filter(|bubble| {
        bubble.kind() == BubbleKind::Library || selected_root && bubble.kind() == BubbleKind::Binary
    }) {
        let identity = LockedBubbleIdentity::new(manifest.name(), bubble.name(), bubble.kind());
        let mut dependencies = external_libraries.clone();
        if bubble.depends_on_library() {
            dependencies.push(
                library
                    .clone()
                    .expect("conventional discovery sorts the library first"),
            );
        }
        state.bubbles.push(
            LockedBubble::new(manifest.name(), bubble.name(), bubble.kind(), dependencies)
                .map_err(|error| eprintln!("pop: {error}"))
                .ok()?,
        );
        if bubble.kind() == BubbleKind::Library {
            library = Some(identity);
        }
    }

    let source = relative_resolution_path(&state.root, package_root)?;
    let content_hash = package_content_hash(&manifest_path, &source_paths)?;
    state.packages.push(
        LockedPackage::new(
            manifest.name(),
            manifest.version(),
            LockedSource::LocalPath(source),
            content_hash,
            std::iter::empty::<String>(),
        )
        .map_err(|error| eprintln!("pop: {error}"))
        .ok()?,
    );
    let resolved = ResolvedLockPackage {
        name: manifest.name().to_owned(),
        version: manifest.version().to_owned(),
        library,
    };
    state.visiting.remove(&manifest_path);
    state.resolved.insert(manifest_path, resolved.clone());
    Some(resolved)
}

fn package_content_hash(
    manifest_path: &Path,
    sources: &BTreeMap<String, PathBuf>,
) -> Option<String> {
    let mut payload = Vec::new();
    append_hash_input(&mut payload, "bubble.toml", &fs::read(manifest_path).ok()?);
    for (relative, source) in sources {
        append_hash_input(&mut payload, relative, &fs::read(source).ok()?);
    }
    Some(sha256_hex(&payload))
}

fn append_hash_input(payload: &mut Vec<u8>, path: &str, bytes: &[u8]) {
    payload.extend_from_slice(&(path.len() as u64).to_le_bytes());
    payload.extend_from_slice(path.as_bytes());
    payload.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    payload.extend_from_slice(bytes);
}

fn relative_resolution_path(root: &Path, package: &Path) -> Option<String> {
    let root = fs::canonicalize(root).ok()?;
    let package = fs::canonicalize(package).ok()?;
    if root == package {
        return Some(".".to_owned());
    }
    let root_components = root.components().collect::<Vec<_>>();
    let package_components = package.components().collect::<Vec<_>>();
    let common = root_components
        .iter()
        .zip(&package_components)
        .take_while(|(left, right)| left == right)
        .count();
    let mut components = vec!["..".to_owned(); root_components.len().saturating_sub(common)];
    components.extend(
        package_components[common..]
            .iter()
            .map(|component| component.as_os_str().to_string_lossy().into_owned()),
    );
    Some(components.join("/"))
}

fn write_lock_atomically(path: &Path, bytes: &[u8]) -> Option<()> {
    let temporary = path.with_extension(format!("lock.tmp-{}", std::process::id()));
    fs::write(&temporary, bytes)
        .map_err(|error| eprintln!("pop: could not write `{}`: {error}", temporary.display()))
        .ok()?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        eprintln!(
            "pop: could not publish `{}` atomically: {error}",
            path.display()
        );
        return None;
    }
    Some(())
}

fn manifest_selection(manifest_path: &Path) -> Option<ManifestSelection> {
    let manifest_path = fs::canonicalize(manifest_path)
        .map_err(|error| {
            eprintln!(
                "pop: could not resolve `{}`: {error}",
                manifest_path.display()
            );
        })
        .ok()?;
    let text = fs::read_to_string(&manifest_path)
        .map_err(|error| eprintln!("pop: could not read `{}`: {error}", manifest_path.display()))
        .ok()?;
    if !text.lines().any(|line| line.trim() == "[workspace]") {
        return Some(ManifestSelection {
            workspace_root: None,
            packages: vec![manifest_path],
        });
    }

    let workspace = parse_workspace_manifest(&text)
        .map_err(|error| eprintln!("pop: {error}"))
        .ok()?;
    let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    if text.lines().any(|line| line.trim() == "[package]") {
        return Some(ManifestSelection {
            workspace_root: Some(root.to_path_buf()),
            packages: vec![manifest_path],
        });
    }
    let candidates = workspace_candidates(root, &workspace)?;
    let members = discover_workspace_members(&workspace, &candidates)
        .map_err(|error| eprintln!("pop: {error}"))
        .ok()?;
    let selected = if workspace.default_members().is_empty() {
        members
    } else {
        workspace.default_members().to_vec()
    };
    Some(ManifestSelection {
        workspace_root: Some(root.to_path_buf()),
        packages: selected
            .into_iter()
            .map(|member| root.join(member).join("bubble.toml"))
            .collect(),
    })
}

fn workspace_candidates(root: &Path, workspace: &WorkspaceManifest) -> Option<Vec<String>> {
    let mut candidates = BTreeSet::new();
    for pattern in workspace.members() {
        if let Some(prefix) = pattern.strip_suffix("/*") {
            let directory = root.join(prefix);
            let mut entries = fs::read_dir(&directory)
                .map_err(|error| {
                    eprintln!(
                        "pop: could not inspect Workspace member root `{}`: {error}",
                        directory.display()
                    );
                })
                .ok()?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| eprintln!("pop: could not inspect Workspace members: {error}"))
                .ok()?;
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                if entry.file_type().ok()?.is_dir() && entry.path().join("bubble.toml").is_file() {
                    candidates.insert(format!("{prefix}/{}", entry.file_name().to_string_lossy()));
                }
            }
        } else if root.join(pattern).join("bubble.toml").is_file() {
            candidates.insert(pattern.clone());
        }
    }
    Some(candidates.into_iter().collect())
}

fn execute_native(executable: &Path, arguments: &[OsString]) -> ExitCode {
    match Command::new(executable).args(arguments).status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(
            status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or(1),
        ),
        Err(error) => {
            eprintln!("pop: could not execute native program: {error}");
            ExitCode::FAILURE
        }
    }
}

fn collect_package_sources(package_root: &Path) -> Result<BTreeMap<String, PathBuf>, ()> {
    let mut sources = BTreeMap::new();
    for directory in ["src", "tests", "examples", "benchmarks"] {
        collect_sources_in(package_root, &package_root.join(directory), &mut sources)?;
    }
    Ok(sources)
}

fn collect_sources_in(
    package_root: &Path,
    directory: &Path,
    sources: &mut BTreeMap<String, PathBuf>,
) -> Result<(), ()> {
    if !directory.exists() {
        return Ok(());
    }
    let mut entries = fs::read_dir(directory)
        .map_err(|error| eprintln!("pop: could not inspect `{}`: {error}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| eprintln!("pop: could not inspect `{}`: {error}", directory.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|error| eprintln!("pop: could not inspect source entry: {error}"))?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_sources_in(package_root, &path, sources)?;
        } else if file_type.is_file() && path.extension() == Some(OsStr::new("pop")) {
            let relative = path
                .strip_prefix(package_root)
                .map_err(|_| eprintln!("pop: Package source escaped its root"))?
                .to_string_lossy()
                .replace('\\', "/");
            sources.insert(relative, path);
        }
    }
    Ok(())
}

fn emit_native_object(program: &NativeProgram, output_path: &Path) -> Option<()> {
    let target = native_target();
    let options = program
        .entry
        .map_or_else(LlvmLoweringOptions::default, |entry| {
            LlvmLoweringOptions::default().with_entry_point(entry)
        });
    let module = lower_mir_to_llvm_ir(&program.mir, &program.types, &target, options)
        .map_err(|error| eprintln!("pop: LLVM lowering failed: {error}"))
        .ok()?;
    module
        .emit_object(output_path)
        .map_err(|error| eprintln!("pop: {error}"))
        .ok()
}

struct NativeProgram {
    mir: pop_mir::MirBubble,
    types: pop_types::TypeArena,
    entry: Option<SymbolId>,
    reference_metadata: ReferenceMetadata,
    checked_documentation: Vec<CheckedDocumentation>,
}

fn lower_native_source(source_path: &Path) -> Option<NativeProgram> {
    let (standard, _) = lower_toolchain_standard()?;
    lower_native_bubble(
        BubbleId::from_raw(FIRST_PACKAGE_BUBBLE),
        &[(source_path.to_path_buf(), source_path.to_path_buf())],
        true,
        vec![standard.metadata],
        Vec::new(),
    )
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("driver crate is below repository root")
        .to_path_buf()
}

fn lower_toolchain_standard() -> Option<(ResolvedPackageLibrary, LoweredPackageBubble)> {
    let package_root = repository_root().join("crates/libraries/standard/pop");
    let manifest_path = package_root.join("bubble.toml");
    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|error| eprintln!("pop: could not read reserved Standard manifest: {error}"))
        .ok()?;
    let manifest = parse_package_manifest(&manifest_text)
        .map_err(|error| eprintln!("pop: invalid reserved Standard manifest: {error}"))
        .ok()?;
    if manifest.name() != "Pop.Standard" {
        eprintln!("pop: reserved Standard manifest has the wrong identity");
        return None;
    }
    let source_paths = collect_package_sources(&package_root).ok()?;
    let relative_paths = source_paths.keys().map(String::as_str).collect::<Vec<_>>();
    let discovered = discover_conventional_bubbles(&manifest, &relative_paths)
        .map_err(|error| eprintln!("pop: could not discover reserved Standard: {error}"))
        .ok()?;
    let [bubble] = discovered.as_slice() else {
        eprintln!("pop: reserved Standard must contain exactly one library Bubble");
        return None;
    };
    if bubble.kind() != BubbleKind::Library {
        eprintln!("pop: reserved Standard must be a library Bubble");
        return None;
    }
    let modules = bubble
        .modules()
        .iter()
        .map(|relative| {
            source_paths
                .get(relative)
                .cloned()
                .map(|source| (PathBuf::from(relative), source))
        })
        .collect::<Option<Vec<_>>>()?;
    let program = lower_native_bubble(
        STANDARD_BUBBLE,
        &modules,
        false,
        Vec::new(),
        vec![INTERNAL_BUBBLE],
    )?;
    let reference = encode_reference_metadata(&program.reference_metadata)
        .map_err(|error| eprintln!("pop: Standard metadata encoding failed: {error}"))
        .ok()?;
    let source_sha256 = package_content_hash(&manifest_path, &source_paths)?;
    let library = ResolvedPackageLibrary {
        package: manifest.name().to_owned(),
        version: manifest.version().to_owned(),
        source_sha256: source_sha256.clone(),
        bubble: bubble.name().to_owned(),
        public_api_sha256: sha256_hex(&reference),
        metadata: program.reference_metadata.clone(),
    };
    let lowered = LoweredPackageBubble {
        bubble: STANDARD_BUBBLE,
        package: manifest.name().to_owned(),
        version: manifest.version().to_owned(),
        source_sha256,
        edition: manifest.edition().to_owned(),
        name: bubble.name().to_owned(),
        kind: BubbleKind::Library,
        root_package: false,
        dependencies: Vec::new(),
        program,
    };
    Some((library, lowered))
}

fn lower_native_bubble(
    bubble: BubbleId,
    modules: &[(PathBuf, PathBuf)],
    requires_entry: bool,
    dependency_metadata: Vec<ReferenceMetadata>,
    additional_dependencies: Vec<BubbleId>,
) -> Option<NativeProgram> {
    let modules = modules
        .iter()
        .enumerate()
        .map(|(index, (display_path, source_path))| {
            let source_text = fs::read_to_string(source_path).map_err(|error| {
                eprintln!("pop: could not read `{}`: {error}", source_path.display());
            })?;
            let file = u32::try_from(index).map_err(|_| {
                eprintln!("pop: too many Modules in one Bubble");
            })?;
            let source = SourceFile::new(
                FileId::from_raw(file),
                display_path.to_string_lossy().into_owned(),
                source_text,
            )
            .map_err(|error| {
                eprintln!("pop: could not load `{}`: {error}", source_path.display());
            })?;
            Ok(FrontEndModule::new(ModuleId::from_raw(file), source))
        })
        .collect::<Result<Vec<_>, ()>>()
        .ok()?;
    let mut dependencies = dependency_metadata
        .iter()
        .map(ReferenceMetadata::bubble)
        .collect::<Vec<_>>();
    dependencies.extend(additional_dependencies);
    dependencies.sort();
    dependencies.dedup();
    let input = FrontEndBubbleInput::new(
        bubble,
        NamespaceId::from_raw(bubble.raw()),
        dependencies,
        modules,
    )
    .with_reference_metadata(dependency_metadata);
    let input = if requires_entry {
        input.with_implicit_main_entry(ModuleId::from_raw(0))
    } else {
        input
    };
    let result = analyze_bubble(input);
    if !result.diagnostics().is_empty() {
        let _ = write_diagnostics(&result.diagnostic_snapshot());
        return None;
    }
    let hir = result.hir()?;
    let entry = if requires_entry {
        Some(select_native_entry(hir, result.types())?)
    } else {
        None
    };
    let mir = lower_hir_bubble(hir, result.types())
        .map_err(|errors| {
            eprintln!(
                "pop: internal compiler error: canonical MIR verification failed: {errors:?}"
            );
        })
        .ok()?;
    let mir = optimize_mir(mir, result.types())
        .map_err(|errors| {
            eprintln!(
                "pop: internal compiler error: optimized MIR verification failed: {errors:?}"
            );
        })
        .ok()?;
    let reference_metadata = result
        .reference_metadata()
        .map_err(|error| eprintln!("pop: public reference metadata emission failed: {error:?}"))
        .ok()?
        .clone();
    let checked_documentation = result.checked_documentation().to_vec();
    Some(NativeProgram {
        mir,
        types: result.types().clone(),
        entry,
        reference_metadata,
        checked_documentation,
    })
}

fn select_native_entry(hir: &pop_hir::HirBubble, types: &pop_types::TypeArena) -> Option<SymbolId> {
    let int_type = types.source_type("Int")?;
    let string_type = types.source_type("String")?;
    let candidates: Vec<_> = hir
        .functions()
        .iter()
        .filter(|function| function.name() == "main")
        .collect();
    let [entry] = candidates.as_slice() else {
        write_invalid_entry();
        return None;
    };
    let parameters_are_valid = entry.parameters().is_empty()
        || entry.parameters().len() == 1
            && entry.parameters().first().is_some_and(|parameter| {
                matches!(
                    types.get(parameter.type_id()),
                    Some(SemanticType::Array(element)) if *element == string_type
                )
            });
    if entry.visibility() != Visibility::Private
        || !parameters_are_valid
        || !(entry.results().is_empty() || entry.results() == [int_type])
    {
        write_invalid_entry();
        return None;
    }
    Some(entry.symbol())
}

fn write_invalid_entry() {
    eprintln!(
        "pop: binary entry must be private or implicit `main` with no parameters or `Array<String>`, and with no result or `Int`"
    );
}

fn link_native_executable(object_paths: &[PathBuf], output_path: &Path) -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("driver crate is under repository root");
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };

    let executable_directory = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let distributed_archives = executable_directory.map(|directory| {
        (
            directory.join("libpop_standard.a"),
            directory.join("libpop_runtime_native.a"),
        )
    });
    let (standard, runtime) = distributed_archives
        .filter(|(standard, runtime)| standard.is_file() && runtime.is_file())
        .unwrap_or_else(|| {
            (
                root.join(format!("target/{profile}/libpop_standard.a")),
                root.join(format!("target/{profile}/libpop_runtime_native.a")),
            )
        });

    if !standard.is_file() || !runtime.is_file() {
        let mut command = Command::new("cargo");
        command
            .current_dir(root)
            .args(["build", "-p", "pop-standard", "-p", "pop-runtime-native"]);
        if profile == "release" {
            command.arg("--release");
        }
        if !matches!(command.status(), Ok(status) if status.success()) {
            eprintln!("pop: could not build native foundation archives");
            return ExitCode::FAILURE;
        }
    }

    if !standard.is_file() || !runtime.is_file() {
        eprintln!("pop: native foundation archives were not produced");
        return ExitCode::FAILURE;
    }

    let mut command = Command::new("clang");
    command
        .args(object_paths)
        .arg(&standard)
        .arg(&runtime)
        // Both Rust static libraries include identical copies of their shared
        // runtime-interface dependency from the same Cargo build.
        .arg("-Wl,--allow-multiple-definition");

    let link = command.arg("-o").arg(output_path).output();

    match link {
        Ok(output) if output.status.success() => ExitCode::SUCCESS,
        Ok(output) => {
            eprintln!(
                "pop: native link failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("pop: could not invoke native linker: {error}");
            ExitCode::FAILURE
        }
    }
}
