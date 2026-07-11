//! Unified `pop` command and build orchestration.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use pop_backend_llvm::{LlvmLoweringOptions, lower_mir_to_llvm_ir};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::lower_hir_bubble;
use pop_source::SourceFile;
use pop_target::{Endianness, PointerWidth, TargetSpec};

const USAGE: &str = "\
Usage:
    pop check <source.pop> [--dump <hir|mir>]...
    pop build <source.pop> --output <executable>
    pop run <source.pop>

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
    Run {
        source_path: PathBuf,
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
        Ok(CommandLine::Run { source_path }) => run_source(&source_path),
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
    let source_path = required_source_path(arguments.next(), "run")?;
    if arguments.next().is_some() {
        return Err("`pop run` accepts exactly one source path in bootstrap mode".to_owned());
    }
    Ok(CommandLine::Run { source_path })
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
    let Some((mir, types)) = lower_source(source_path) else {
        return ExitCode::FAILURE;
    };
    let Some(int_type) = types.source_type("Int") else {
        eprintln!("pop: internal compiler error: missing canonical Int type");
        return ExitCode::from(101);
    };
    let mut entries = mir
        .functions()
        .iter()
        .filter(|function| function.parameters().is_empty() && function.results() == [int_type]);
    let Some(entry) = entries.next() else {
        eprintln!("pop: native bootstrap requires one function with signature () -> Int");
        return ExitCode::FAILURE;
    };
    if entries.next().is_some() {
        eprintln!("pop: native bootstrap found more than one () -> Int entry candidate");
        return ExitCode::FAILURE;
    }
    let target = TargetSpec::builder("x86_64-unknown-linux-gnu")
        .pointer_width(PointerWidth::Bits64)
        .endianness(Endianness::Little)
        .build()
        .expect("repository native target is complete");
    let module = match lower_mir_to_llvm_ir(
        &mir,
        &types,
        &target,
        LlvmLoweringOptions::default()
            .with_entry_point(entry.symbol()),
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
    let result = link_native_executable(&object_path, output_path);
    let _ = fs::remove_file(object_path);
    result
}

fn run_source(source_path: &Path) -> ExitCode {
    let executable = std::env::temp_dir().join(format!("pop-run-{}", std::process::id()));
    let build = build_source(source_path, &executable);
    if build != ExitCode::SUCCESS {
        return build;
    }
    let status = Command::new(&executable).status();
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

fn lower_source(source_path: &Path) -> Option<(pop_mir::MirBubble, pop_types::TypeArena)> {
    let source_text = fs::read_to_string(source_path)
        .map_err(|error| {
            eprintln!("pop: could not read `{}`: {error}", source_path.display());
        })
        .ok()?;
    let source = SourceFile::new(
        FileId::from_raw(0),
        source_path.to_string_lossy().into_owned(),
        source_text,
    )
    .map_err(|error| {
        eprintln!("pop: could not load `{}`: {error}", source_path.display());
    })
    .ok()?;
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    if !result.diagnostics().is_empty() {
        let _ = write_diagnostics(&result.diagnostic_snapshot());
        return None;
    }
    let hir = result.hir()?;
    let mir = lower_hir_bubble(hir, result.types())
        .map_err(|errors| {
            eprintln!(
                "pop: internal compiler error: canonical MIR verification failed: {errors:?}"
            );
        })
        .ok()?;
    Some((mir, result.types().clone()))
}

fn link_native_executable(object_path: &Path, output_path: &Path) -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("driver crate is under repository root");
    let standard = root.join("target/debug/libpop_standard.a");
    let runtime = root.join("target/debug/libpop_runtime_native.a");
    if !standard.is_file() || !runtime.is_file() {
        let build = Command::new("cargo")
            .current_dir(root)
            .args(["build", "-p", "pop-standard", "-p", "pop-runtime-native"])
            .status();
        if !matches!(build, Ok(status) if status.success()) {
            eprintln!("pop: could not build bootstrap foundation archives");
            return ExitCode::FAILURE;
        }
    }
    let link = Command::new("clang")
        .arg(object_path)
        .arg(&standard)
        .arg(&runtime)
        .arg("-o")
        .arg(output_path)
        .output();
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
