//! Unified `pop` command and build orchestration.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use pop_backend_c::{CLoweringOptions, lower_mir_to_c};
use pop_backend_llvm::{LlvmLoweringOptions, lower_mir_to_llvm_ir};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId, SymbolId};
use pop_mir::{lower_hir_bubble, optimize_mir};
use pop_projects::{BubbleKind, discover_conventional_bubbles, parse_package_manifest};
use pop_resolve::Visibility;
use pop_source::SourceFile;
use pop_target::{Endianness, PointerWidth, TargetSpec};
use pop_types::SemanticType;

const USAGE: &str = "\
Usage:
    pop check <source.pop> [--dump <hir|mir>]...
    pop build <source.pop> --output <executable>
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
}

#[derive(Debug, Eq, PartialEq)]
enum CommandLine {
    Help,
    Check {
        source_path: PathBuf,
        dumps: Vec<DumpKind>,
    },
    Build {
        source_path: PathBuf,
        output_path: PathBuf,
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
        arguments: Vec<OsString>,
    },
}

fn main() -> ExitCode {
    match parse_arguments(std::env::args_os().skip(1)) {
        Ok(CommandLine::Help) => write_help(),
        Ok(CommandLine::Check { source_path, dumps }) => check_source(&source_path, &dumps),
        Ok(CommandLine::Build {
            source_path,
            output_path,
        }) => build_source(&source_path, &output_path),
        Ok(CommandLine::TranspileToC { source_path }) => transpile_source_to_c(&source_path),
        Ok(CommandLine::Run {
            source_path,
            arguments,
        }) => run_source(&source_path, &arguments),
        Ok(CommandLine::PackageRun {
            manifest_path,
            arguments,
        }) => run_package(&manifest_path, &arguments),
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
    if command == "run" {
        return parse_run_arguments(arguments);
    }
    if command != "check" {
        return Err(format!(
            "unsupported command `{}`",
            command.to_string_lossy()
        ));
    }

    let mut source_path = None;
    let mut dumps = Vec::new();
    while let Some(argument) = arguments.next() {
        if argument == "--help" || argument == "-h" {
            return Ok(CommandLine::Help);
        }
        if argument == "--dump" {
            let Some(kind) = arguments.next() else {
                return Err("`--dump` requires hir|mir".to_owned());
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
    let source_path = required_source_path(arguments.next(), "build")?;
    let Some(option) = arguments.next() else {
        return Err("`pop build` requires `--output <executable>`".to_owned());
    };
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

fn parse_run_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, String> {
    let Some(first) = arguments.next() else {
        return Err("`pop run` requires a source path or `--manifestPath`".to_owned());
    };
    if first == "--manifestPath" {
        let manifest_path = arguments
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| "`--manifestPath` requires a bubble.toml path".to_owned())?;
        let program_arguments = parse_program_arguments(arguments)?;
        return Ok(CommandLine::PackageRun {
            manifest_path,
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
        _ => Err(format!(
            "unsupported dump kind `{}`; expected hir|mir",
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
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
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

    let mut output = String::new();
    for dump in dumps {
        match dump {
            DumpKind::Hir => output.push_str(&hir.dump(result.types())),
            DumpKind::Mir => output.push_str(&mir.dump()),
        }
    }
    write_output(&output)
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
    let target = TargetSpec::builder("x86_64-unknown-linux-gnu")
        .pointer_width(PointerWidth::Bits64)
        .endianness(Endianness::Little)
        .build()
        .expect("repository native target is complete");
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
    if let Err(error) = module.emit_object(&object_path) {
        eprintln!("pop: {error}");
        return ExitCode::FAILURE;
    }
    let result = link_native_executable(std::slice::from_ref(&object_path), output_path);
    let _ = fs::remove_file(object_path);
    result
}

fn transpile_source_to_c(source_path: &Path) -> ExitCode {
    let Some(program) = lower_native_source(source_path) else {
        return ExitCode::FAILURE;
    };
    let NativeProgram { mir, types, entry } = program;
    let optimized = match optimize_mir(mir, &types) {
        Ok(mir) => mir,
        Err(errors) => {
            eprintln!("pop: internal compiler error: optimized MIR verification failed");
            for error in errors {
                eprintln!("  {error:?}");
            }
            return ExitCode::from(101);
        }
    };
    let options = CLoweringOptions::default()
        .with_entry_point(entry.expect("standalone transpilation has a verified entry"));
    let translation = match lower_mir_to_c(&optimized, &types, options) {
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

fn build_package(manifest_path: &Path) -> Option<Vec<PathBuf>> {
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let manifest_text = fs::read_to_string(manifest_path)
        .map_err(|error| {
            eprintln!("pop: could not read `{}`: {error}", manifest_path.display());
        })
        .ok()?;
    let manifest = parse_package_manifest(&manifest_text)
        .map_err(|error| eprintln!("pop: {error}"))
        .ok()?;
    if !manifest.dependencies().is_empty() {
        eprintln!("pop: Package dependency resolution is not available in this native slice");
        return None;
    }
    let source_paths = collect_package_sources(package_root).ok()?;
    let relative_paths: Vec<_> = source_paths.keys().map(String::as_str).collect();
    let bubbles = discover_conventional_bubbles(&manifest, &relative_paths)
        .map_err(|error| eprintln!("pop: {error}"))
        .ok()?;
    let selected: Vec<_> = bubbles
        .iter()
        .filter(|bubble| matches!(bubble.kind(), BubbleKind::Library | BubbleKind::Binary))
        .collect();
    if selected.is_empty() {
        eprintln!("pop: Package has no library or binary Bubbles");
        return None;
    }

    let mut lowered = Vec::new();
    for (index, bubble) in selected.iter().enumerate() {
        let bubble_id = u32::try_from(index).ok().map(BubbleId::from_raw)?;
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
        let program =
            lower_native_bubble(bubble_id, &modules, bubble.kind() == BubbleKind::Binary)?;
        lowered.push((*bubble, program));
    }

    let output_root = package_root.join("target/debug");
    let dependency_root = output_root.join("deps");
    fs::create_dir_all(&dependency_root)
        .map_err(|error| eprintln!("pop: could not create build output: {error}"))
        .ok()?;
    let mut library_objects = Vec::new();
    let mut binary_objects = Vec::new();
    for (bubble, program) in &lowered {
        let suffix = if bubble.kind() == BubbleKind::Library {
            "library"
        } else {
            "binary"
        };
        let object = dependency_root.join(format!("{}.{}.o", bubble.name(), suffix));
        emit_native_object(program, &object)?;
        if bubble.kind() == BubbleKind::Library {
            library_objects.push(object);
        } else {
            binary_objects.push((*bubble, object));
        }
    }

    let mut executables = Vec::new();
    for (bubble, object) in binary_objects {
        let mut objects = vec![object];
        if bubble.depends_on_library() {
            objects.extend(library_objects.iter().cloned());
        }
        let executable = output_root.join(bubble.name());
        if link_native_executable(&objects, &executable) != ExitCode::SUCCESS {
            return None;
        }
        executables.push(executable);
    }
    Some(executables)
}

fn run_package(manifest_path: &Path, arguments: &[OsString]) -> ExitCode {
    let Some(executables) = build_package(manifest_path) else {
        return ExitCode::FAILURE;
    };
    let [executable] = executables.as_slice() else {
        eprintln!("pop: `pop run` requires exactly one discovered binary Bubble");
        return ExitCode::FAILURE;
    };
    execute_native(executable, arguments)
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
    let target = TargetSpec::builder("x86_64-unknown-linux-gnu")
        .pointer_width(PointerWidth::Bits64)
        .endianness(Endianness::Little)
        .build()
        .expect("repository native target is complete");
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
}

fn lower_native_source(source_path: &Path) -> Option<NativeProgram> {
    lower_native_bubble(
        BubbleId::from_raw(0),
        &[(source_path.to_path_buf(), source_path.to_path_buf())],
        true,
    )
}

fn lower_native_bubble(
    bubble: BubbleId,
    modules: &[(PathBuf, PathBuf)],
    requires_entry: bool,
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
    let input = FrontEndBubbleInput::new(
        bubble,
        NamespaceId::from_raw(bubble.raw()),
        Vec::new(),
        modules,
    );
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
    Some(NativeProgram {
        mir,
        types: result.types().clone(),
        entry,
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

    let standard = root.join("target/debug/libpop_standard.a");
    let runtime = root.join("target/debug/libpop_runtime_native.a");

    let build = Command::new("cargo")
        .current_dir(root)
        .args([
            "build",
            "-p",
            "pop-standard",
            "-p",
            "pop-runtime-native",
        ])
        .status();

    if !matches!(build, Ok(status) if status.success()) {
        eprintln!("pop: could not build bootstrap foundation archives");
        return ExitCode::FAILURE;
    }

    if !standard.is_file() || !runtime.is_file() {
        eprintln!("pop: bootstrap foundation archives were not produced");
        return ExitCode::FAILURE;
    }

    let mut command = Command::new("clang");
    command.args(object_paths).arg(&standard).arg(&runtime);

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
