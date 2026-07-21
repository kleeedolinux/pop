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
use std::sync::{Arc, OnceLock};

use pop_backend_api::RuntimeProfile;
use pop_backend_c::{CLoweringOptions, lower_mir_to_c};
use pop_backend_llvm::{
    BpfLoweringOptions, BpfProgramKind, LlvmLoweringOptions, lower_mir_to_bpf_module,
    lower_mir_to_llvm_ir,
};
use pop_documentation_generator::{DocumentationMember, render_xml};
use pop_driver::{
    CheckedDocumentation, FrontEndBubbleInput, FrontEndModule, NativeLinkInput,
    NativeLinkPlanSource, PoplibDependency, PoplibEmission, ReferenceFunction, ReferenceMetadata,
    ReferenceType, VerifiedFfiGeneratedBindings, analyze_bubble, artifact_sha256_hex, emit_poplib,
    encode_reference_metadata, generate_ffi_bindings, load_poplib, resolve_native_link_inputs,
    validate_foreign_link_aliases, verify_ffi_generated_bindings,
};
use pop_foundation::{BubbleId, Diagnostic, FileId, ModuleId, NamespaceId, SymbolId};
use pop_localization::{
    Argument as LocalizedArgument, Language, RenderContext, select_process_language,
};
use pop_mir::{lower_hir_bubble_with_fingerprint, optimize_mir};
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
const INTERNAL_PACKAGE_NAME: &str = "Pop.Internal";
const STANDARD_PACKAGE_NAME: &str = "Pop.Standard";
const FFI_PACKAGE_NAME: &str = "Pop.Ffi";

static CLI_RENDERING: OnceLock<RenderContext> = OnceLock::new();

