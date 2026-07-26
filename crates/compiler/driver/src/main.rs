//! Unified `pop` command and build orchestration.

#![allow(
    clippy::map_unwrap_or,
    clippy::option_option,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines
)]

mod presentation;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::{Arc, OnceLock};

use pop_backend_api::RuntimeProfile;
use pop_backend_c::{CLoweringOptions, lower_mir_to_c};
use pop_backend_llvm::{
    BpfLoweringOptions, BpfProgramKind, LlvmLoweringOptions, lower_mir_to_bpf_module,
    lower_mir_to_llvm_ir,
};
use pop_diagnostics::{
    DiagnosticPolicy, DiagnosticReport, DiagnosticSelector, DocumentSnapshot, FixAllSummary,
    WorkspaceSnapshot, apply_safe_fix_all, catalog as diagnostic_catalog, latest_warning_wave,
};
use pop_documentation_generator::{DocumentationMember, render_xml};
use pop_driver::{
    CheckedDocumentation, FrontEndBubbleInput, FrontEndModule, NativeLinkInput,
    NativeLinkPlanSource, PoplibDependency, PoplibEmission, ReferenceFunction, ReferenceMetadata,
    ReferenceType, VerifiedFfiGeneratedBindings, analyze_bubble, artifact_sha256_hex, emit_poplib,
    encode_reference_metadata, generate_ffi_bindings, load_poplib, resolve_native_link_inputs,
    validate_foreign_link_aliases, verify_ffi_generated_bindings,
};
use pop_formatter::format_documentation_comments;
use pop_foundation::{
    BubbleId, Diagnostic, DiagnosticArgument, DiagnosticCategory, DiagnosticOriginKind,
    DiagnosticSeverity, FileId, FixApplicability, ModuleId, NamespaceId, SourceSpan, SymbolId,
};
use pop_localization::{
    Argument as LocalizedArgument, DiagnosticSource, Language, RenderContext,
    select_process_language,
};
use pop_mir::{lower_hir_bubble_with_fingerprint, optimize_mir};
use pop_projects::{
    BubbleKind, BubbleLock, DependencyRequirement, DependencySource, LockMode, LockedBubble,
    LockedBubbleIdentity, LockedPackage, LockedSource, WorkspaceManifest, apply_lock_policy,
    discover_conventional_bubbles, discover_workspace_members, encode_lock, parse_package_manifest,
    parse_workspace_manifest, sha256_hex,
};
use pop_resolve::Visibility;
use pop_source::SourceFile;
use pop_target::TargetSpec;
use pop_types::SemanticType;
use presentation::{
    ColorChoice, CommandFeedback, MessageFormat, Request as PresentationRequest, Tone,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const INTERNAL_BUBBLE: BubbleId = BubbleId::from_raw(1);
const STANDARD_BUBBLE: BubbleId = BubbleId::from_raw(2);
const FIRST_PACKAGE_BUBBLE: u32 = 3;
const INTERNAL_PACKAGE_NAME: &str = "Pop.Internal";
const STANDARD_PACKAGE_NAME: &str = "Pop.Standard";
const FFI_PACKAGE_NAME: &str = "Pop.Ffi";
const NATIVE_PLATFORM_TARGET: &str = "x86_64-unknown-linux-gnu";

static CLI_RENDERING: OnceLock<RenderContext> = OnceLock::new();
static CLI_DIAGNOSTICS: OnceLock<DiagnosticOptions> = OnceLock::new();

#[derive(Clone, Debug, Eq, PartialEq)]
struct DiagnosticOptions {
    policy: DiagnosticPolicy,
    maximum_errors: NonZeroUsize,
}

impl Default for DiagnosticOptions {
    fn default() -> Self {
        Self {
            policy: DiagnosticPolicy::new(1),
            maximum_errors: NonZeroUsize::new(100).expect("default error limit is non-zero"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ManifestControls {
    lock_mode: LockMode,
    features: Vec<String>,
    platform_target: String,
    registry_root: Option<PathBuf>,
}

impl ManifestControls {
    const fn offline(&self) -> bool {
        matches!(self.lock_mode, LockMode::Offline | LockMode::Frozen)
    }
}

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
    FixSource {
        source_path: PathBuf,
    },
    PackageFix {
        manifest_path: PathBuf,
    },
    PackageCheck {
        manifest_path: PathBuf,
        controls: ManifestControls,
    },
    Lint {
        manifest_path: PathBuf,
        controls: ManifestControls,
    },
    Format {
        manifest_path: PathBuf,
        check: bool,
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
        controls: ManifestControls,
        selection: PackageBuildSelection,
    },
    Documentation {
        manifest_path: PathBuf,
        controls: ManifestControls,
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
        controls: ManifestControls,
        arguments: Vec<OsString>,
    },
    Test {
        manifest_path: PathBuf,
        controls: ManifestControls,
    },
    Benchmark {
        manifest_path: PathBuf,
        controls: ManifestControls,
    },
    FfiGenerate {
        alias: String,
        manifest_path: PathBuf,
        platform_target: String,
    },
}

impl CommandLine {
    const fn feedback_identity(&self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Help => ("help", "ui.phase.loading", "loading"),
            Self::Scaffold(options) => match options.mode {
                ScaffoldMode::New => ("new", "ui.phase.creating", "creating"),
                ScaffoldMode::Initialize => ("initialize", "ui.phase.creating", "creating"),
            },
            Self::Check { .. } | Self::PackageCheck { .. } => {
                ("check", "ui.phase.checking", "checking")
            }
            Self::Lint { .. } => ("lint", "ui.phase.checking", "checking"),
            Self::Format { .. } => ("format", "ui.phase.checking", "checking"),
            Self::FixSource { .. } | Self::PackageFix { .. } => {
                ("fix", "ui.phase.fixing", "fixing")
            }
            Self::Build { .. } | Self::BuildBpf { .. } | Self::PackageBuild { .. } => {
                ("build", "ui.phase.building", "building")
            }
            Self::Documentation { .. } => ("documentation", "ui.phase.documenting", "documenting"),
            Self::TranspileToC { .. } => ("transpile", "ui.phase.transpiling", "transpiling"),
            Self::Run { .. } | Self::PackageRun { .. } => ("run", "ui.phase.running", "running"),
            Self::Test { .. } => ("test", "ui.phase.running", "running"),
            Self::Benchmark { .. } => ("benchmark", "ui.phase.running", "running"),
            Self::FfiGenerate { .. } => ("ffi generate", "ui.phase.generating", "generating"),
        }
    }
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
enum PackageBuildSelection {
    Ordinary,
    Example(String),
    Test(String),
    Benchmark(String),
}

impl PackageBuildSelection {
    const fn root_selection(&self) -> RootBubbleSelection {
        match self {
            Self::Ordinary => RootBubbleSelection::Ordinary,
            Self::Example(_) => RootBubbleSelection::Examples,
            Self::Test(_) => RootBubbleSelection::Tests,
            Self::Benchmark(_) => RootBubbleSelection::Benchmarks,
        }
    }

    fn bubble_name(&self) -> Option<&str> {
        match self {
            Self::Ordinary => None,
            Self::Example(name) | Self::Test(name) | Self::Benchmark(name) => Some(name),
        }
    }

    fn cache_identity(&self) -> String {
        match self {
            Self::Ordinary => "ordinary".to_owned(),
            Self::Example(name) => format!("example:{name}"),
            Self::Test(name) => format!("test:{name}"),
            Self::Benchmark(name) => format!("benchmark:{name}"),
        }
    }

    fn cache_record_label(&self) -> String {
        match self {
            Self::Ordinary => "development".to_owned(),
            Self::Example(name) => format!("development-example-{name}"),
            Self::Test(name) => format!("development-test-{name}"),
            Self::Benchmark(name) => format!("development-benchmark-{name}"),
        }
    }
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
    let (presentation_request, diagnostic_options, arguments) =
        match extract_presentation_options(arguments) {
            Ok(selection) => selection,
            Err(error) => {
                presentation::initialize(PresentationRequest::default());
                let message = format!("pop: {}\n\n{}", error.render(), localized("cli.usage", &[]));
                let _ = presentation::write_stderr_line(&message, Tone::Error);
                return ExitCode::from(2);
            }
        };
    presentation::initialize(presentation_request);
    let _ = CLI_DIAGNOSTICS.set(diagnostic_options);
    match parse_arguments(arguments) {
        Ok(CommandLine::Help) => write_help(),
        Ok(command) => execute_with_feedback(command),
        Err(error) => {
            let message = format!("pop: {}\n\n{}", error.render(), localized("cli.usage", &[]));
            let _ = presentation::write_stderr_line(&message, Tone::Error);
            ExitCode::from(2)
        }
    }
}

fn execute_command(command: CommandLine) -> ExitCode {
    match command {
        CommandLine::Help => write_help(),
        CommandLine::Scaffold(options) => scaffold_package(&options),
        CommandLine::Check { source_path, dumps } => check_source(&source_path, &dumps),
        CommandLine::FixSource { source_path } => fix_source(&source_path),
        CommandLine::PackageFix { manifest_path } => fix_manifest(&manifest_path),
        CommandLine::PackageCheck {
            manifest_path,
            controls,
        }
        | CommandLine::Lint {
            manifest_path,
            controls,
        } => check_manifest(&manifest_path, &controls),
        CommandLine::Format {
            manifest_path,
            check,
        } => format_manifest(&manifest_path, check),
        CommandLine::Build {
            source_path,
            output_path,
        } => build_source(&source_path, &output_path),
        CommandLine::BuildBpf {
            source_path,
            target,
            runtime_profile,
            program,
            output_path,
        } => build_bpf_source(
            &source_path,
            &target,
            runtime_profile,
            program,
            &output_path,
        ),
        CommandLine::PackageBuild {
            manifest_path,
            controls,
            selection,
        } => build_manifest(&manifest_path, &controls, &selection)
            .map_or(ExitCode::FAILURE, |_| ExitCode::SUCCESS),
        CommandLine::Documentation {
            manifest_path,
            controls,
        } => document_manifest(&manifest_path, &controls),
        CommandLine::TranspileToC { source_path } => transpile_source_to_c(&source_path),
        CommandLine::Run {
            source_path,
            arguments,
        } => run_source(&source_path, &arguments),
        CommandLine::PackageRun {
            manifest_path,
            controls,
            arguments,
        } => run_manifest(&manifest_path, &controls, &arguments),
        CommandLine::Test {
            manifest_path,
            controls,
        } => test_manifest(&manifest_path, &controls),
        CommandLine::Benchmark {
            manifest_path,
            controls,
        } => benchmark_manifest(&manifest_path, &controls),
        CommandLine::FfiGenerate {
            alias,
            manifest_path,
            platform_target,
        } => ffi_generate(&alias, &manifest_path, &platform_target),
    }
}

fn execute_with_feedback(command: CommandLine) -> ExitCode {
    let (command_name, phase_key, phase_id) = command.feedback_identity();
    let phase = localized(phase_key, &[]);
    let fallback = localized("ui.command.interactiveFallback", &[]);
    let started = localized(
        "ui.command.started",
        &[
            LocalizedArgument::text("phase", &phase),
            LocalizedArgument::text("command", command_name),
        ],
    );
    let feedback = CommandFeedback::start(command_name, &phase, phase_id, &fallback, &started);
    let result = execute_command(command);
    let success = result == ExitCode::SUCCESS;
    let progress = localized(
        "ui.command.progress",
        &[
            LocalizedArgument::text("phase", &phase),
            LocalizedArgument::unsigned("completed", 1),
            LocalizedArgument::unsigned("total", 1),
        ],
    );
    let finished = localized(
        if success {
            "ui.command.finished"
        } else {
            "ui.command.failed"
        },
        &[LocalizedArgument::text("command", command_name)],
    );
    feedback.finish(success, &progress, &finished);
    result
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
    let _ = presentation::write_stderr_line(
        &format!("pop: {}", localized(key, arguments)),
        Tone::Error,
    );
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

fn extract_presentation_options(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<(PresentationRequest, DiagnosticOptions, Vec<OsString>), UsageError> {
    let mut output = Vec::new();
    let mut request = PresentationRequest::default();
    let mut diagnostic_options = DiagnosticOptions::default();
    let mut interactive_seen = false;
    let mut color_seen = false;
    let mut message_format_seen = false;
    let mut warning_wave_seen = false;
    let mut maximum_errors_seen = false;
    let mut warnings_as_errors = Vec::new();
    let mut disabled_warnings = Vec::new();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if argument == "--" {
            output.push(argument);
            output.extend(arguments);
            break;
        }
        match argument.to_str() {
            Some("--interactive") if !interactive_seen => {
                interactive_seen = true;
                request.interactive = true;
            }
            Some("--color") if !color_seen => {
                color_seen = true;
                let value = arguments
                    .next()
                    .ok_or_else(|| option_requires("--color", "auto|always|never"))?;
                request.color = ColorChoice::parse(&value.to_string_lossy()).ok_or_else(|| {
                    UsageError::new(
                        "cli.unsupportedChoice",
                        vec![
                            LocalizedArgument::text("choice", "color policy"),
                            LocalizedArgument::text("value", value.to_string_lossy()),
                            LocalizedArgument::text("expected", "auto, always, or never"),
                        ],
                    )
                })?;
            }
            Some("--messageFormat") if !message_format_seen => {
                message_format_seen = true;
                let value = arguments
                    .next()
                    .ok_or_else(|| option_requires("--messageFormat", "human|json"))?;
                request.message_format = MessageFormat::parse(&value.to_string_lossy())
                    .ok_or_else(|| {
                        UsageError::new(
                            "cli.unsupportedChoice",
                            vec![
                                LocalizedArgument::text("choice", "message format"),
                                LocalizedArgument::text("value", value.to_string_lossy()),
                                LocalizedArgument::text("expected", "human or json"),
                            ],
                        )
                    })?;
            }
            Some("--warningWave") if !warning_wave_seen => {
                warning_wave_seen = true;
                let value = arguments
                    .next()
                    .ok_or_else(|| option_requires("--warningWave", "<number|Latest>"))?;
                let value = value.to_string_lossy();
                let wave = if value == "Latest" {
                    latest_warning_wave()
                } else {
                    value.parse::<u32>().map_err(|_| {
                        UsageError::new(
                            "cli.unsupportedChoice",
                            vec![
                                LocalizedArgument::text("choice", "warning wave"),
                                LocalizedArgument::text("value", &value),
                                LocalizedArgument::text(
                                    "expected",
                                    "a non-negative number or Latest",
                                ),
                            ],
                        )
                    })?
                };
                diagnostic_options.policy = DiagnosticPolicy::new(wave);
            }
            Some("--warningsAsErrors") => {
                warnings_as_errors.push(parse_diagnostic_selector(
                    "--warningsAsErrors",
                    arguments.next(),
                )?);
            }
            Some("--disabledWarnings") => {
                disabled_warnings.push(parse_diagnostic_selector(
                    "--disabledWarnings",
                    arguments.next(),
                )?);
            }
            Some("--maximumErrors") if !maximum_errors_seen => {
                maximum_errors_seen = true;
                let value = arguments
                    .next()
                    .ok_or_else(|| option_requires("--maximumErrors", "<1..10000>"))?;
                let value = value.to_string_lossy();
                let maximum = value
                    .parse::<usize>()
                    .ok()
                    .filter(|maximum| (1..=10_000).contains(maximum))
                    .and_then(NonZeroUsize::new)
                    .ok_or_else(|| {
                        UsageError::new(
                            "cli.unsupportedChoice",
                            vec![
                                LocalizedArgument::text("choice", "maximum errors"),
                                LocalizedArgument::text("value", &value),
                                LocalizedArgument::text("expected", "1 through 10000"),
                            ],
                        )
                    })?;
                diagnostic_options.maximum_errors = maximum;
            }
            Some(
                "--interactive" | "--color" | "--messageFormat" | "--warningWave"
                | "--maximumErrors",
            ) => {
                return Err(unsupported_option(&argument));
            }
            _ => output.push(argument),
        }
    }
    diagnostic_options.policy = diagnostic_options
        .policy
        .with_warnings_as_errors(warnings_as_errors)
        .with_disabled_warnings(disabled_warnings);
    Ok((request, diagnostic_options, output))
}

fn parse_diagnostic_selector(
    option: &str,
    value: Option<OsString>,
) -> Result<DiagnosticSelector, UsageError> {
    let value = value.ok_or_else(|| option_requires(option, "<*|WarningGroup|POP####>"))?;
    let value = value.to_string_lossy();
    DiagnosticSelector::parse(value.as_ref()).map_err(|_| {
        UsageError::new(
            "cli.unsupportedChoice",
            vec![
                LocalizedArgument::text("choice", "diagnostic selector"),
                LocalizedArgument::text("value", &value),
                LocalizedArgument::text(
                    "expected",
                    "*, a WarningGroup, or POP followed by four digits",
                ),
            ],
        )
    })
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
    if command == "fix" {
        return parse_fix_arguments(arguments);
    }
    if command == "lint" {
        return parse_lint_arguments(arguments);
    }
    if command == "format" {
        return parse_format_arguments(arguments);
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
    if command == "test" {
        return parse_test_arguments(arguments);
    }
    if command == "benchmark" {
        return parse_benchmark_arguments(arguments);
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

fn parse_fix_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, UsageError> {
    let first = arguments.next();
    if first.as_deref() == Some(OsStr::new("--manifestPath")) {
        let manifest_path = required_manifest_path(arguments.next(), "fix")?;
        if arguments.next().is_some() {
            return Err(unexpected_arguments("fix"));
        }
        return Ok(CommandLine::PackageFix { manifest_path });
    }
    let source_path = required_source_path(first, "fix")?;
    if arguments.next().is_some() {
        return Err(unexpected_arguments("fix"));
    }
    Ok(CommandLine::FixSource { source_path })
}

fn parse_lint_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, UsageError> {
    let Some(option) = arguments.next() else {
        return Err(command_requires("lint", "--manifestPath <bubble.toml>"));
    };
    if option != "--manifestPath" {
        return Err(expected_option(&option, "--manifestPath"));
    }
    let manifest_path = required_manifest_path(arguments.next(), "lint")?;
    let controls = parse_manifest_controls(arguments)?;
    Ok(CommandLine::Lint {
        manifest_path,
        controls,
    })
}

fn parse_test_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, UsageError> {
    let Some(option) = arguments.next() else {
        return Err(command_requires("test", "--manifestPath <bubble.toml>"));
    };
    if option != "--manifestPath" {
        return Err(expected_option(&option, "--manifestPath"));
    }
    let manifest_path = required_manifest_path(arguments.next(), "test")?;
    let controls = parse_manifest_controls(arguments)?;
    Ok(CommandLine::Test {
        manifest_path,
        controls,
    })
}

fn parse_benchmark_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, UsageError> {
    let Some(option) = arguments.next() else {
        return Err(command_requires(
            "benchmark",
            "--manifestPath <bubble.toml>",
        ));
    };
    if option != "--manifestPath" {
        return Err(expected_option(&option, "--manifestPath"));
    }
    let manifest_path = required_manifest_path(arguments.next(), "benchmark")?;
    let controls = parse_manifest_controls(arguments)?;
    Ok(CommandLine::Benchmark {
        manifest_path,
        controls,
    })
}

fn parse_format_arguments(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<CommandLine, UsageError> {
    let Some(option) = arguments.next() else {
        return Err(command_requires("format", "--manifestPath <bubble.toml>"));
    };
    if option != "--manifestPath" {
        return Err(expected_option(&option, "--manifestPath"));
    }
    let manifest_path = required_manifest_path(arguments.next(), "format")?;
    let check = match arguments.next() {
        None => false,
        Some(option) if option == "--check" && arguments.next().is_none() => true,
        Some(option) => return Err(expected_option(&option, "--check")),
    };
    Ok(CommandLine::Format {
        manifest_path,
        check,
    })
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
    let controls = parse_manifest_controls(arguments)?;
    Ok(CommandLine::Documentation {
        manifest_path,
        controls,
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
        let controls = parse_manifest_controls(arguments)?;
        return Ok(CommandLine::PackageCheck {
            manifest_path,
            controls,
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
        let mut selection = PackageBuildSelection::Ordinary;
        let mut lock_arguments = Vec::new();
        while let Some(argument) = arguments.next() {
            let selected = match argument.to_str() {
                Some("--example") => Some("example"),
                Some("--test") => Some("test"),
                Some("--benchmark") => Some("benchmark"),
                _ => None,
            };
            if let Some(kind) = selected {
                if !matches!(selection, PackageBuildSelection::Ordinary) {
                    return Err(UsageError::new(
                        "cli.unsupportedChoice",
                        vec![
                            LocalizedArgument::text("choice", "Bubble selector"),
                            LocalizedArgument::text("value", argument.to_string_lossy()),
                            LocalizedArgument::text("expected", "exactly one selector"),
                        ],
                    ));
                }
                let name = arguments
                    .next()
                    .ok_or_else(|| {
                        option_requires(
                            match kind {
                                "example" => "--example",
                                "test" => "--test",
                                "benchmark" => "--benchmark",
                                _ => unreachable!("matched build selector"),
                            },
                            "<BubbleName>",
                        )
                    })?
                    .into_string()
                    .map_err(|_| {
                        UsageError::new(
                            "cli.unsupportedChoice",
                            vec![
                                LocalizedArgument::text("choice", "Bubble name"),
                                LocalizedArgument::text("value", "non-UTF-8 input"),
                                LocalizedArgument::text("expected", "PascalCase"),
                            ],
                        )
                    })?;
                selection = match kind {
                    "example" => PackageBuildSelection::Example(name),
                    "test" => PackageBuildSelection::Test(name),
                    "benchmark" => PackageBuildSelection::Benchmark(name),
                    _ => unreachable!("matched build selector"),
                };
            } else {
                lock_arguments.push(argument);
            }
        }
        let controls = parse_manifest_controls(lock_arguments)?;
        return Ok(CommandLine::PackageBuild {
            manifest_path,
            controls,
            selection,
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
        let controls = parse_manifest_controls(controls.iter().cloned())?;
        return Ok(CommandLine::PackageRun {
            manifest_path,
            controls,
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

fn parse_manifest_controls(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<ManifestControls, UsageError> {
    let mut locked = false;
    let mut offline = false;
    let mut features = Vec::new();
    let mut platform_target = None;
    let mut registry_root = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--locked") => locked = true,
            Some("--offline") => offline = true,
            Some("--frozen") => {
                locked = true;
                offline = true;
            }
            Some("--feature") => {
                let feature = arguments
                    .next()
                    .ok_or_else(|| option_requires("--feature", "<camelCase>"))?
                    .into_string()
                    .map_err(|_| {
                        UsageError::new(
                            "cli.unsupportedChoice",
                            vec![
                                LocalizedArgument::text("choice", "feature"),
                                LocalizedArgument::text("value", "non-UTF-8 input"),
                                LocalizedArgument::text("expected", "camelCase"),
                            ],
                        )
                    })?;
                features.push(feature);
            }
            Some("--platformTarget") if platform_target.is_none() => {
                platform_target = Some(
                    arguments
                        .next()
                        .ok_or_else(|| option_requires("--platformTarget", "<triple>"))?
                        .into_string()
                        .map_err(|_| {
                            UsageError::new(
                                "cli.unsupportedChoice",
                                vec![
                                    LocalizedArgument::text("choice", "platform target"),
                                    LocalizedArgument::text("value", "non-UTF-8 input"),
                                    LocalizedArgument::text("expected", "UTF-8 target triple"),
                                ],
                            )
                        })?,
                );
            }
            Some("--registryRoot") if registry_root.is_none() => {
                registry_root =
                    Some(PathBuf::from(arguments.next().ok_or_else(|| {
                        option_requires("--registryRoot", "<directory>")
                    })?));
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
    features.sort();
    if features.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(UsageError::new(
            "cli.unsupportedChoice",
            vec![
                LocalizedArgument::text("choice", "feature"),
                LocalizedArgument::text("value", "duplicate selection"),
                LocalizedArgument::text("expected", "unique feature names"),
            ],
        ));
    }
    let lock_mode = match (locked, offline) {
        (false, false) => LockMode::Normal,
        (true, false) => LockMode::Locked,
        (false, true) => LockMode::Offline,
        (true, true) => LockMode::Frozen,
    };
    Ok(ManifestControls {
        lock_mode,
        features,
        platform_target: platform_target.unwrap_or_else(|| NATIVE_PLATFORM_TARGET.to_owned()),
        registry_root,
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
    if let Err(error) = writeln!(
        io::stdout().lock(),
        "{}\n\n{}",
        localized("cli.usage", &[]),
        localized("cli.presentationOptions", &[])
    ) {
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
    let diagnostic_sources = vec![source.clone()];
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
    if !result.diagnostics().is_empty()
        && write_diagnostics(result.diagnostics(), &diagnostic_sources)
    {
        return ExitCode::FAILURE;
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

struct FixDocument {
    file: FileId,
    bubble: BubbleId,
    path: PathBuf,
    original: String,
    baseline_errors: BTreeSet<&'static str>,
}

fn fix_manifest(manifest_path: &Path) -> ExitCode {
    let Some(selection) = manifest_selection(manifest_path) else {
        return ExitCode::FAILURE;
    };
    let Some((standard, _)) = lower_toolchain_standard() else {
        return ExitCode::FAILURE;
    };
    let mut paths = BTreeSet::new();
    for manifest in &selection.packages {
        let Some(package_root) = manifest.parent() else {
            tool_failure!("pop: selected Package manifest has no parent directory");
            return ExitCode::FAILURE;
        };
        let Ok(sources) = collect_package_sources(package_root) else {
            return ExitCode::FAILURE;
        };
        paths.extend(sources.into_values());
    }

    let mut documents = Vec::new();
    let mut snapshots = Vec::new();
    let mut diagnostics = Vec::new();
    for (index, path) in paths.into_iter().enumerate() {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
            Ok(_) => {
                tool_failure!(
                    "pop: fix requires real Package source files: `{}`",
                    path.display()
                );
                return ExitCode::FAILURE;
            }
            Err(error) => {
                tool_failure!("pop: could not inspect `{}`: {error}", path.display());
                return ExitCode::FAILURE;
            }
        };
        let original = match fs::read_to_string(&path) {
            Ok(original) => original,
            Err(error) => {
                tool_failure!("pop: could not read `{}`: {error}", path.display());
                return ExitCode::FAILURE;
            }
        };
        let Ok(raw) = u32::try_from(index) else {
            tool_failure!("pop: too many Package sources to fix");
            return ExitCode::FAILURE;
        };
        let file = FileId::from_raw(raw);
        let bubble = BubbleId::from_raw(FIRST_PACKAGE_BUBBLE.saturating_add(raw));
        let Some(initial) =
            analyze_fix_document(file, bubble, &path, &original, &standard.metadata)
        else {
            return ExitCode::FAILURE;
        };
        let baseline_errors = initial
            .iter()
            .filter(|diagnostic| diagnostic.severity() == DiagnosticSeverity::Error)
            .map(|diagnostic| diagnostic.code().as_str())
            .collect();
        diagnostics.extend(initial);
        let snapshot = DocumentSnapshot::new(file, 0, original.clone());
        snapshots.push(if metadata.permissions().readonly() {
            snapshot.read_only()
        } else {
            snapshot
        });
        documents.push(FixDocument {
            file,
            bubble,
            path,
            original,
            baseline_errors,
        });
    }
    let Ok(mut workspace) = WorkspaceSnapshot::new(snapshots) else {
        tool_failure!("pop: internal compiler error: duplicate Package fix source identity");
        return ExitCode::from(101);
    };
    let summary = match apply_safe_fix_all(&mut workspace, &diagnostics, |candidate| {
        documents.iter().all(|document| {
            let Some(candidate_document) = candidate.document(document.file) else {
                return false;
            };
            let Some(candidate_diagnostics) = analyze_fix_document(
                document.file,
                document.bubble,
                &document.path,
                candidate_document.text(),
                &standard.metadata,
            ) else {
                return false;
            };
            let has_unapplied_safe_fix = candidate_diagnostics
                .iter()
                .flat_map(Diagnostic::fixes)
                .any(|fix| {
                    fix.applicability() == FixApplicability::Safe
                        && fix.fix_all_equivalence().is_some()
                });
            !has_unapplied_safe_fix
                && candidate_diagnostics.iter().all(|diagnostic| {
                    diagnostic.severity() != DiagnosticSeverity::Error
                        || document
                            .baseline_errors
                            .contains(diagnostic.code().as_str())
                })
        })
    }) {
        Ok(summary) => summary,
        Err(error) => {
            tool_failure!("pop: safe Package fix-all was not applied: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut changes = Vec::new();
    for (index, document) in documents.iter().enumerate() {
        let candidate = workspace
            .document(document.file)
            .expect("fix transaction preserves every Package document")
            .text();
        if candidate == document.original {
            continue;
        }
        let Some(parent) = document.path.parent() else {
            tool_failure!("pop: source path has no parent directory");
            return ExitCode::FAILURE;
        };
        let Some(name) = document.path.file_name().and_then(OsStr::to_str) else {
            tool_failure!("pop: source path is not valid UTF-8");
            return ExitCode::FAILURE;
        };
        let staging = parent.join(format!(
            ".{name}.pop-fix-{}-{index}.staging",
            std::process::id()
        ));
        let backup = parent.join(format!(
            ".{name}.pop-fix-{}-{index}.backup",
            std::process::id()
        ));
        changes.push(FileChange {
            path: document.path.clone(),
            expected: document.original.as_bytes().to_vec(),
            bytes: candidate.as_bytes().to_vec(),
            staging,
            backup,
        });
    }

    let validate_package = || {
        selection.packages.iter().all(|manifest| {
            lower_package(
                manifest,
                RootBubbleSelection::All,
                &[],
                NATIVE_PLATFORM_TARGET,
                None,
                false,
            )
            .is_some()
        })
    };
    if changes.is_empty() {
        if !validate_package() {
            return ExitCode::FAILURE;
        }
        return write_fix_summary(summary);
    }
    if let Err(error) = publish_file_transaction(&changes, validate_package) {
        tool_failure!("pop: could not publish safe Package fixes atomically: {error}");
        return ExitCode::FAILURE;
    }
    write_fix_summary(summary)
}

fn analyze_fix_document(
    file: FileId,
    bubble: BubbleId,
    path: &Path,
    text: &str,
    standard: &ReferenceMetadata,
) -> Option<Vec<Diagnostic>> {
    let source = SourceFile::new(file, path.to_string_lossy().into_owned(), text.to_owned())
        .map_err(|error| tool_failure!("pop: could not load `{}`: {error}", path.display()))
        .ok()?;
    let result = analyze_bubble(
        FrontEndBubbleInput::new(
            bubble,
            NamespaceId::from_raw(bubble.raw()),
            vec![STANDARD_BUBBLE],
            vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
        )
        .with_reference_metadata(vec![standard.clone()]),
    );
    Some(result.diagnostics().to_vec())
}

fn fix_source(source_path: &Path) -> ExitCode {
    let metadata = match fs::symlink_metadata(source_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
        Ok(_) => {
            tool_failure!(
                "pop: fix requires a real writable source file: `{}`",
                source_path.display()
            );
            return ExitCode::FAILURE;
        }
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
    let file = FileId::from_raw(0);
    let source = match SourceFile::new(
        file,
        source_path.to_string_lossy().into_owned(),
        source_text.clone(),
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
            vec![FrontEndModule::new(ModuleId::from_raw(0), source.clone())],
        )
        .with_reference_metadata(vec![standard.metadata.clone()]),
    );
    if result.diagnostics().is_empty() {
        return write_fix_summary(FixAllSummary::default());
    }

    let document = DocumentSnapshot::new(file, 0, source_text.clone());
    let document = if metadata.permissions().readonly() {
        document.read_only()
    } else {
        document
    };
    let Ok(mut workspace) = WorkspaceSnapshot::new([document]) else {
        tool_failure!("pop: internal compiler error: duplicate fix source identity");
        return ExitCode::from(101);
    };
    let display_path = source.path().to_owned();
    let summary = match apply_safe_fix_all(&mut workspace, result.diagnostics(), |candidate| {
        let Some(document) = candidate.document(file) else {
            return false;
        };
        let Ok(candidate_source) =
            SourceFile::new(file, display_path.clone(), document.text().to_owned())
        else {
            return false;
        };
        let candidate_result = analyze_bubble(
            FrontEndBubbleInput::new(
                BubbleId::from_raw(FIRST_PACKAGE_BUBBLE),
                NamespaceId::from_raw(FIRST_PACKAGE_BUBBLE),
                vec![STANDARD_BUBBLE],
                vec![FrontEndModule::new(ModuleId::from_raw(0), candidate_source)],
            )
            .with_reference_metadata(vec![standard.metadata.clone()]),
        );
        candidate_result.hir().is_some()
            && candidate_result
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.severity() != DiagnosticSeverity::Error)
    }) {
        Ok(summary) => summary,
        Err(error) => {
            tool_failure!("pop: safe fix-all was not applied: {error}");
            return ExitCode::FAILURE;
        }
    };

    if summary.applied_fix_count() == 0 {
        let summary_result = write_fix_summary(summary);
        if summary_result != ExitCode::SUCCESS {
            return summary_result;
        }
        return if write_diagnostics(result.diagnostics(), &[source]) {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        };
    }

    let candidate = workspace
        .document(file)
        .expect("fix transaction preserves the source document")
        .text();
    if let Err(error) = publish_fixed_source(
        source_path,
        source_text.as_bytes(),
        candidate.as_bytes(),
        metadata.permissions(),
    ) {
        tool_failure!("pop: could not publish safe fixes atomically: {error}");
        return ExitCode::FAILURE;
    }
    write_fix_summary(summary)
}

fn publish_fixed_source(
    source_path: &Path,
    expected: &[u8],
    replacement: &[u8],
    permissions: fs::Permissions,
) -> Result<(), String> {
    let parent = source_path
        .parent()
        .ok_or_else(|| "source path has no parent directory".to_owned())?;
    let name = source_path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| "source filename must be UTF-8".to_owned())?;
    let mut staged = None;
    for attempt in 0..32 {
        let path = parent.join(format!(".{name}.pop-fix-{}-{attempt}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                staged = Some((path, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let Some((staged_path, mut staged_file)) = staged else {
        return Err("no unique same-directory staging path is available".to_owned());
    };
    let stage_result = (|| {
        staged_file.write_all(replacement)?;
        staged_file.sync_all()?;
        staged_file.set_permissions(permissions)?;
        drop(staged_file);
        let current = fs::read(source_path)?;
        if current != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "source changed after the fix snapshot",
            ));
        }
        fs::rename(&staged_path, source_path)
    })();
    if let Err(error) = stage_result {
        let _ = fs::remove_file(&staged_path);
        return Err(error.to_string());
    }
    Ok(())
}

fn write_fix_summary(summary: FixAllSummary) -> ExitCode {
    if presentation::is_json() {
        if let Err(error) = presentation::write_json(&json!({
            "schemaVersion": 1,
            "kind": "fixSummary",
            "appliedFixes": summary.applied_fix_count(),
            "changedDocuments": summary.changed_document_count(),
            "skippedReview": summary.skipped_review_count(),
            "skippedUnsafe": summary.skipped_unsafe_count(),
            "skippedUnproven": summary.skipped_unproven_count(),
        })) {
            emit_localized(
                "cli.writeDiagnosticsFailed",
                &[LocalizedArgument::external("detail", error)],
            );
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }
    let message = localized(
        "ui.fixSummary",
        &[
            LocalizedArgument::unsigned(
                "applied",
                u64::try_from(summary.applied_fix_count()).unwrap_or(u64::MAX),
            ),
            LocalizedArgument::unsigned(
                "changed",
                u64::try_from(summary.changed_document_count()).unwrap_or(u64::MAX),
            ),
            LocalizedArgument::unsigned(
                "review",
                u64::try_from(summary.skipped_review_count()).unwrap_or(u64::MAX),
            ),
            LocalizedArgument::unsigned(
                "unsafe",
                u64::try_from(summary.skipped_unsafe_count()).unwrap_or(u64::MAX),
            ),
            LocalizedArgument::unsigned(
                "unproven",
                u64::try_from(summary.skipped_unproven_count()).unwrap_or(u64::MAX),
            ),
        ],
    );
    if presentation::write_stderr_line(&message, Tone::Success).is_err() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn native_target() -> TargetSpec {
    TargetSpec::for_triple(NATIVE_PLATFORM_TARGET).expect("repository native target is complete")
}

fn manifest_native_target(controls: &ManifestControls) -> Option<TargetSpec> {
    let target = TargetSpec::for_triple(&controls.platform_target)
        .map_err(|_| {
            tool_failure!(
                "pop: unknown platform target `{}`",
                controls.platform_target
            );
        })
        .ok()?;
    if target.triple() != NATIVE_PLATFORM_TARGET {
        tool_failure!(
            "pop: platform target `{}` does not support native Package workflows in this toolchain",
            target.triple()
        );
        return None;
    }
    Some(target)
}

fn diagnostic_severity_name(severity: DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Information => "information",
        DiagnosticSeverity::Hint => "hint",
    }
}

fn diagnostic_category_name(category: DiagnosticCategory) -> &'static str {
    match category {
        DiagnosticCategory::Syntax => "syntax",
        DiagnosticCategory::Resolution => "resolution",
        DiagnosticCategory::Type => "type",
        DiagnosticCategory::Flow => "flow",
        DiagnosticCategory::CompileTime => "compileTime",
        DiagnosticCategory::RuntimeSafety => "runtimeSafety",
        DiagnosticCategory::Style => "style",
        DiagnosticCategory::Backend => "backend",
        DiagnosticCategory::Project => "project",
        DiagnosticCategory::Tooling => "tooling",
    }
}

fn diagnostic_origin_name(origin: DiagnosticOriginKind) -> &'static str {
    match origin {
        DiagnosticOriginKind::Source => "source",
        DiagnosticOriginKind::Generated => "generated",
        DiagnosticOriginKind::Desugared => "desugared",
        DiagnosticOriginKind::CompileTime => "compileTime",
    }
}

fn fix_applicability_name(applicability: FixApplicability) -> &'static str {
    match applicability {
        FixApplicability::Safe => "safe",
        FixApplicability::RequiresReview => "requiresReview",
        FixApplicability::Unsafe => "unsafe",
    }
}

fn span_json(span: SourceSpan, sources: &[SourceFile]) -> Value {
    let source = sources.iter().find(|source| source.id() == span.file());
    let start = source.and_then(|source| source.line_column(span.range().start()));
    let end = source.and_then(|source| source.line_column(span.range().end()));
    json!({
        "file": span.file().raw(),
        "path": source.map(SourceFile::path),
        "start": span.range().start().to_u32(),
        "end": span.range().end().to_u32(),
        "startPosition": start.map(|position| json!({
            "line": position.line(),
            "column": position.column(),
        })),
        "endPosition": end.map(|position| json!({
            "line": position.line(),
            "column": position.column(),
        })),
        "origin": span.origin().map(|origin| origin.raw()),
    })
}

fn diagnostic_argument_json(argument: &DiagnosticArgument) -> Value {
    match argument {
        DiagnosticArgument::Character(value) => {
            json!({ "kind": "character", "value": value.to_string() })
        }
        DiagnosticArgument::Identifier(value) => {
            json!({ "kind": "identifier", "value": value })
        }
        DiagnosticArgument::Type { type_id, display } => json!({
            "kind": "type",
            "typeId": type_id.raw(),
            "display": display,
        }),
        DiagnosticArgument::Unsigned(value) => {
            json!({ "kind": "unsigned", "value": value })
        }
        DiagnosticArgument::SyntaxExpectation(value) => {
            json!({ "kind": "syntaxExpectation", "value": value })
        }
        DiagnosticArgument::Token(value) => json!({ "kind": "token", "value": value }),
    }
}

fn diagnostic_json(
    diagnostic: &Diagnostic,
    policy: &DiagnosticPolicy,
    sources: &[SourceFile],
) -> Value {
    let labels = diagnostic
        .labels()
        .iter()
        .map(|label| {
            json!({
                "span": span_json(label.span(), sources),
                "messageKey": label.message_key().as_str(),
                "arguments": label
                    .arguments()
                    .iter()
                    .map(diagnostic_argument_json)
                    .collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let notes = diagnostic
        .notes()
        .iter()
        .map(|note| {
            json!({
                "messageKey": note.message_key().as_str(),
                "arguments": note
                    .arguments()
                    .iter()
                    .map(diagnostic_argument_json)
                    .collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let origins = diagnostic
        .origin_chain()
        .iter()
        .map(|origin| {
            json!({
                "kind": diagnostic_origin_name(origin.kind()),
                "span": span_json(origin.span(), sources),
            })
        })
        .collect::<Vec<_>>();
    let fixes = diagnostic
        .fixes()
        .iter()
        .map(|fix| {
            json!({
                "id": fix.id(),
                "titleKey": fix.title_key().as_str(),
                "applicability": fix_applicability_name(fix.applicability()),
                "equivalenceKey": fix.fix_all_equivalence(),
                "edit": {
                    "revision": fix.edit().revision(),
                    "edits": fix
                        .edit()
                        .edits()
                        .iter()
                        .map(|edit| json!({
                            "file": edit.file().raw(),
                            "path": sources
                                .iter()
                                .find(|source| source.id() == edit.file())
                                .map(SourceFile::path),
                            "start": edit.range().start().to_u32(),
                            "end": edit.range().end().to_u32(),
                            "replacement": edit.replacement(),
                        }))
                        .collect::<Vec<_>>(),
                },
            })
        })
        .collect::<Vec<_>>();
    json!({
        "code": diagnostic.code().as_str(),
        "severity": diagnostic_severity_name(diagnostic.severity()),
        "category": diagnostic_category_name(diagnostic.category()),
        "messageKey": diagnostic.message_key().as_str(),
        "arguments": diagnostic
            .arguments()
            .iter()
            .map(diagnostic_argument_json)
            .collect::<Vec<_>>(),
        "primarySpan": span_json(diagnostic.primary_span(), sources),
        "labels": labels,
        "notes": notes,
        "originChain": origins,
        "fixes": fixes,
        "warningWave": diagnostic.warning_wave().map(|wave| wave.value()),
        "warningGroup": diagnostic_catalog()
            .ok()
            .and_then(|entries| entries
                .into_iter()
                .find(|entry| entry.code() == diagnostic.code()))
            .and_then(|entry| entry.warning_group())
            .map(|group| group.name()),
        "suppressionKey": diagnostic
            .suppression_key()
            .map(|key| key.as_str()),
        "policy": {
            "enabled": policy.evaluate(diagnostic).is_enabled(),
            "promoted": policy.evaluate(diagnostic).is_promoted(),
            "blocksArtifact": policy.evaluate(diagnostic).blocks_artifact(),
        },
    })
}

fn write_diagnostics(diagnostics: &[Diagnostic], sources: &[SourceFile]) -> bool {
    let options = CLI_DIAGNOSTICS.get().cloned().unwrap_or_default();
    let report = DiagnosticReport::new(diagnostics, &options.policy, options.maximum_errors);
    if presentation::is_json() {
        for diagnostic in report.diagnostics() {
            if let Err(error) = presentation::write_json(&json!({
                "schemaVersion": 1,
                "kind": "diagnostic",
                "diagnostic": diagnostic_json(diagnostic, &options.policy, sources),
            })) {
                emit_localized(
                    "cli.writeDiagnosticsFailed",
                    &[LocalizedArgument::external("detail", error)],
                );
                return true;
            }
        }
        if report.reached_error_limit()
            && presentation::write_json(&json!({
                "schemaVersion": 1,
                "kind": "diagnosticLimitReached",
                "maximumErrors": options.maximum_errors.get(),
                "omittedErrors": report.omitted_error_count(),
            }))
            .is_err()
        {
            return true;
        }
        return report.blocks_artifact();
    }
    let diagnostic_sources = sources
        .iter()
        .map(|source| DiagnosticSource::new(source.id(), source.path(), source.text()))
        .collect::<Vec<_>>();
    for diagnostic in report.diagnostics() {
        match rendering().diagnostic_with_sources_and_width(
            diagnostic,
            &diagnostic_sources,
            presentation::display_width,
        ) {
            Ok(rendered) => {
                let tone = match diagnostic.severity() {
                    DiagnosticSeverity::Error => Tone::Error,
                    DiagnosticSeverity::Warning => Tone::Warning,
                    DiagnosticSeverity::Information | DiagnosticSeverity::Hint => Tone::Information,
                };
                if let Err(error) = presentation::write_diagnostic(&rendered, tone) {
                    emit_localized(
                        "cli.writeDiagnosticsFailed",
                        &[LocalizedArgument::external("detail", error)],
                    );
                    return true;
                }
            }
            Err(error) => {
                emit_localized(
                    "cli.renderDiagnosticsFailed",
                    &[LocalizedArgument::external("detail", error)],
                );
                return true;
            }
        }
    }
    if report.reached_error_limit() {
        emit_localized(
            "cli.diagnosticLimitReached",
            &[
                LocalizedArgument::unsigned(
                    "maximum",
                    u64::try_from(options.maximum_errors.get()).unwrap_or(u64::MAX),
                ),
                LocalizedArgument::unsigned(
                    "omitted",
                    u64::try_from(report.omitted_error_count()).unwrap_or(u64::MAX),
                ),
            ],
        );
    }
    report.blocks_artifact()
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
    bubbles: Vec<LoweredPackageBubble>,
    native_link_sources: Vec<NativeLinkPlanSource>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RootBubbleSelection {
    Ordinary,
    All,
    Tests,
    Examples,
    Benchmarks,
}

impl RootBubbleSelection {
    const fn selects(self, kind: BubbleKind) -> bool {
        match self {
            Self::Ordinary => matches!(kind, BubbleKind::Library | BubbleKind::Binary),
            Self::All => true,
            Self::Tests => matches!(kind, BubbleKind::Library | BubbleKind::Test),
            Self::Examples => matches!(kind, BubbleKind::Library | BubbleKind::Example),
            Self::Benchmarks => matches!(kind, BubbleKind::Library | BubbleKind::Benchmark),
        }
    }

    const fn includes_development(self) -> bool {
        matches!(
            self,
            Self::All | Self::Tests | Self::Examples | Self::Benchmarks
        )
    }
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

fn lower_package(
    manifest_path: &Path,
    selection: RootBubbleSelection,
    selected_features: &[String],
    platform_target: &str,
    registry_root: Option<&Path>,
    offline: bool,
) -> Option<LoweredPackage> {
    let manifest_path = fs::canonicalize(manifest_path)
        .map_err(|error| {
            tool_failure!(
                "pop: could not resolve `{}`: {error}",
                manifest_path.display()
            );
        })
        .ok()?;
    let (standard, standard_bubble) = lower_toolchain_standard()?;
    let resolution_root = dependency_resolution_root(&manifest_path);
    let mut state = PackageLoweringState {
        next_bubble: FIRST_PACKAGE_BUBBLE,
        visiting: BTreeSet::new(),
        resolved: BTreeMap::new(),
        bubbles: vec![standard_bubble],
        native_link_sources: Vec::new(),
        standard,
        resolution_root,
        registry_root: registry_root.map(Path::to_path_buf),
        offline,
    };
    lower_package_recursive(
        &manifest_path,
        true,
        selection,
        selected_features,
        platform_target,
        &mut state,
    )?;
    Some(LoweredPackage {
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
    resolution_root: PathBuf,
    registry_root: Option<PathBuf>,
    offline: bool,
}

fn lower_package_recursive(
    manifest_path: &Path,
    root_package: bool,
    root_selection: RootBubbleSelection,
    selected_features: &[String],
    platform_target: &str,
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
        verify_ffi_generated_bindings(package_root, &manifest, platform_target)
            .map_err(|error| tool_failure!("pop: {error}"))
            .ok()?;
    let native_link_plan = manifest
        .native_link_plan(platform_target)
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()?;
    state.native_link_sources.push(NativeLinkPlanSource::new(
        package_root,
        native_link_plan.clone(),
    ));

    let mut external_libraries = vec![state.standard.clone()];
    let selected_dependencies = manifest
        .selected_dependencies_with_features(platform_target, false, selected_features)
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()?;
    for requirement in selected_dependencies {
        external_libraries.push(resolve_lowering_dependency(
            requirement,
            package_root,
            platform_target,
            state,
        )?);
    }
    let mut development_libraries = Vec::new();
    if root_package && root_selection.includes_development() {
        manifest
            .selected_dependencies_with_features(platform_target, true, selected_features)
            .map_err(|error| tool_failure!("pop: {error}"))
            .ok()?;
        for requirement in manifest.development_dependencies() {
            development_libraries.push(resolve_lowering_dependency(
                requirement,
                package_root,
                platform_target,
                state,
            )?);
        }
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
    let development_metadata = development_libraries
        .iter()
        .map(|library| library.metadata.clone())
        .collect::<Vec<_>>();
    let development_retained_adapters = development_libraries
        .iter()
        .filter_map(|library| {
            library
                .retained_adapters_popc
                .clone()
                .map(|bytes| (library.metadata.bubble(), bytes))
        })
        .collect::<Vec<_>>();
    let development_artifact_dependencies = development_libraries
        .iter()
        .map(ResolvedPackageLibrary::artifact_dependency)
        .collect::<Vec<_>>();
    let normal_ffi_dependency = external_libraries
        .iter()
        .find(|library| library.package == FFI_PACKAGE_NAME)
        .map(|library| library.metadata.bubble());
    let development_ffi_dependency = development_libraries
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
                || root_package && root_selection.selects(bubble.kind())
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
        let is_auxiliary = matches!(
            bubble.kind(),
            BubbleKind::Test | BubbleKind::Example | BubbleKind::Benchmark
        );
        let mut bubble_artifact_dependencies = artifact_dependencies.clone();
        if is_auxiliary {
            dependency_metadata.extend(development_metadata.clone());
            dependency_retained_adapters.extend(development_retained_adapters.clone());
            bubble_artifact_dependencies.extend(development_artifact_dependencies.clone());
        }
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
            matches!(
                bubble.kind(),
                BubbleKind::Binary | BubbleKind::Test | BubbleKind::Example | BubbleKind::Benchmark
            ),
            dependency_metadata,
            dependency_retained_adapters,
            Vec::new(),
            if is_auxiliary {
                development_ffi_dependency.or(normal_ffi_dependency)
            } else {
                normal_ffi_dependency
            },
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
            dependencies: bubble_artifact_dependencies,
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

fn inherited_dependency_requirement(
    requirement: &DependencyRequirement,
    package_root: &Path,
) -> Option<(DependencyRequirement, PathBuf)> {
    if !requirement.workspace_inherited() {
        return Some((requirement.clone(), package_root.to_path_buf()));
    }
    for ancestor in package_root.ancestors() {
        let manifest_path = ancestor.join("bubble.toml");
        let Ok(text) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        if !text.lines().any(|line| line.trim() == "[workspace]") {
            continue;
        }
        let workspace = parse_workspace_manifest(&text)
            .map_err(|error| tool_failure!("pop: {error}"))
            .ok()?;
        let inherited = workspace
            .dependencies()
            .iter()
            .find(|dependency| dependency.alias() == requirement.alias())
            .cloned()
            .or_else(|| {
                tool_failure!(
                    "pop: workspace dependency `{}` has no inherited resolution entry",
                    requirement.alias()
                );
                None
            })?;
        return Some((inherited, ancestor.to_path_buf()));
    }
    tool_failure!(
        "pop: workspace dependency `{}` has no ancestor Workspace root",
        requirement.alias()
    );
    None
}

struct ResolvedDependencyLocation {
    manifest_path: PathBuf,
    locked_source: Option<LockedSource>,
}

fn resolve_dependency_location(
    requirement: &DependencyRequirement,
    dependency_root: &Path,
    resolution_root: &Path,
    registry_root: Option<&Path>,
    offline: bool,
) -> Option<ResolvedDependencyLocation> {
    match requirement.source() {
        DependencySource::LocalPath(path) => Some(ResolvedDependencyLocation {
            manifest_path: dependency_root.join(path).join("bubble.toml"),
            locked_source: None,
        }),
        DependencySource::Registry => {
            let registry_root = registry_root.or_else(|| {
                tool_failure!(
                    "pop: registry dependency `{}` requires --registryRoot <directory>",
                    requirement.alias()
                );
                None
            })?;
            let version = requirement.version_requirement().or_else(|| {
                tool_failure!(
                    "pop: registry dependency `{}` requires an exact version",
                    requirement.alias()
                );
                None
            })?;
            let package_root = registry_package_root(registry_root, requirement.alias(), version)?;
            Some(ResolvedDependencyLocation {
                manifest_path: package_root.join("bubble.toml"),
                locked_source: Some(LockedSource::Registry("default".to_owned())),
            })
        }
        DependencySource::ExactGit {
            repository,
            revision,
        } => {
            let checkout = exact_git_checkout(
                repository,
                revision,
                dependency_root,
                resolution_root,
                offline,
            )?;
            Some(ResolvedDependencyLocation {
                manifest_path: checkout.join("bubble.toml"),
                locked_source: Some(LockedSource::ExactGit {
                    repository: repository.clone(),
                    revision: revision.clone(),
                }),
            })
        }
        DependencySource::Workspace => {
            tool_failure!(
                "pop: workspace dependency `{}` has no inherited resolution entry",
                requirement.alias()
            );
            None
        }
    }
}

fn registry_package_root(root: &Path, alias: &str, version: &str) -> Option<PathBuf> {
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| {
            tool_failure!(
                "pop: could not inspect registry root `{}`: {error}",
                root.display()
            );
        })
        .ok()?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        tool_failure!(
            "pop: registry root must be a real directory: `{}`",
            root.display()
        );
        return None;
    }
    let root = fs::canonicalize(root).ok()?;
    let mut path = root.clone();
    for component in [alias, version] {
        path.push(component);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| {
                tool_failure!(
                    "pop: registry mirror entry `{}` is unavailable: {error}",
                    path.display()
                );
            })
            .ok()?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            tool_failure!(
                "pop: registry mirror entry must be a real directory: `{}`",
                path.display()
            );
            return None;
        }
    }
    let canonical = fs::canonicalize(&path).ok()?;
    canonical
        .starts_with(&root)
        .then_some(canonical)
        .or_else(|| {
            tool_failure!("pop: registry mirror entry escaped its root");
            None
        })
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExactGitSourceRecord {
    schema_version: u16,
    repository: String,
    revision: String,
    content_sha256: String,
}

fn exact_git_checkout(
    repository: &str,
    revision: &str,
    dependency_root: &Path,
    resolution_root: &Path,
    offline: bool,
) -> Option<PathBuf> {
    if !is_full_git_revision(revision) || repository_locator_has_credentials(repository) {
        tool_failure!("pop: exact-Git dependency has an invalid repository or full revision");
        return None;
    }
    let mut key_payload = Vec::new();
    append_hash_input(&mut key_payload, "repository", repository.as_bytes());
    append_hash_input(&mut key_payload, "revision", revision.as_bytes());
    let key = sha256_hex(&key_payload);
    let source_root = resolution_root
        .join("target/resolution/exactGit")
        .join(&key);
    let checkout = source_root.join("checkout");
    let record_path = source_root.join("source.json");
    if source_root.exists() {
        return verify_exact_git_checkout(&checkout, &record_path, repository, revision)
            .then_some(checkout);
    }
    if offline {
        tool_failure!(
            "pop: offline exact-Git dependency `{repository}` revision `{revision}` is not cached"
        );
        return None;
    }
    let parent = source_root.parent()?;
    fs::create_dir_all(parent)
        .map_err(|error| tool_failure!("pop: could not create Git source cache: {error}"))
        .ok()?;
    let staging = parent.join(format!(".{key}.staging-{}", std::process::id()));
    if staging.exists() {
        tool_failure!(
            "pop: exact-Git staging path already exists: `{}`",
            staging.display()
        );
        return None;
    }
    fs::create_dir(&staging)
        .map_err(|error| tool_failure!("pop: could not stage exact-Git source: {error}"))
        .ok()?;
    let staged_checkout = staging.join("checkout");
    let repository_argument = if repository.contains("://") || Path::new(repository).is_absolute() {
        PathBuf::from(repository)
    } else {
        dependency_root.join(repository)
    };
    let clone = Command::new("git")
        .args(["clone", "--quiet", "--no-checkout"])
        .arg(&repository_argument)
        .arg(&staged_checkout)
        .status();
    if !clone.is_ok_and(|status| status.success()) {
        let _ = fs::remove_dir_all(&staging);
        tool_failure!("pop: Git could not clone exact dependency `{repository}` without a shell");
        return None;
    }
    let checkout_status = Command::new("git")
        .arg("-C")
        .arg(&staged_checkout)
        .args(["checkout", "--quiet", "--detach", revision])
        .status();
    if !checkout_status.is_ok_and(|status| status.success())
        || git_head(&staged_checkout).as_deref() != Some(revision)
    {
        let _ = fs::remove_dir_all(&staging);
        tool_failure!("pop: Git dependency did not resolve exact revision `{revision}`");
        return None;
    }
    let manifest_path = staged_checkout.join("bubble.toml");
    let Some(content_sha256) = collect_package_sources(&staged_checkout)
        .ok()
        .and_then(|sources| package_content_hash(&manifest_path, &sources))
    else {
        let _ = fs::remove_dir_all(&staging);
        tool_failure!("pop: exact-Git checkout has invalid Package sources");
        return None;
    };
    let record = ExactGitSourceRecord {
        schema_version: 1,
        repository: repository.to_owned(),
        revision: revision.to_owned(),
        content_sha256,
    };
    let mut record_bytes = serde_json::to_vec(&record).ok()?;
    record_bytes.push(b'\n');
    if let Err(error) = fs::write(staging.join("source.json"), record_bytes) {
        let _ = fs::remove_dir_all(&staging);
        tool_failure!("pop: could not write exact-Git source record: {error}");
        return None;
    }
    if let Err(error) = fs::rename(&staging, &source_root) {
        let _ = fs::remove_dir_all(&staging);
        tool_failure!("pop: could not publish exact-Git source cache: {error}");
        return None;
    }
    verify_exact_git_checkout(&checkout, &record_path, repository, revision).then_some(checkout)
}

fn verify_exact_git_checkout(
    checkout: &Path,
    record_path: &Path,
    repository: &str,
    revision: &str,
) -> bool {
    let Ok(record_bytes) = fs::read(record_path) else {
        tool_failure!("pop: exact-Git source record is missing");
        return false;
    };
    let Ok(record) = serde_json::from_slice::<ExactGitSourceRecord>(&record_bytes) else {
        tool_failure!("pop: exact-Git source record is malformed");
        return false;
    };
    let mut canonical = serde_json::to_vec(&record).expect("source record serializes");
    canonical.push(b'\n');
    if canonical != record_bytes
        || record.schema_version != 1
        || record.repository != repository
        || record.revision != revision
        || git_head(checkout).as_deref() != Some(revision)
    {
        tool_failure!("pop: exact-Git source record or revision does not match");
        return false;
    }
    let manifest_path = checkout.join("bubble.toml");
    let Some(sources) = collect_package_sources(checkout).ok() else {
        return false;
    };
    if package_content_hash(&manifest_path, &sources).as_deref()
        != Some(record.content_sha256.as_str())
    {
        tool_failure!("pop: exact-Git cached source content hash does not match");
        return false;
    }
    true
}

fn git_head(checkout: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn is_full_git_revision(revision: &str) -> bool {
    matches!(revision.len(), 40 | 64) && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn repository_locator_has_credentials(repository: &str) -> bool {
    repository
        .split_once("://")
        .and_then(|(_, remainder)| remainder.split('/').next())
        .is_some_and(|authority| authority.contains('@'))
}

fn resolve_lowering_dependency(
    requirement: &DependencyRequirement,
    package_root: &Path,
    platform_target: &str,
    state: &mut PackageLoweringState,
) -> Option<ResolvedPackageLibrary> {
    let (requirement, dependency_root) =
        inherited_dependency_requirement(requirement, package_root)?;
    let dependency_manifest = resolve_dependency_location(
        &requirement,
        &dependency_root,
        &state.resolution_root,
        state.registry_root.as_deref(),
        state.offline,
    )?
    .manifest_path;
    let dependency_manifest = fs::canonicalize(&dependency_manifest)
        .map_err(|error| {
            tool_failure!(
                "pop: could not resolve dependency `{}` at `{}`: {error}",
                requirement.alias(),
                dependency_manifest.display()
            );
        })
        .ok()?;
    let Some(library) = lower_package_recursive(
        &dependency_manifest,
        false,
        RootBubbleSelection::Ordinary,
        &[],
        platform_target,
        state,
    )?
    else {
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
    Some(library)
}

fn check_manifest(manifest_path: &Path, controls: &ManifestControls) -> ExitCode {
    let Some(selection) = manifest_selection(manifest_path) else {
        return ExitCode::FAILURE;
    };
    if prepare_lock(&selection, controls).is_none() {
        return ExitCode::FAILURE;
    }
    let target = native_target();
    for manifest in &selection.packages {
        let Some(package) = lower_package(
            manifest,
            RootBubbleSelection::All,
            &controls.features,
            &controls.platform_target,
            controls.registry_root.as_deref(),
            controls.offline(),
        ) else {
            return ExitCode::FAILURE;
        };
        if let Err(error) = resolve_native_link_inputs(&package.native_link_sources, &target) {
            tool_failure!("pop: {error}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

struct FileChange {
    path: PathBuf,
    expected: Vec<u8>,
    bytes: Vec<u8>,
    staging: PathBuf,
    backup: PathBuf,
}

fn format_manifest(manifest_path: &Path, check: bool) -> ExitCode {
    let Some(selection) = manifest_selection(manifest_path) else {
        return ExitCode::FAILURE;
    };
    let mut paths = BTreeSet::new();
    for manifest in selection.packages {
        let Some(package_root) = manifest.parent() else {
            tool_failure!("pop: selected Package manifest has no parent directory");
            return ExitCode::FAILURE;
        };
        let Ok(sources) = collect_package_sources(package_root) else {
            return ExitCode::FAILURE;
        };
        paths.extend(sources.into_values());
    }

    let mut changes = Vec::new();
    for (index, path) in paths.into_iter().enumerate() {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                tool_failure!("pop: could not read `{}`: {error}", path.display());
                return ExitCode::FAILURE;
            }
        };
        let Ok(file) = u32::try_from(index) else {
            tool_failure!("pop: too many Package sources to format");
            return ExitCode::FAILURE;
        };
        let source = match SourceFile::new(
            FileId::from_raw(file),
            path.to_string_lossy().into_owned(),
            text.clone(),
        ) {
            Ok(source) => source,
            Err(error) => {
                tool_failure!("pop: could not load `{}`: {error}", path.display());
                return ExitCode::FAILURE;
            }
        };
        let formatted = format_documentation_comments(&source);
        if formatted != text {
            let Some(parent) = path.parent() else {
                tool_failure!("pop: source path has no parent directory");
                return ExitCode::FAILURE;
            };
            let Some(name) = path.file_name().and_then(OsStr::to_str) else {
                tool_failure!("pop: source path is not valid UTF-8");
                return ExitCode::FAILURE;
            };
            let staging = parent.join(format!(
                ".{name}.pop-format-{}-{index}.staging",
                std::process::id()
            ));
            let backup = parent.join(format!(
                ".{name}.pop-format-{}-{index}.backup",
                std::process::id()
            ));
            changes.push(FileChange {
                path,
                expected: text.into_bytes(),
                bytes: formatted.into_bytes(),
                staging,
                backup,
            });
        }
    }

    if check {
        if changes.is_empty() {
            return ExitCode::SUCCESS;
        }
        tool_failure!(
            "pop: {} selected source file(s) require formatting",
            changes.len()
        );
        return ExitCode::FAILURE;
    }
    if changes.is_empty() {
        return ExitCode::SUCCESS;
    }
    if let Err(error) = publish_file_transaction(&changes, || true) {
        tool_failure!("pop: Package formatting transaction failed: {error}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn publish_file_transaction(
    changes: &[FileChange],
    postcondition: impl FnOnce() -> bool,
) -> Result<(), String> {
    for change in changes {
        if change.staging.exists() || change.backup.exists() {
            cleanup_format_staging(changes);
            return Err(format!(
                "refusing conflicting transaction paths beside `{}`",
                change.path.display()
            ));
        }
        let permissions = fs::metadata(&change.path)
            .map_err(|error| format!("could not inspect `{}`: {error}", change.path.display()))?
            .permissions();
        let mut staging = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&change.staging)
            .map_err(|error| {
                cleanup_format_staging(changes);
                format!("could not stage `{}`: {error}", change.path.display())
            })?;
        staging.write_all(&change.bytes).map_err(|error| {
            cleanup_format_staging(changes);
            format!("could not stage `{}`: {error}", change.path.display())
        })?;
        staging.sync_all().map_err(|error| {
            cleanup_format_staging(changes);
            format!("could not synchronize `{}`: {error}", change.path.display())
        })?;
        fs::set_permissions(&change.staging, permissions).map_err(|error| {
            cleanup_format_staging(changes);
            format!(
                "could not preserve permissions for `{}`: {error}",
                change.path.display()
            )
        })?;
    }

    for (index, change) in changes.iter().enumerate() {
        let current = match fs::read(&change.path) {
            Ok(current) => current,
            Err(error) => {
                rollback_file_transaction(changes, index);
                return Err(format!(
                    "could not re-read `{}` before publication: {error}",
                    change.path.display()
                ));
            }
        };
        if current != change.expected {
            rollback_file_transaction(changes, index);
            return Err(format!(
                "`{}` changed after the transaction snapshot",
                change.path.display()
            ));
        }
        if let Err(error) = fs::rename(&change.path, &change.backup) {
            rollback_file_transaction(changes, index);
            return Err(format!(
                "could not protect `{}` before publication: {error}",
                change.path.display()
            ));
        }
        if let Err(error) = fs::rename(&change.staging, &change.path) {
            let _ = fs::rename(&change.backup, &change.path);
            rollback_file_transaction(changes, index);
            return Err(format!(
                "could not publish `{}`: {error}",
                change.path.display()
            ));
        }
    }
    if !postcondition() {
        rollback_file_transaction(changes, changes.len());
        return Err("published candidates failed the transaction postcondition".to_owned());
    }
    for change in changes {
        fs::remove_file(&change.backup).map_err(|error| {
            format!(
                "formatted source was published but backup `{}` could not be removed: {error}",
                change.backup.display()
            )
        })?;
    }
    Ok(())
}

fn rollback_file_transaction(changes: &[FileChange], published: usize) {
    for change in changes[..published].iter().rev() {
        let _ = fs::remove_file(&change.path);
        let _ = fs::rename(&change.backup, &change.path);
    }
    cleanup_format_staging(changes);
}

fn cleanup_format_staging(changes: &[FileChange]) {
    for change in changes {
        if change.staging.is_file() {
            let _ = fs::remove_file(&change.staging);
        }
    }
}

fn build_manifest(
    manifest_path: &Path,
    controls: &ManifestControls,
    build_selection: &PackageBuildSelection,
) -> Option<Vec<PathBuf>> {
    let selection = manifest_selection(manifest_path)?;
    prepare_lock(&selection, controls)?;
    let resolution_root = selection.workspace_root.clone().unwrap_or_else(|| {
        selection.packages[0]
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    });
    let lock_bytes = fs::read(resolution_root.join("bubble.lock"))
        .map_err(|error| tool_failure!("pop: could not read build lock: {error}"))
        .ok()?;
    let cache_key = build_cache_key(
        &lock_bytes,
        controls,
        &build_selection.cache_identity(),
        "Development",
    );
    let cache_record_label = build_selection.cache_record_label();
    let shared_output = selection
        .workspace_root
        .as_ref()
        .map(|root| root.join("target/debug"));
    let mut executables = Vec::new();
    for manifest in selection.packages {
        let built = build_selected_package_to(
            &manifest,
            shared_output.as_deref(),
            build_selection.root_selection(),
            controls,
            &cache_key,
            &cache_record_label,
        )?;
        executables.extend(built.into_iter().filter(|executable| {
            build_selection.bubble_name().is_none_or(|selected| {
                executable.file_name().and_then(OsStr::to_str) == Some(selected)
            })
        }));
    }
    if let Some(selected) = build_selection.bubble_name()
        && executables.is_empty()
    {
        tool_failure!("pop: selected Bubble `{selected}` was not discovered");
        return None;
    }
    Some(executables)
}

fn document_manifest(manifest_path: &Path, controls: &ManifestControls) -> ExitCode {
    let Some(selection) = manifest_selection(manifest_path) else {
        return ExitCode::FAILURE;
    };
    if prepare_lock(&selection, controls).is_none() {
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
        let Some(package) = lower_package(
            &manifest,
            RootBubbleSelection::Ordinary,
            &controls.features,
            &controls.platform_target,
            controls.registry_root.as_deref(),
            controls.offline(),
        ) else {
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BuildCacheRecord {
    schema_version: u16,
    key: String,
    outputs: Vec<BuildCacheOutput>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BuildCacheOutput {
    path: String,
    size: u64,
    sha256: String,
    executable: bool,
}

fn build_cache_key(
    lock_bytes: &[u8],
    controls: &ManifestControls,
    selection: &str,
    profile: &str,
) -> String {
    let mut payload = Vec::new();
    append_hash_input(
        &mut payload,
        "compiler",
        env!("CARGO_PKG_VERSION").as_bytes(),
    );
    append_hash_input(&mut payload, "editionContract", b"2026");
    append_hash_input(
        &mut payload,
        "platformTarget",
        controls.platform_target.as_bytes(),
    );
    append_hash_input(&mut payload, "profile", profile.as_bytes());
    append_hash_input(&mut payload, "selection", selection.as_bytes());
    append_hash_input(&mut payload, "plriAbi", b"1");
    append_hash_input(&mut payload, "backend", b"llvm-native");
    append_hash_input(&mut payload, "bubble.lock", lock_bytes);
    sha256_hex(&payload)
}

fn emit_build_cache_event(package: &str, key: &str, status: &str) -> Result<(), ()> {
    if !presentation::is_json() {
        return Ok(());
    }
    presentation::write_json(&json!({
        "schemaVersion": 1,
        "kind": "buildCache",
        "package": package,
        "key": key,
        "status": status,
    }))
    .map_err(|error| {
        tool_failure!("pop: could not write structured build-cache event: {error}");
    })
}

fn load_build_cache(
    cache_path: &Path,
    output_root: &Path,
    expected_key: &str,
) -> Option<Vec<PathBuf>> {
    let bytes = fs::read(cache_path).ok()?;
    if bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    let record: BuildCacheRecord = serde_json::from_slice(&bytes).ok()?;
    if record.schema_version != 1 || record.key != expected_key {
        return None;
    }
    let mut canonical = serde_json::to_vec(&record).ok()?;
    canonical.push(b'\n');
    if canonical != bytes {
        return None;
    }
    if record
        .outputs
        .windows(2)
        .any(|pair| pair[0].path >= pair[1].path)
    {
        return None;
    }
    let mut executables = Vec::new();
    for output in &record.outputs {
        let path = resolve_cache_output(output_root, &output.path)?;
        let bytes = fs::read(&path).ok()?;
        if u64::try_from(bytes.len()).ok()? != output.size || sha256_hex(&bytes) != output.sha256 {
            return None;
        }
        if output.executable {
            executables.push(path);
        }
    }
    Some(executables)
}

fn resolve_cache_output(output_root: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        return None;
    }
    let mut path = output_root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return None;
        };
        path.push(component);
        let metadata = fs::symlink_metadata(&path).ok()?;
        if metadata.file_type().is_symlink() {
            return None;
        }
    }
    fs::symlink_metadata(&path)
        .ok()
        .is_some_and(|metadata| metadata.is_file())
        .then_some(path)
}

fn collect_cache_files(path: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), ()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| tool_failure!("pop: could not inspect cache output: {error}"))?;
    if metadata.file_type().is_symlink() {
        tool_failure!(
            "pop: refusing symlinked build-cache output `{}`",
            path.display()
        );
        return Err(());
    }
    if metadata.is_file() {
        files.insert(path.to_path_buf());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(());
    }
    let mut entries = fs::read_dir(path)
        .map_err(|error| tool_failure!("pop: could not inspect cache outputs: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| tool_failure!("pop: could not inspect cache outputs: {error}"))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        collect_cache_files(&entry.path(), files)?;
    }
    Ok(())
}

fn write_build_cache(
    cache_path: &Path,
    output_root: &Path,
    key: &str,
    files: &BTreeSet<PathBuf>,
    executables: &[PathBuf],
) -> Result<(), String> {
    let executable_set = executables.iter().collect::<BTreeSet<_>>();
    let mut outputs = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(output_root)
            .map_err(|_| "build-cache output escaped its root".to_owned())?
            .to_string_lossy()
            .replace('\\', "/");
        if resolve_cache_output(output_root, &relative).as_deref() != Some(path) {
            return Err(format!(
                "build-cache output `{}` is not a regular in-root file",
                path.display()
            ));
        }
        let bytes = fs::read(path).map_err(|error| error.to_string())?;
        outputs.push(BuildCacheOutput {
            path: relative,
            size: u64::try_from(bytes.len())
                .map_err(|_| "build-cache output is too large".to_owned())?,
            sha256: sha256_hex(&bytes),
            executable: executable_set.contains(path),
        });
    }
    outputs.sort_by(|left, right| left.path.cmp(&right.path));
    let record = BuildCacheRecord {
        schema_version: 1,
        key: key.to_owned(),
        outputs,
    };
    let mut bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("build-cache record is too large".to_owned());
    }
    let parent = cache_path
        .parent()
        .ok_or_else(|| "build-cache path has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let staging = parent.join(format!(
        ".cache-record-{}-{}.staging",
        std::process::id(),
        key
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .map_err(|error| error.to_string())?;
    let publish = (|| -> Result<(), std::io::Error> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&staging, cache_path)
    })();
    if let Err(error) = publish {
        let _ = fs::remove_file(&staging);
        return Err(error.to_string());
    }
    Ok(())
}

fn build_selected_package_to(
    manifest_path: &Path,
    selected_output_root: Option<&Path>,
    root_selection: RootBubbleSelection,
    controls: &ManifestControls,
    cache_key: &str,
    cache_record_label: &str,
) -> Option<Vec<PathBuf>> {
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let output_root = selected_output_root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| package_root.join("target/debug"));
    let manifest_text = fs::read_to_string(manifest_path).ok()?;
    let manifest = parse_package_manifest(&manifest_text)
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()?;
    let cache_path = output_root
        .join(".pop-cache")
        .join(format!("{}-{cache_record_label}.json", manifest.name()));
    if let Some(executables) = load_build_cache(&cache_path, &output_root, cache_key) {
        if emit_build_cache_event(manifest.name(), cache_key, "hit").is_err() {
            return None;
        }
        return Some(executables);
    }
    if emit_build_cache_event(manifest.name(), cache_key, "miss").is_err() {
        return None;
    }

    let package = lower_package(
        manifest_path,
        root_selection,
        &controls.features,
        &controls.platform_target,
        controls.registry_root.as_deref(),
        controls.offline(),
    )?;
    let selected_target = native_target();
    let native_link_resolution =
        resolve_native_link_inputs(&package.native_link_sources, &selected_target)
            .map_err(|error| tool_failure!("pop: {error}"))
            .ok()?;

    let dependency_root = output_root.join("deps");
    fs::create_dir_all(&dependency_root)
        .map_err(|error| tool_failure!("pop: could not create build output: {error}"))
        .ok()?;
    let mut library_objects = Vec::new();
    let mut binary_objects = Vec::new();
    let mut cache_files = BTreeSet::new();
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
            .with_required_capabilities(vec![
                "exceptions".to_owned(),
                "preciseStackMaps".to_owned(),
                "relocatingNursery".to_owned(),
                "threads".to_owned(),
            ])
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
            collect_cache_files(&artifact, &mut cache_files).ok()?;
            cache_files.insert(object.clone());
            library_objects.push(object);
        } else if bubble.root_package {
            cache_files.insert(object.clone());
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
        cache_files.insert(executable.clone());
        executables.push(executable);
    }
    write_build_cache(
        &cache_path,
        &output_root,
        cache_key,
        &cache_files,
        &executables,
    )
    .map_err(|error| tool_failure!("pop: could not publish build cache: {error}"))
    .ok()?;
    Some(executables)
}

fn run_manifest(
    manifest_path: &Path,
    controls: &ManifestControls,
    arguments: &[OsString],
) -> ExitCode {
    let Some(executables) =
        build_manifest(manifest_path, controls, &PackageBuildSelection::Ordinary)
    else {
        return ExitCode::FAILURE;
    };
    let [executable] = executables.as_slice() else {
        tool_failure!("pop: `pop run` requires exactly one discovered binary Bubble");
        return ExitCode::FAILURE;
    };
    execute_native(executable, arguments)
}

fn test_manifest(manifest_path: &Path, controls: &ManifestControls) -> ExitCode {
    run_auxiliary_manifest(
        manifest_path,
        controls,
        RootBubbleSelection::Tests,
        "test",
        "testResult",
    )
}

fn benchmark_manifest(manifest_path: &Path, controls: &ManifestControls) -> ExitCode {
    run_auxiliary_manifest(
        manifest_path,
        controls,
        RootBubbleSelection::Benchmarks,
        "benchmark",
        "benchmarkResult",
    )
}

fn run_auxiliary_manifest(
    manifest_path: &Path,
    controls: &ManifestControls,
    root_selection: RootBubbleSelection,
    command_name: &str,
    event_kind: &str,
) -> ExitCode {
    let Some(selection) = manifest_selection(manifest_path) else {
        return ExitCode::FAILURE;
    };
    if prepare_lock(&selection, controls).is_none() {
        return ExitCode::FAILURE;
    }
    let output_root = selection.workspace_root.clone().unwrap_or_else(|| {
        selection.packages[0]
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    });
    let lock_bytes = match fs::read(output_root.join("bubble.lock")) {
        Ok(bytes) => bytes,
        Err(error) => {
            tool_failure!("pop: could not read {command_name} lock: {error}");
            return ExitCode::FAILURE;
        }
    };
    let cache_key = build_cache_key(
        &lock_bytes,
        controls,
        command_name,
        if command_name == "test" {
            "Test"
        } else {
            "Benchmark"
        },
    );
    let output_root = output_root.join("target").join(command_name);
    let mut executables = Vec::new();
    for manifest in &selection.packages {
        let Some(package_executables) = build_selected_package_to(
            manifest,
            Some(&output_root),
            root_selection,
            controls,
            &cache_key,
            command_name,
        ) else {
            return ExitCode::FAILURE;
        };
        executables.extend(package_executables);
    }
    if executables.is_empty() {
        tool_failure!(
            "pop: `pop {command_name}` requires at least one discovered {command_name} Bubble"
        );
        return ExitCode::FAILURE;
    }

    let mut failed = false;
    for executable in executables {
        let bubble = executable
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("<invalid>");
        let status = Command::new(&executable).status();
        let (outcome, exit_code) = match status {
            Ok(status) if status.success() => ("success", 0),
            Ok(status) => {
                failed = true;
                (
                    "failure",
                    status
                        .code()
                        .and_then(|code| u8::try_from(code).ok())
                        .unwrap_or(1),
                )
            }
            Err(error) => {
                failed = true;
                tool_failure!(
                    "pop: could not execute {command_name} Bubble `{bubble}` at `{}`: {error}",
                    executable.display()
                );
                ("failure", 1)
            }
        };
        if presentation::is_json() {
            if let Err(error) = presentation::write_json(&json!({
                "schemaVersion": 1,
                "kind": event_kind,
                "bubble": bubble,
                "outcome": outcome,
                "exitCode": exit_code,
            })) {
                tool_failure!("pop: could not write structured {command_name} result: {error}");
                return ExitCode::FAILURE;
            }
        } else if outcome == "failure" {
            tool_failure!(
                "pop: {command_name} Bubble `{bubble}` failed with exit code {exit_code}"
            );
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
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
    source: LockedSource,
}

struct LockResolutionState {
    root: PathBuf,
    selected_roots: BTreeSet<PathBuf>,
    visiting: BTreeSet<PathBuf>,
    resolved: BTreeMap<PathBuf, ResolvedLockPackage>,
    packages: Vec<LockedPackage>,
    bubbles: Vec<LockedBubble>,
    registry_root: Option<PathBuf>,
    offline: bool,
}

fn prepare_lock(selection: &ManifestSelection, controls: &ManifestControls) -> Option<()> {
    manifest_native_target(controls)?;
    let root = selection.workspace_root.clone().unwrap_or_else(|| {
        selection.packages[0]
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    });
    let lock = resolve_selection_lock(selection, &root, controls)?;
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
    let changed = apply_lock_policy(existing.as_deref(), &proposed, controls.lock_mode, false)
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()?;
    if changed {
        write_lock_atomically(&lock_path, &proposed)?;
    }
    Some(())
}

fn resolve_selection_lock(
    selection: &ManifestSelection,
    root: &Path,
    controls: &ManifestControls,
) -> Option<BubbleLock> {
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
        registry_root: controls.registry_root.clone(),
        offline: controls.offline(),
    };
    let roots = state.selected_roots.iter().cloned().collect::<Vec<_>>();
    for manifest in roots {
        resolve_lock_package(
            &manifest,
            &controls.features,
            &controls.platform_target,
            None,
            &mut state,
        )?;
    }
    BubbleLock::new(
        "1",
        &controls.platform_target,
        state.packages,
        state.bubbles,
    )
    .map_err(|error| tool_failure!("pop: {error}"))
    .ok()
}

fn resolve_lock_package(
    manifest_path: &Path,
    selected_features: &[String],
    platform_target: &str,
    locked_source: Option<LockedSource>,
    state: &mut LockResolutionState,
) -> Option<ResolvedLockPackage> {
    let manifest_path = fs::canonicalize(manifest_path).ok()?;
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let source = if let Some(source) = locked_source {
        source
    } else {
        LockedSource::LocalPath(relative_resolution_path(&state.root, package_root)?)
    };
    if let Some(resolved) = state.resolved.get(&manifest_path) {
        if resolved.source != source {
            tool_failure!(
                "pop: Package `{}` resolved through conflicting source identities",
                resolved.name
            );
            return None;
        }
        return Some(resolved.clone());
    }
    if !state.visiting.insert(manifest_path.clone()) {
        tool_failure!(
            "pop: Package dependency cycle includes `{}`",
            manifest_path.display()
        );
        return None;
    }
    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|error| {
            tool_failure!("pop: could not read `{}`: {error}", manifest_path.display());
        })
        .ok()?;
    let manifest = parse_package_manifest(&manifest_text)
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()?;

    let mut external_libraries = Vec::new();
    let selected_dependencies = manifest
        .selected_dependencies_with_features(platform_target, false, selected_features)
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()?;
    for requirement in selected_dependencies {
        external_libraries.push(resolve_lock_dependency(
            requirement,
            package_root,
            platform_target,
            state,
        )?);
    }
    let selected_root = state.selected_roots.contains(&manifest_path);
    let mut development_libraries = Vec::new();
    if selected_root {
        manifest
            .selected_dependencies_with_features(platform_target, true, selected_features)
            .map_err(|error| tool_failure!("pop: {error}"))
            .ok()?;
        for requirement in manifest.development_dependencies() {
            development_libraries.push(resolve_lock_dependency(
                requirement,
                package_root,
                platform_target,
                state,
            )?);
        }
    }

    let source_paths = collect_package_sources(package_root).ok()?;
    let relative_paths = source_paths.keys().map(String::as_str).collect::<Vec<_>>();
    let discovered = discover_conventional_bubbles(&manifest, &relative_paths)
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()?;
    let mut library = None;
    for bubble in discovered
        .iter()
        .filter(|bubble| bubble.kind() == BubbleKind::Library || selected_root)
    {
        let identity = LockedBubbleIdentity::new(manifest.name(), bubble.name(), bubble.kind());
        let mut dependencies = external_libraries.clone();
        if matches!(
            bubble.kind(),
            BubbleKind::Test | BubbleKind::Example | BubbleKind::Benchmark
        ) {
            dependencies.extend(development_libraries.clone());
        }
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

    let content_hash = package_content_hash(&manifest_path, &source_paths)?;
    state.packages.push(
        LockedPackage::new(
            manifest.name(),
            manifest.version(),
            source.clone(),
            content_hash,
            if selected_root {
                selected_features.to_vec()
            } else {
                Vec::new()
            },
        )
        .map_err(|error| tool_failure!("pop: {error}"))
        .ok()?,
    );
    let resolved = ResolvedLockPackage {
        name: manifest.name().to_owned(),
        version: manifest.version().to_owned(),
        library,
        source,
    };
    state.visiting.remove(&manifest_path);
    state.resolved.insert(manifest_path, resolved.clone());
    Some(resolved)
}

fn resolve_lock_dependency(
    requirement: &DependencyRequirement,
    package_root: &Path,
    platform_target: &str,
    state: &mut LockResolutionState,
) -> Option<LockedBubbleIdentity> {
    let (requirement, dependency_root) =
        inherited_dependency_requirement(requirement, package_root)?;
    let location = resolve_dependency_location(
        &requirement,
        &dependency_root,
        &state.root,
        state.registry_root.as_deref(),
        state.offline,
    )?;
    let dependency = resolve_lock_package(
        &location.manifest_path,
        &[],
        platform_target,
        location.locked_source,
        state,
    )?;
    if requirement
        .version_requirement()
        .is_some_and(|required| required != dependency.version)
    {
        tool_failure!("pop: dependency version mismatch for `{}`", dependency.name);
        return None;
    }
    dependency.library.clone().or_else(|| {
        tool_failure!(
            "pop: dependency `{}` has no library Bubble",
            dependency.name
        );
        None
    })
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

fn dependency_resolution_root(manifest_path: &Path) -> PathBuf {
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    for ancestor in package_root.ancestors() {
        let candidate = ancestor.join("bubble.toml");
        if fs::read_to_string(&candidate)
            .ok()
            .is_some_and(|text| text.lines().any(|line| line.trim() == "[workspace]"))
        {
            return ancestor.to_path_buf();
        }
    }
    package_root.to_path_buf()
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

#[allow(clippy::too_many_arguments)]
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
    let diagnostic_sources = modules
        .iter()
        .map(|module| module.source().clone())
        .collect::<Vec<_>>();
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
    if !result.diagnostics().is_empty()
        && write_diagnostics(result.diagnostics(), &diagnostic_sources)
    {
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
