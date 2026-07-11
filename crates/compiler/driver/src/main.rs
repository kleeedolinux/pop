//! Unified `pop` command and build orchestration.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::lower_hir_bubble;
use pop_source::SourceFile;

const USAGE: &str = "\
Usage:
    pop check <source.pop> [--dump <hir|mir>]...

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
}

fn main() -> ExitCode {
    match parse_arguments(std::env::args_os().skip(1)) {
        Ok(CommandLine::Help) => write_help(),
        Ok(CommandLine::Check { source_path, dumps }) => check_source(&source_path, &dumps),
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