macro_rules! tool_failure {
    ($($argument:tt)*) => {{
        let detail = format!($($argument)*);
        let detail = detail.strip_prefix("pop: ").unwrap_or(&detail);
        emit_localized(
            "cli.toolFailure",
            &[LocalizedArgument::external("detail", detail)],
        );
    }};
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DumpKind {
    Hir,
    Mir,
    Ll,
}

#[derive(Debug, Eq, PartialEq)]
enum CommandLine {
    Help,
    Scaffold(ScaffoldOptions),
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
    FfiGenerate {
        alias: String,
        manifest_path: PathBuf,
        platform_target: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScaffoldMode {
    New,
    Initialize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScaffoldKind {
    Binary,
    Library,
}

#[derive(Debug, Eq, PartialEq)]
struct ScaffoldOptions {
    mode: ScaffoldMode,
    path: PathBuf,
    name: Option<String>,
    kind: ScaffoldKind,
}

#[derive(Debug)]
struct UsageError {
    key: &'static str,
    arguments: Vec<LocalizedArgument>,
}

impl UsageError {
    fn new(key: &'static str, arguments: Vec<LocalizedArgument>) -> Self {
        Self { key, arguments }
    }

    fn simple(key: &'static str) -> Self {
        Self::new(key, Vec::new())
    }

    fn render(&self) -> String {
        localized(self.key, &self.arguments)
    }
}

fn main() -> ExitCode {
    let (explicit_language, arguments) = match extract_language(std::env::args_os().skip(1)) {
        Ok(selection) => selection,
        Err(LanguageOptionError::Unsupported(requested)) => {
            let _ = initialize_rendering(None);
            emit_localized(
                "cli.unsupportedLanguage",
                &[LocalizedArgument::text("language", requested)],
            );
            return ExitCode::from(2);
        }
        Err(LanguageOptionError::MissingValue) => {
            let _ = initialize_rendering(None);
            emit_localized("cli.languageNeedsValue", &[]);
            return ExitCode::from(2);
        }
    };
    if let Err(error) = initialize_rendering(explicit_language.as_deref()) {
        let _ = initialize_rendering(None);
        emit_localized(
            "cli.selectLanguageFailed",
            &[LocalizedArgument::external("detail", error)],
        );
        return ExitCode::from(2);
    }
    match parse_arguments(arguments) {
        Ok(CommandLine::Help) => write_help(),
        Ok(CommandLine::Scaffold(options)) => scaffold_package(&options),
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
        Ok(CommandLine::FfiGenerate {
            alias,
            manifest_path,
            platform_target,
        }) => ffi_generate(&alias, &manifest_path, &platform_target),
        Err(error) => {
            let _ = writeln!(
                io::stderr().lock(),
                "pop: {}\n\n{}",
                error.render(),
                localized("cli.usage", &[])
            );
            ExitCode::from(2)
        }
    }
}

fn initialize_rendering(explicit: Option<&str>) -> Result<(), pop_localization::LocalizationError> {
    if CLI_RENDERING.get().is_some() {
        return Ok(());
    }
    let language = select_process_language(explicit)?;
    let _ = CLI_RENDERING.set(RenderContext::new(language));
    Ok(())
}

fn rendering() -> RenderContext {
    CLI_RENDERING
        .get()
        .copied()
        .unwrap_or_else(|| RenderContext::new(Language::English))
}

fn localized(key: &str, arguments: &[LocalizedArgument]) -> String {
    rendering()
        .message(key, arguments)
        .unwrap_or_else(|error| format!("localization failure: {error}"))
}

fn emit_localized(key: &str, arguments: &[LocalizedArgument]) {
    let _ = writeln!(io::stderr().lock(), "pop: {}", localized(key, arguments));
}

fn extract_language(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<(Option<String>, Vec<OsString>), LanguageOptionError> {
    let mut output = Vec::new();
    let mut explicit = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if argument == "--" {
            output.push(argument);
            output.extend(arguments);
            break;
        }
        if argument == "--language" {
            let value = arguments.next().ok_or(LanguageOptionError::MissingValue)?;
            let value = value.to_string_lossy().into_owned();
            if Language::from_tag(&value).is_none() {
                return Err(LanguageOptionError::Unsupported(value));
            }
            explicit = Some(value);
        } else {
            output.push(argument);
        }
    }
    Ok((explicit, output))
}

enum LanguageOptionError {
    MissingValue,
    Unsupported(String),
}

fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<CommandLine, UsageError> {
    let mut arguments = arguments.into_iter();
    let Some(command) = arguments.next() else {
        return Err(UsageError::simple("cli.missingCommand"));
    };
    if command == "--help" || command == "-h" {
        return Ok(CommandLine::Help);
    }
    if command == "new" || command == "initialize" {
        return parse_scaffold_arguments(command == "new", arguments);
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
    if command == "ffi" {
        return parse_ffi_arguments(arguments);
    }
    if command != "check" {
        return Err(UsageError::new(
            "cli.unsupportedCommand",
            vec![LocalizedArgument::text(
                "command",
                command.to_string_lossy(),
            )],
        ));
    }

    parse_check_arguments(arguments)
}

fn parse_scaffold_arguments(
    create_new: bool,
    arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, UsageError> {
    let mode = if create_new {
        ScaffoldMode::New
    } else {
        ScaffoldMode::Initialize
    };
    let mut path = None;
    let mut name = None;
    let mut kind = ScaffoldKind::Binary;
    let mut selected_kind = false;
    let mut arguments = arguments.peekable();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--name") => {
                if name.is_some() {
                    return Err(unsupported_option(&argument));
                }
                name = Some(
                    arguments
                        .next()
                        .ok_or_else(|| option_requires("--name", "<Package.Name>"))?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            Some("--library" | "--binary") => {
                if selected_kind {
                    return Err(UsageError::simple("cli.scaffoldKindConflict"));
                }
                selected_kind = true;
                kind = if argument == "--library" {
                    ScaffoldKind::Library
                } else {
                    ScaffoldKind::Binary
                };
            }
            Some(value) if value.starts_with('-') => return Err(unsupported_option(&argument)),
            _ if path.is_none() => path = Some(PathBuf::from(argument)),
            _ => {
                return Err(unexpected_arguments(if create_new {
                    "new"
                } else {
                    "initialize"
                }));
            }
        }
    }
    let path = match (mode, path) {
        (ScaffoldMode::New | ScaffoldMode::Initialize, Some(path)) => path,
        (ScaffoldMode::Initialize, None) => PathBuf::from("."),
        (ScaffoldMode::New, None) => return Err(UsageError::simple("cli.newNeedsPath")),
    };
    Ok(CommandLine::Scaffold(ScaffoldOptions {
        mode,
        path,
        name,
        kind,
    }))
}

fn parse_ffi_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, UsageError> {
    let Some(action) = arguments.next() else {
        return Err(command_requires("ffi", "generate"));
    };
    if action != "generate" {
        return Err(UsageError::new(
            "cli.unsupportedChoice",
            vec![
                LocalizedArgument::text("choice", "`pop ffi` action"),
                LocalizedArgument::text("value", action.to_string_lossy()),
                LocalizedArgument::text("expected", "generate"),
            ],
        ));
    }
    let alias = arguments
        .next()
        .ok_or_else(|| command_requires("ffi generate", "<alias>"))?;
    let alias = alias.into_string().map_err(|_| {
        UsageError::new(
            "cli.unsupportedChoice",
            vec![
                LocalizedArgument::text("choice", "manifest alias"),
                LocalizedArgument::text("value", "non-UTF-8 input"),
                LocalizedArgument::text("expected", "UTF-8"),
            ],
        )
    })?;
    if alias.starts_with('-') {
        return Err(command_requires("ffi generate", "<alias> before options"));
    }
    let mut manifest_path = None;
    let mut platform_target = None;
    while let Some(option) = arguments.next() {
        match option.to_str() {
            Some("--manifestPath") if manifest_path.is_none() => {
                manifest_path = Some(required_manifest_path(arguments.next(), "ffi generate")?);
            }
            Some("--platformTarget") if platform_target.is_none() => {
                platform_target = Some(
                    arguments
                        .next()
                        .ok_or_else(|| option_requires("--platformTarget", "a target triple"))?
                        .into_string()
                        .map_err(|_| {
                            UsageError::new(
                                "cli.unsupportedChoice",
                                vec![
                                    LocalizedArgument::text("choice", "platform target"),
                                    LocalizedArgument::text("value", "non-UTF-8 input"),
                                    LocalizedArgument::text("expected", "UTF-8"),
                                ],
                            )
                        })?,
                );
            }
            _ => {
                return Err(expected_option(
                    &option,
                    "--manifestPath or --platformTarget",
                ));
            }
        }
    }
    Ok(CommandLine::FfiGenerate {
        alias,
        manifest_path: manifest_path
            .ok_or_else(|| command_requires("ffi generate", "--manifestPath <bubble.toml>"))?,
        platform_target: platform_target
            .ok_or_else(|| command_requires("ffi generate", "--platformTarget <triple>"))?,
    })
}

fn ffi_generate(alias: &str, manifest_path: &Path, platform_target: &str) -> ExitCode {
    match generate_ffi_bindings(manifest_path, platform_target, alias) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tool_failure!("pop: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_documentation_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, UsageError> {
    let Some(option) = arguments.next() else {
        return Err(command_requires(
            "documentation",
            "--manifestPath <bubble.toml>",
        ));
    };
    if option != "--manifestPath" {
        return Err(expected_option(&option, "--manifestPath"));
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
) -> Result<CommandLine, UsageError> {
    let Some(first) = arguments.next() else {
        return Err(UsageError::simple("cli.checkNeedsSource"));
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
                return Err(option_requires("--dump", "hir|mir|ll"));
            };
            let kind = parse_dump_kind(&kind)?;
            if !dumps.contains(&kind) {
                dumps.push(kind);
            }
            continue;
        }
        if argument.to_string_lossy().starts_with('-') {
            return Err(unsupported_option(&argument));
        }
        if source_path.replace(PathBuf::from(argument)).is_some() {
            return Err(UsageError::new(
                "cli.oneSource",
                vec![LocalizedArgument::text("command", "check")],
            ));
        }
    }

    let source_path = source_path.ok_or_else(|| source_required("check"))?;
    if source_path.extension() != Some(OsStr::new("pop")) {
        return Err(source_required("check"));
    }
    Ok(CommandLine::Check { source_path, dumps })
}

fn parse_transpile_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, UsageError> {
    let source_path = required_source_path(arguments.next(), "transpile")?;
    let Some(option) = arguments.next() else {
        return Err(command_requires("transpile", "--to c"));
    };
    if option != "--to" {
        return Err(unsupported_option(&option));
    }
    let Some(target) = arguments.next() else {
        return Err(UsageError::simple("cli.transpileNeedsFormat"));
    };
    if target != "c" {
        return Err(UsageError::new(
            "cli.unsupportedTranspileTarget",
            vec![LocalizedArgument::text("value", target.to_string_lossy())],
        ));
    }
    if arguments.next().is_some() {
        return Err(unexpected_arguments("transpile"));
    }
    Ok(CommandLine::TranspileToC { source_path })
}

fn parse_build_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, UsageError> {
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
        return Err(UsageError::simple("cli.buildNeedsOutputOrTarget"));
    };
    if option == "--target" {
        let target = arguments
            .next()
            .ok_or_else(|| UsageError::simple("cli.targetNeedsTriple"))?
            .to_string_lossy()
            .into_owned();
        let Some(runtime_option) = arguments.next() else {
            return Err(bpf_requires("--runtime-profile linux-ebpf"));
        };
        if runtime_option != "--runtime-profile" {
            return Err(expected_option(&runtime_option, "--runtime-profile"));
        }
        let runtime_profile = arguments
            .next()
            .ok_or_else(|| UsageError::simple("cli.runtimeProfileNeedsName"))
            .and_then(|profile| {
                RuntimeProfile::parse(&profile.to_string_lossy()).map_err(|_| {
                    UsageError::new(
                        "cli.unsupportedRuntimeProfile",
                        vec![LocalizedArgument::text("value", profile.to_string_lossy())],
                    )
                })
            })?;
        let Some(program_option) = arguments.next() else {
            return Err(bpf_requires("--bpf-program xdp"));
        };
        if program_option != "--bpf-program" {
            return Err(expected_option(&program_option, "--bpf-program"));
        }
        let program = match arguments.next().as_deref() {
            Some(value) if value == OsStr::new("xdp") => BpfProgramKind::Xdp,
            Some(value) => {
                return Err(UsageError::new(
                    "cli.unsupportedBpfProgram",
                    vec![LocalizedArgument::text("value", value.to_string_lossy())],
                ));
            }
            None => return Err(option_requires("--bpf-program", "xdp")),
        };
        let Some(output_option) = arguments.next() else {
            return Err(bpf_requires("--emit-object <object.o>"));
        };
        if output_option != "--emit-object" {
            return Err(expected_option(&output_option, "--emit-object"));
        }
        let output_path = arguments
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| UsageError::simple("cli.emitObjectNeedsPath"))?;
        if arguments.next().is_some() {
            return Err(unexpected_arguments("build"));
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
        return Err(unsupported_option(&option));
    }
    let output_path = arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| UsageError::simple("cli.outputNeedsExecutablePath"))?;
    if arguments.next().is_some() {
        return Err(unexpected_arguments("build"));
    }
    Ok(CommandLine::Build {
        source_path,
        output_path,
    })
}

fn required_manifest_path(
    argument: Option<OsString>,
    command: &str,
) -> Result<PathBuf, UsageError> {
    let path = argument.map(PathBuf::from).ok_or_else(|| {
        UsageError::new(
            "cli.manifestRequired",
            vec![LocalizedArgument::text("command", command)],
        )
    })?;
    if path.file_name() != Some(OsStr::new("bubble.toml")) {
        return Err(UsageError::new(
            "cli.manifestName",
            vec![LocalizedArgument::text("command", command)],
        ));
    }
    Ok(path)
}

fn parse_run_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, UsageError> {
    let Some(first) = arguments.next() else {
        return Err(UsageError::simple("cli.runNeedsInput"));
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

fn parse_lock_controls(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<LockMode, UsageError> {
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
                return Err(UsageError::new(
                    "cli.manifestOption",
                    vec![LocalizedArgument::text(
                        "option",
                        argument.to_string_lossy(),
                    )],
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
) -> Result<Vec<OsString>, UsageError> {
    let Some(separator) = arguments.next() else {
        return Ok(Vec::new());
    };
    if separator != "--" {
        return Err(UsageError::new(
            "cli.programArgumentsSeparator",
            vec![LocalizedArgument::text(
                "option",
                separator.to_string_lossy(),
            )],
        ));
    }
    Ok(arguments.collect())
}

fn required_source_path(argument: Option<OsString>, command: &str) -> Result<PathBuf, UsageError> {
    let path = argument
        .map(PathBuf::from)
        .ok_or_else(|| source_required(command))?;
    if path.extension() != Some(OsStr::new("pop")) {
        return Err(source_required(command));
    }
    Ok(path)
}

fn parse_dump_kind(kind: &OsStr) -> Result<DumpKind, UsageError> {
    match kind.to_str() {
        Some("hir") => Ok(DumpKind::Hir),
        Some("mir") => Ok(DumpKind::Mir),
        Some("ll") => Ok(DumpKind::Ll),
        _ => Err(UsageError::new(
            "cli.unsupportedDumpKind",
            vec![LocalizedArgument::text("value", kind.to_string_lossy())],
        )),
    }
}

fn command_requires(command: &str, option: &str) -> UsageError {
    UsageError::new(
        "cli.commandRequiresOption",
        vec![
            LocalizedArgument::text("command", command),
            LocalizedArgument::text("option", option),
        ],
    )
}

fn unsupported_option(option: &OsStr) -> UsageError {
    UsageError::new(
        "cli.unsupportedOption",
        vec![LocalizedArgument::text("option", option.to_string_lossy())],
    )
}

fn expected_option(option: &OsStr, expected: &str) -> UsageError {
    UsageError::new(
        "cli.expectedOption",
        vec![
            LocalizedArgument::text("option", option.to_string_lossy()),
            LocalizedArgument::text("expected", expected),
        ],
    )
}

fn option_requires(option: &str, value: &str) -> UsageError {
    UsageError::new(
        "cli.optionRequiresValue",
        vec![
            LocalizedArgument::text("option", option),
            LocalizedArgument::text("value", value),
        ],
    )
}

fn unexpected_arguments(command: &str) -> UsageError {
    UsageError::new(
        "cli.unexpectedArguments",
        vec![LocalizedArgument::text("command", command)],
    )
}

fn source_required(command: &str) -> UsageError {
    UsageError::new(
        "cli.sourceRequired",
        vec![LocalizedArgument::text("command", command)],
    )
}

fn bpf_requires(option: &str) -> UsageError {
    UsageError::new(
        "cli.bpfRequires",
        vec![LocalizedArgument::text("option", option)],
    )
}

fn write_help() -> ExitCode {
    if let Err(error) = writeln!(io::stdout().lock(), "{}", localized("cli.usage", &[])) {
        emit_localized(
            "cli.writeHelpFailed",
            &[LocalizedArgument::external("detail", error)],
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn scaffold_package(options: &ScaffoldOptions) -> ExitCode {
    match create_scaffold(options) {
        Ok((name, destination)) => {
            let message = localized(
                "cli.packageCreated",
                &[
                    LocalizedArgument::text("name", name),
                    LocalizedArgument::text("path", destination.display()),
                ],
            );
            if writeln!(io::stdout().lock(), "{message}").is_err() {
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            emit_localized(
                "cli.scaffoldFailed",
                &[LocalizedArgument::external("detail", error)],
            );
            ExitCode::from(2)
        }
    }
}

fn create_scaffold(options: &ScaffoldOptions) -> Result<(String, PathBuf), String> {
    let destination = absolute_scaffold_path(&options.path)?;
    let name = options
        .name
        .clone()
        .or_else(|| scaffold_directory_name(&destination))
        .ok_or_else(|| "a valid PascalCase Package name is required; use --name".to_owned())?;
    let (manifest, source, root_name) = scaffold_text(&name, options.kind)?;
    validate_scaffold(&manifest, &source, options.kind)?;

    match options.mode {
        ScaffoldMode::New => publish_new_scaffold(&destination, &manifest, &source, root_name)?,
        ScaffoldMode::Initialize => {
            publish_initialized_scaffold(&destination, &manifest, &source, root_name)?;
        }
    }
    Ok((name, destination))
}

fn absolute_scaffold_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|current| current.join(path))
        .map_err(|error| format!("could not resolve destination: {error}"))
}

fn scaffold_directory_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(OsStr::to_str)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

fn scaffold_text(name: &str, kind: ScaffoldKind) -> Result<(String, String, &'static str), String> {
    let manifest =
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n");
    parse_package_manifest(&manifest)
        .map_err(|_| format!("`{name}` is not a valid PascalCase Package identity"))?;
    Ok(match kind {
        ScaffoldKind::Binary => (
            manifest,
            format!("namespace {name}\n\nfunction main()\nend\n"),
            "main.pop",
        ),
        ScaffoldKind::Library => (manifest, format!("namespace {name}\n"), "lib.pop"),
    })
}

fn validate_scaffold(manifest: &str, source: &str, kind: ScaffoldKind) -> Result<(), String> {
    parse_package_manifest(manifest)
        .map_err(|error| format!("generated manifest is invalid: {error}"))?;
    let file = FileId::from_raw(0);
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(
        file,
        Arc::<str>::from("scaffold.pop"),
        Arc::<str>::from(source),
    )
    .map_err(|error| format!("generated source is invalid: {error}"))?;
    let input = FrontEndBubbleInput::new(
        BubbleId::from_raw(FIRST_PACKAGE_BUBBLE),
        NamespaceId::from_raw(FIRST_PACKAGE_BUBBLE),
        Vec::new(),
        vec![FrontEndModule::new(module, source)],
    );
    let input = if kind == ScaffoldKind::Binary {
        input.with_implicit_main_entry(module)
    } else {
        input
    };
    let result = analyze_bubble(input);
    if let Some(diagnostic) = result.diagnostics().first() {
        return Err(format!(
            "generated source failed compiler validation with {}",
            diagnostic.code()
        ));
    }
    Ok(())
}

fn publish_new_scaffold(
    destination: &Path,
    manifest: &str,
    source: &str,
    root_name: &str,
) -> Result<(), String> {
    if fs::symlink_metadata(destination).is_ok() {
        return Err(format!(
            "destination `{}` already exists",
            destination.display()
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "destination has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("could not create parent: {error}"))?;
    let leaf = destination
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| "destination must have a UTF-8 directory name".to_owned())?;
    let staging = parent.join(format!(".{leaf}.pop-new-{}", std::process::id()));
    if fs::symlink_metadata(&staging).is_ok() {
        return Err("scaffolding staging path already exists".to_owned());
    }
    if let Err(error) = write_scaffold(&staging, manifest, source, root_name) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    fs::rename(&staging, destination).map_err(|error| {
        let _ = fs::remove_dir_all(&staging);
        format!("could not publish Package atomically: {error}")
    })
}

fn publish_initialized_scaffold(
    destination: &Path,
    manifest: &str,
    source: &str,
    root_name: &str,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(destination)
        .map_err(|error| format!("initialization directory is unavailable: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("initialization destination must be a real directory".to_owned());
    }
    for protected in ["bubble.toml", "src/lib.pop", "src/main.pop"] {
        if fs::symlink_metadata(destination.join(protected)).is_ok() {
            return Err(format!("refusing to overwrite `{protected}`"));
        }
    }
    let existing_source = destination.join("src");
    if let Ok(metadata) = fs::symlink_metadata(&existing_source)
        && (metadata.file_type().is_symlink() || !metadata.is_dir())
    {
        return Err("existing `src` must be a real directory".to_owned());
    }
    let staging = destination.join(format!(".pop-initialize-{}", std::process::id()));
    if fs::symlink_metadata(&staging).is_ok() {
        return Err("scaffolding staging path already exists".to_owned());
    }
    write_scaffold(&staging, manifest, source, root_name)?;
    let staged_manifest = staging.join("bubble.toml");
    let staged_source = staging.join("src");
    let manifest_path = destination.join("bubble.toml");
    fs::rename(&staged_manifest, &manifest_path)
        .map_err(|error| format!("could not publish manifest: {error}"))?;
    let publish_source = if existing_source.is_dir() {
        fs::rename(
            staged_source.join(root_name),
            existing_source.join(root_name),
        )
        .map(|()| {
            let _ = fs::remove_dir(&staged_source);
        })
    } else {
        fs::rename(&staged_source, &existing_source)
    };
    if let Err(error) = publish_source {
        let _ = fs::remove_file(&manifest_path);
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("could not publish source directory: {error}"));
    }
    let _ = fs::remove_dir_all(&staging);
    Ok(())
}

fn write_scaffold(
    root: &Path,
    manifest: &str,
    source: &str,
    root_name: &str,
) -> Result<(), String> {
    fs::create_dir_all(root.join("src"))
        .map_err(|error| format!("could not create scaffold: {error}"))?;
    fs::write(root.join("bubble.toml"), manifest)
        .map_err(|error| format!("could not write manifest: {error}"))?;
    fs::write(root.join("src").join(root_name), source)
        .map_err(|error| format!("could not write source: {error}"))
}

fn check_source(source_path: &PathBuf, dumps: &[DumpKind]) -> ExitCode {
    let source_text = match fs::read_to_string(source_path) {
        Ok(source) => source,
        Err(error) => {
            emit_localized(
                "cli.readFailed",
                &[
                    LocalizedArgument::text("path", source_path.display()),
                    LocalizedArgument::external("detail", error),
                ],
            );
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
            emit_localized(
                "cli.loadFailed",
                &[
                    LocalizedArgument::text("path", source_path.display()),
                    LocalizedArgument::external("detail", error),
                ],
            );
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
        return write_diagnostics(result.diagnostics());
    }
    let Some(hir) = result.hir() else {
        tool_failure!("pop: internal compiler error: successful analysis did not publish HIR");
        return ExitCode::from(101);
    };
    let mir = match lower_hir_bubble_with_fingerprint(hir, result.types(), artifact_sha256_hex) {
        Ok(mir) => mir,
        Err(errors) => {
            tool_failure!("pop: internal compiler error: canonical MIR verification failed");
            for error in errors {
                tool_failure!("  {error:?}");
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
                tool_failure!("pop: internal compiler error: LLVM lowering failed: {error}");
                return ExitCode::from(101);
            }
        };
        if let Err(error) = module.verify() {
            tool_failure!("pop: internal compiler error: {error}");
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

fn write_diagnostics(diagnostics: &[Diagnostic]) -> ExitCode {
    let mut output = String::new();
    for diagnostic in diagnostics {
        match rendering().diagnostic(diagnostic) {
            Ok(rendered) => output.push_str(&rendered),
            Err(error) => {
                emit_localized(
                    "cli.renderDiagnosticsFailed",
                    &[LocalizedArgument::external("detail", error)],
                );
                return ExitCode::from(101);
            }
        }
    }
    if let Err(error) = io::stderr().lock().write_all(output.as_bytes()) {
        emit_localized(
            "cli.writeDiagnosticsFailed",
            &[LocalizedArgument::external("detail", error)],
        );
    }
    ExitCode::FAILURE
}

fn write_output(output: &str) -> ExitCode {
    if let Err(error) = io::stdout().lock().write_all(output.as_bytes()) {
        emit_localized(
            "cli.writeOutputFailed",
            &[LocalizedArgument::external("detail", error)],
        );
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
            tool_failure!("pop: LLVM lowering failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let object_path = std::env::temp_dir().join(format!("pop-native-{}.o", std::process::id()));
    let standard_object_path =
        std::env::temp_dir().join(format!("pop-standard-{}.o", std::process::id()));
    if let Err(error) = module.emit_object(&object_path) {
        tool_failure!("pop: {error}");
        return ExitCode::FAILURE;
    }
    if emit_native_object(&standard.program, &standard_object_path).is_none() {
        let _ = fs::remove_file(&object_path);
        return ExitCode::FAILURE;
    }
    let result = link_native_executable(
        &[object_path.clone(), standard_object_path.clone()],
        &[],
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
            tool_failure!("pop: {error}: `{target_triple}`");
            return ExitCode::FAILURE;
        }
    };
    let Some(program_mir) = lower_native_source(source_path) else {
        return ExitCode::FAILURE;
    };
    let Some(entry) = program_mir.entry else {
        tool_failure!("pop: BPF build requires an explicit entry point");
        return ExitCode::FAILURE;
    };
    let options = match program {
        BpfProgramKind::Xdp => BpfLoweringOptions::xdp(entry).with_runtime_profile(runtime_profile),
    };
    let module =
        match lower_mir_to_bpf_module(&program_mir.mir, &program_mir.types, &target, options) {
            Ok(module) => module,
            Err(error) => {
                tool_failure!("pop: {}: {error}", error.diagnostic_code());
                return ExitCode::FAILURE;
            }
        };
    if let Err(error) = module.emit_object(output_path) {
        tool_failure!("pop: {}: {error}", error.diagnostic_code());
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
            tool_failure!("pop: C lowering failed: {error}");
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
            tool_failure!("pop: could not execute native program: {error}");
            ExitCode::FAILURE
        }
    }
}

struct LoweredPackage {
    root: PathBuf,
    bubbles: Vec<LoweredPackageBubble>,
    native_link_sources: Vec<NativeLinkPlanSource>,
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
    native_link_plan: pop_projects::NativeLinkPlan,
    program: NativeProgram,
}

fn lower_package(manifest_path: &Path) -> Option<LoweredPackage> {
    let manifest_path = fs::canonicalize(manifest_path)
        .map_err(|error| {
            tool_failure!(
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
        native_link_sources: Vec::new(),
        standard,
    };
    lower_package_recursive(&manifest_path, true, &mut state)?;
    Some(LoweredPackage {
        root: package_root.to_path_buf(),
        bubbles: state.bubbles,
        native_link_sources: state.native_link_sources,
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
    retained_adapters_popc: Option<Vec<u8>>,
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
    native_link_sources: Vec<NativeLinkPlanSource>,
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
        tool_failure!(
            "pop: Package dependency cycle includes `{}`",
            manifest_path.display()
        );
        return None;
    }
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let manifest_text = fs::read_to_string(manifest_path)
        .map_err(|error| {
            tool_failure!("pop: could not read `{}`: {error}", manifest_path.display());
        })
        .ok()?;
    let manifest = parse_package_manifest(&manifest_text)
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()?;
    if matches!(
        manifest.name(),
        INTERNAL_PACKAGE_NAME | STANDARD_PACKAGE_NAME
    ) {
        tool_failure!(
            "pop: Package `{}` attempts to replace a reserved foundation identity",
            manifest.name()
        );
        return None;
    }
    let verified_ffi_bindings =
        verify_ffi_generated_bindings(package_root, &manifest, native_target().triple())
            .map_err(|error| tool_failure!("pop: {error}"))
            .ok()?;
    let native_link_plan = manifest
        .native_link_plan(native_target().triple())
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()?;
    state.native_link_sources.push(NativeLinkPlanSource::new(
        package_root,
        native_link_plan.clone(),
    ));

    let mut external_libraries = vec![state.standard.clone()];
    for requirement in manifest.dependencies() {
        let dependency_manifest = match requirement.source() {
            DependencySource::LocalPath(path) => package_root.join(path).join("bubble.toml"),
            DependencySource::Registry => {
                tool_failure!(
                    "pop: registry dependency `{}` requires a resolved bubble.lock",
                    requirement.alias()
                );
                return None;
            }
            DependencySource::ExactGit { .. } => {
                tool_failure!(
                    "pop: exact-Git dependency `{}` requires a resolved bubble.lock",
                    requirement.alias()
                );
                return None;
            }
            DependencySource::Workspace => {
                tool_failure!(
                    "pop: workspace-inherited dependency `{}` requires a Workspace root",
                    requirement.alias()
                );
                return None;
            }
        };
        let dependency_manifest = fs::canonicalize(&dependency_manifest)
            .map_err(|error| {
                tool_failure!(
                    "pop: could not resolve dependency `{}` at `{}`: {error}",
                    requirement.alias(),
                    dependency_manifest.display()
                );
            })
            .ok()?;
        let Some(library) = lower_package_recursive(&dependency_manifest, false, state)? else {
            tool_failure!(
                "pop: dependency `{}` has no public library Bubble",
                requirement.alias()
            );
            return None;
        };
        if requirement
            .version_requirement()
            .is_some_and(|required| required != library.version)
        {
            tool_failure!(
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
            tool_failure!(
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
    let external_retained_adapters = external_libraries
        .iter()
        .filter_map(|library| {
            library
                .retained_adapters_popc
                .clone()
                .map(|bytes| (library.metadata.bubble(), bytes))
        })
        .collect::<Vec<_>>();
    let artifact_dependencies = external_libraries
        .iter()
        .map(ResolvedPackageLibrary::artifact_dependency)
        .collect::<Vec<_>>();
    let ffi_dependency = external_libraries
        .iter()
        .find(|library| library.package == FFI_PACKAGE_NAME)
        .map(|library| library.metadata.bubble());
    let relative_paths: Vec<_> = source_paths.keys().map(String::as_str).collect();
    let bubbles = discover_conventional_bubbles(&manifest, &relative_paths)
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()?;
    if verified_ffi_bindings.iter().any(|bindings| {
        !bubbles.iter().any(|bubble| {
            bubble
                .modules()
                .iter()
                .any(|module| module == bindings.source_path())
        })
    }) {
        tool_failure!("pop: generated FFI callback metadata does not name a discovered Module");
        return None;
    }
    let selected: Vec<_> = bubbles
        .iter()
        .filter(|bubble| {
            bubble.kind() == BubbleKind::Library
                || root_package && bubble.kind() == BubbleKind::Binary
        })
        .collect();
    if selected.is_empty() {
        tool_failure!("pop: Package has no selected library or binary Bubbles");
        return None;
    }

    let mut library: Option<ResolvedPackageLibrary> = None;
    for bubble in selected {
        let bubble_id = BubbleId::from_raw(state.next_bubble);
        state.next_bubble = state.next_bubble.checked_add(1)?;
        let modules = bubble
            .modules()
            .iter()
            .map(|relative| {
                let source = source_paths.get(relative).cloned().ok_or_else(|| {
                    tool_failure!("pop: discovered Module `{relative}` is missing");
                })?;
                Ok((PathBuf::from(relative), source))
            })
            .collect::<Result<Vec<_>, ()>>()
            .ok()?;
        let mut dependency_metadata = external_metadata.clone();
        let mut dependency_retained_adapters = external_retained_adapters.clone();
        if bubble.depends_on_library() {
            let library = library
                .as_ref()
                .expect("sorted conventional discovery lowers the library first");
            dependency_metadata.push(library.metadata.clone());
            if let Some(bytes) = &library.retained_adapters_popc {
                dependency_retained_adapters.push((library.metadata.bubble(), bytes.clone()));
            }
        }
        let program = lower_native_bubble(
            bubble_id,
            &modules,
            bubble.kind() == BubbleKind::Binary,
            dependency_metadata,
            dependency_retained_adapters,
            Vec::new(),
            ffi_dependency,
            verified_ffi_bindings
                .iter()
                .filter(|bindings| {
                    bubble
                        .modules()
                        .iter()
                        .any(|module| module == bindings.source_path())
                })
                .cloned()
                .collect(),
        )?;
        validate_foreign_link_aliases(&program.mir, &native_link_plan)
            .map_err(|error| tool_failure!("pop: {error}"))
            .ok()?;
        if bubble.kind() == BubbleKind::Library {
            let reference = encode_reference_metadata(&program.reference_metadata)
                .map_err(|error| tool_failure!("pop: reference metadata encoding failed: {error}"))
                .ok()?;
            library = Some(ResolvedPackageLibrary {
                package: manifest.name().to_owned(),
                version: manifest.version().to_owned(),
                source_sha256: source_sha256.clone(),
                bubble: bubble.name().to_owned(),
                public_api_sha256: sha256_hex(&reference),
                metadata: program.reference_metadata.clone(),
                retained_adapters_popc: program.retained_adapters_popc.clone(),
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
            native_link_plan: native_link_plan.clone(),
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
    let target = native_target();
    for manifest in &selection.packages {
        let Some(package) = lower_package(manifest) else {
            return ExitCode::FAILURE;
        };
        if let Err(error) = resolve_native_link_inputs(&package.native_link_sources, &target) {
            tool_failure!("pop: {error}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
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
                    tool_failure!("pop: documentation output failed: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let directory = output_root.join(&bubble.name);
            if let Err(error) = fs::create_dir_all(&directory) {
                tool_failure!(
                    "pop: could not create documentation output `{}`: {error}",
                    directory.display()
                );
                return ExitCode::FAILURE;
            }
            let output = directory.join("documentation.xml");
            if let Err(error) = fs::write(&output, xml) {
                tool_failure!(
                    "pop: could not write documentation output `{}`: {error}",
                    output.display()
                );
                return ExitCode::FAILURE;
            }
            emitted += 1;
        }
    }
    if emitted == 0 {
        tool_failure!("pop: `pop documentation` requires a selected library Bubble");
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
        ReferenceType::Record(identity) => format!(
            "record:b{}:s{}",
            identity.bubble().raw(),
            identity.symbol().raw()
        ),
        ReferenceType::Class(nominal) | ReferenceType::Interface(nominal) => {
            let arguments = nominal
                .arguments()
                .iter()
                .map(|argument| reference_type_text(argument, type_parameters))
                .collect::<Vec<_>>()
                .join(",");
            let kind = if matches!(reference, ReferenceType::Class(_)) {
                "class"
            } else {
                "interface"
            };
            format!(
                "{kind}:b{}:s{}<{arguments}>",
                nominal.definition().bubble().raw(),
                nominal.definition().symbol().raw()
            )
        }
        ReferenceType::Tuple(elements) => format!(
            "({})",
            elements
                .iter()
                .map(|element| reference_type_text(element, type_parameters))
                .collect::<Vec<_>>()
                .join(",")
        ),
        ReferenceType::Function {
            is_async,
            parameters,
            results,
            ..
        } => format!(
            "{}function({})->({})",
            if *is_async { "async " } else { "" },
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
    let selected_target = native_target();
    let native_link_resolution =
        resolve_native_link_inputs(&package.native_link_sources, &selected_target)
            .map_err(|error| tool_failure!("pop: {error}"))
            .ok()?;

    let output_root = selected_output_root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| package.root.join("target/debug"));
    let dependency_root = output_root.join("deps");
    fs::create_dir_all(&dependency_root)
        .map_err(|error| tool_failure!("pop: could not create build output: {error}"))
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
                .map_err(|error| tool_failure!("pop: documentation output failed: {error}"))
                .ok()?;
            let implementation = fs::read(&emission_object)
                .map_err(|error| tool_failure!("pop: could not read native object: {error}"))
                .ok()?;
            let target = native_target();
            let provider_aliases = bubble
                .native_link_plan
                .libraries()
                .iter()
                .map(pop_projects::NativeLibrary::alias)
                .collect::<BTreeSet<_>>();
            let resolved_native_providers = native_link_resolution
                .providers()
                .iter()
                .filter(|provider| provider_aliases.contains(provider.alias()))
                .cloned()
                .collect();
            let mut emission = PoplibEmission::new(
                &bubble.package,
                &bubble.version,
                &bubble.source_sha256,
                &bubble.name,
                bubble.kind,
                &bubble.edition,
                bubble.program.reference_metadata.clone(),
            )
            .with_dependencies(bubble.dependencies.clone())
            .with_native_link_plan(bubble.native_link_plan.clone())
            .with_resolved_native_providers(resolved_native_providers)
            .with_documentation(documentation.into_bytes())
            .with_target_implementation(target.triple(), implementation);
            if let Some(descriptor) = &bubble.program.retained_adapters_popc {
                emission = emission.with_retained_adapters_popc(descriptor.clone());
            }
            let artifact = dependency_root.join(format!("{}.poplib", bubble.name));
            emit_poplib(&artifact, &emission)
                .map_err(|error| tool_failure!("pop: library artifact emission failed: {error}"))
                .ok()?;
            let loaded = load_poplib(&artifact)
                .map_err(|error| {
                    tool_failure!("pop: emitted library verification failed: {error:?}");
                })
                .ok()?;
            let (selected_target, selected_implementation) =
                loaded.target_implementation().or_else(|| {
                    tool_failure!("pop: library artifact has no target implementation");
                    None
                })?;
            if selected_target != target.triple() {
                tool_failure!(
                    "pop: library target mismatch: expected {}, found {selected_target}",
                    target.triple()
                );
                return None;
            }
            fs::write(&object, selected_implementation)
                .map_err(|error| tool_failure!("pop: could not select library object: {error}"))
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
        if link_native_executable(&objects, native_link_resolution.inputs(), &executable)
            != ExitCode::SUCCESS
        {
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
        tool_failure!("pop: `pop run` requires exactly one discovered binary Bubble");
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
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()?;
    let lock_path = root.join("bubble.lock");
    let existing = match fs::read(&lock_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            tool_failure!("pop: could not read `{}`: {error}", lock_path.display());
            return None;
        }
    };
    let changed = apply_lock_policy(existing.as_deref(), &proposed, mode, false)
        .map_err(|error| tool_failure!("pop: {error}"))
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
        .map_err(|error| tool_failure!("pop: {error}"))
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
        tool_failure!(
            "pop: Package dependency cycle includes `{}`",
            manifest_path.display()
        );
        return None;
    }
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|error| {
            tool_failure!("pop: could not read `{}`: {error}", manifest_path.display());
        })
        .ok()?;
    let manifest = parse_package_manifest(&manifest_text)
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()?;

    let mut external_libraries = Vec::new();
    for requirement in manifest.dependencies() {
        let dependency_manifest = match requirement.source() {
            DependencySource::LocalPath(path) => package_root.join(path).join("bubble.toml"),
            DependencySource::Registry => {
                tool_failure!(
                    "pop: registry dependency `{}` is not available without registry resolution",
                    requirement.alias()
                );
                return None;
            }
            DependencySource::ExactGit { .. } => {
                tool_failure!(
                    "pop: exact-Git dependency `{}` is not available without Git resolution",
                    requirement.alias()
                );
                return None;
            }
            DependencySource::Workspace => {
                tool_failure!(
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
            tool_failure!("pop: dependency version mismatch for `{}`", dependency.name);
            return None;
        }
        let library = dependency.library.clone().or_else(|| {
            tool_failure!(
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
        .map_err(|error| tool_failure!("pop: {error}"))
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
                .map_err(|error| tool_failure!("pop: {error}"))
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
        .map_err(|error| tool_failure!("pop: {error}"))
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
        .map_err(|error| tool_failure!("pop: could not write `{}`: {error}", temporary.display()))
        .ok()?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        tool_failure!(
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
            tool_failure!(
                "pop: could not resolve `{}`: {error}",
                manifest_path.display()
            );
        })
        .ok()?;
    let text = fs::read_to_string(&manifest_path)
        .map_err(|error| {
            tool_failure!("pop: could not read `{}`: {error}", manifest_path.display());
        })
        .ok()?;
    if !text.lines().any(|line| line.trim() == "[workspace]") {
        return Some(ManifestSelection {
            workspace_root: None,
            packages: vec![manifest_path],
        });
    }

    let workspace = parse_workspace_manifest(&text)
        .map_err(|error| tool_failure!("pop: {error}"))
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
        .map_err(|error| tool_failure!("pop: {error}"))
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
                    tool_failure!(
                        "pop: could not inspect Workspace member root `{}`: {error}",
                        directory.display()
                    );
                })
                .ok()?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| tool_failure!("pop: could not inspect Workspace members: {error}"))
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
            tool_failure!("pop: could not execute native program: {error}");
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
        .map_err(|error| {
            tool_failure!("pop: could not inspect `{}`: {error}", directory.display());
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            tool_failure!("pop: could not inspect `{}`: {error}", directory.display());
        })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|error| tool_failure!("pop: could not inspect source entry: {error}"))?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_sources_in(package_root, &path, sources)?;
        } else if file_type.is_file() && path.extension() == Some(OsStr::new("pop")) {
            let relative = path
                .strip_prefix(package_root)
                .map_err(|_| tool_failure!("pop: Package source escaped its root"))?
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
        .map_err(|error| tool_failure!("pop: LLVM lowering failed: {error}"))
        .ok()?;
    module
        .emit_object(output_path)
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()
}

struct NativeProgram {
    mir: pop_mir::MirBubble,
    types: pop_types::TypeArena,
    entry: Option<SymbolId>,
    reference_metadata: ReferenceMetadata,
    retained_adapters_popc: Option<Vec<u8>>,
    checked_documentation: Vec<CheckedDocumentation>,
}

fn lower_native_source(source_path: &Path) -> Option<NativeProgram> {
    let (standard, _) = lower_toolchain_standard()?;
    let retained_adapters = standard
        .retained_adapters_popc
        .map(|bytes| vec![(STANDARD_BUBBLE, bytes)])
        .unwrap_or_default();
    lower_native_bubble(
        BubbleId::from_raw(FIRST_PACKAGE_BUBBLE),
        &[(source_path.to_path_buf(), source_path.to_path_buf())],
        true,
        vec![standard.metadata],
        retained_adapters,
        Vec::new(),
        None,
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
        .map_err(|error| tool_failure!("pop: could not read reserved Standard manifest: {error}"))
        .ok()?;
    let manifest = parse_package_manifest(&manifest_text)
        .map_err(|error| tool_failure!("pop: invalid reserved Standard manifest: {error}"))
        .ok()?;
    if manifest.name() != STANDARD_PACKAGE_NAME {
        tool_failure!("pop: reserved Standard manifest has the wrong identity");
        return None;
    }
    let source_paths = collect_package_sources(&package_root).ok()?;
    let relative_paths = source_paths.keys().map(String::as_str).collect::<Vec<_>>();
    let discovered = discover_conventional_bubbles(&manifest, &relative_paths)
        .map_err(|error| tool_failure!("pop: could not discover reserved Standard: {error}"))
        .ok()?;
    let [bubble] = discovered.as_slice() else {
        tool_failure!("pop: reserved Standard must contain exactly one library Bubble");
        return None;
    };
    if bubble.kind() != BubbleKind::Library {
        tool_failure!("pop: reserved Standard must be a library Bubble");
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
        Vec::new(),
        vec![INTERNAL_BUBBLE],
        None,
        Vec::new(),
    )?;
    let reference = encode_reference_metadata(&program.reference_metadata)
        .map_err(|error| tool_failure!("pop: Standard metadata encoding failed: {error}"))
        .ok()?;
    let source_sha256 = package_content_hash(&manifest_path, &source_paths)?;
    let library = ResolvedPackageLibrary {
        package: manifest.name().to_owned(),
        version: manifest.version().to_owned(),
        source_sha256: source_sha256.clone(),
        bubble: bubble.name().to_owned(),
        public_api_sha256: sha256_hex(&reference),
        metadata: program.reference_metadata.clone(),
        retained_adapters_popc: program.retained_adapters_popc.clone(),
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
        native_link_plan: manifest
            .native_link_plan(native_target().triple())
            .map_err(|error| tool_failure!("pop: invalid Standard native link plan: {error}"))
            .ok()?,
        program,
    };
    Some((library, lowered))
}

fn lower_native_bubble(
    bubble: BubbleId,
    modules: &[(PathBuf, PathBuf)],
    requires_entry: bool,
    dependency_metadata: Vec<ReferenceMetadata>,
    dependency_retained_adapters_popc: Vec<(BubbleId, Vec<u8>)>,
    additional_dependencies: Vec<BubbleId>,
    ffi_dependency: Option<BubbleId>,
    verified_ffi_bindings: Vec<VerifiedFfiGeneratedBindings>,
) -> Option<NativeProgram> {
    let modules = modules
        .iter()
        .enumerate()
        .map(|(index, (display_path, source_path))| {
            let source_text = fs::read_to_string(source_path).map_err(|error| {
                emit_localized(
                    "cli.readFailed",
                    &[
                        LocalizedArgument::text("path", source_path.display()),
                        LocalizedArgument::external("detail", error),
                    ],
                );
            })?;
            let file = u32::try_from(index).map_err(|_| {
                tool_failure!("pop: too many Modules in one Bubble");
            })?;
            let source = SourceFile::new(
                FileId::from_raw(file),
                display_path.to_string_lossy().into_owned(),
                source_text,
            )
            .map_err(|error| {
                emit_localized(
                    "cli.loadFailed",
                    &[
                        LocalizedArgument::text("path", source_path.display()),
                        LocalizedArgument::external("detail", error),
                    ],
                );
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
    .with_reference_metadata(dependency_metadata)
    .with_reference_retained_adapters_popc(dependency_retained_adapters_popc);
    let input = if let Some(ffi_dependency) = ffi_dependency {
        input.with_ffi_dependency(ffi_dependency)
    } else {
        input
    };
    let input = input.with_verified_ffi_generated_bindings(verified_ffi_bindings);
    let input = if requires_entry {
        input.with_implicit_main_entry(ModuleId::from_raw(0))
    } else {
        input
    };
    let result = analyze_bubble(input);
    if !result.diagnostics().is_empty() {
        let _ = write_diagnostics(result.diagnostics());
        return None;
    }
    let hir = result.hir()?;
    let entry = if requires_entry {
        Some(select_native_entry(hir, result.types())?)
    } else {
        None
    };
    let mir = lower_hir_bubble_with_fingerprint(hir, result.types(), artifact_sha256_hex)
        .map_err(|errors| {
            tool_failure!(
                "pop: internal compiler error: canonical MIR verification failed: {errors:?}"
            );
        })
        .ok()?;
    let mir = optimize_mir(mir, result.types())
        .map_err(|errors| {
            tool_failure!(
                "pop: internal compiler error: optimized MIR verification failed: {errors:?}"
            );
        })
        .ok()?;
    let reference_metadata = result
        .reference_metadata()
        .map_err(|error| tool_failure!("pop: public reference metadata emission failed: {error:?}"))
        .ok()?
        .clone();
    let retained_adapters_popc = result
        .retained_metadata()
        .map_err(|error| {
            tool_failure!("pop: retained metadata emission failed: {error:?}");
        })
        .ok()?
        .public_popc()
        .map_err(|error| {
            tool_failure!("pop: retained metadata filtering failed: {error:?}");
        })
        .ok()
        .filter(|descriptor| !descriptor.is_empty());
    let checked_documentation = result.checked_documentation().to_vec();
    Some(NativeProgram {
        mir,
        types: result.types().clone(),
        entry,
        reference_metadata,
        retained_adapters_popc,
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
    tool_failure!(
        "pop: binary entry must be private or implicit `main` with no parameters or `Array<String>`, and with no result or `Int`"
    );
}

fn link_native_executable(
    object_paths: &[PathBuf],
    native_inputs: &[NativeLinkInput],
    output_path: &Path,
) -> ExitCode {
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
            tool_failure!("pop: could not build native foundation archives");
            return ExitCode::FAILURE;
        }
    }

    if !standard.is_file() || !runtime.is_file() {
        tool_failure!("pop: native foundation archives were not produced");
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
    for input in native_inputs {
        input.append_to(&mut command);
    }

    let link = command.arg("-o").arg(output_path).output();

    match link {
        Ok(output) if output.status.success() => ExitCode::SUCCESS,
        Ok(output) => {
            tool_failure!(
                "pop: native link failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            ExitCode::FAILURE
        }
        Err(error) => {
            tool_failure!("pop: could not invoke native linker: {error}");
            ExitCode::FAILURE
        }
    }
}
