use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use pop_target::{CAbiScalarKind, PointerWidth, TargetSpec};
use sha2::{Digest, Sha256};

use super::{FfiGenerationError, FfiGenerationErrorKind};

const MAX_DESCRIPTOR_BYTES: usize = 4 * 1024 * 1024;
const MAX_DECLARATIONS: usize = 4_096;
const MAX_MEMBERS: usize = 256;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_TEXT_BYTES: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Descriptor {
    pub schema_version: u64,
    pub platform_target: String,
    pub producer_name: String,
    pub producer_version: String,
    pub output_namespace: String,
    pub binding_namespace: String,
    pub records: Vec<Record>,
    pub functions: Vec<Function>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Record {
    pub name: String,
    pub size: u64,
    pub alignment: u64,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Field {
    pub name: String,
    pub type_name: AbiType,
    pub offset: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Function {
    pub name: String,
    pub symbol: String,
    pub abi: ForeignAbi,
    pub nonblocking: bool,
    pub pointer_parameters: Vec<String>,
    pub result_ownership: Option<PointerOwnership>,
    pub callback_pairs: Vec<CallbackPair>,
    pub parameters: Vec<Parameter>,
    pub result: Option<AbiType>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CallbackPair {
    pub callback_parameter_index: u64,
    pub context_parameter_index: u64,
    pub lifetime: CallbackLifetime,
    pub abi: CallbackAbi,
    pub signature_fingerprint: String,
    pub thread: CallbackThread,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CallbackLifetime {
    CallScoped,
    Registered,
}

impl CallbackLifetime {
    const fn source_name(self) -> &'static str {
        match self {
            Self::CallScoped => "CallScoped",
            Self::Registered => "Registered",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CallbackAbi {
    C,
    System,
}

impl CallbackAbi {
    const fn source_name(self) -> &'static str {
        match self {
            Self::C => "C",
            Self::System => "System",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CallbackThread {
    CallingThread,
    AttachedThread,
}

impl CallbackThread {
    const fn source_name(self) -> &'static str {
        match self {
            Self::CallingThread => "CallingThread",
            Self::AttachedThread => "AttachedThread",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Parameter {
    pub name: String,
    pub type_name: AbiType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CallbackSignature {
    pub parameters: Vec<Parameter>,
    pub result: Option<AbiType>,
}

impl CallbackSignature {
    fn render(&self, output: &mut String) {
        output.push_str("function(");
        for (index, parameter) in self.parameters.iter().enumerate() {
            if index != 0 {
                output.push_str(", ");
            }
            write!(output, "{}: ", parameter.name).expect("String write");
            parameter.type_name.render(output);
        }
        output.push(')');
        if let Some(result) = &self.result {
            output.push_str(": ");
            result.render(output);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct GeneratedMetadata {
    pub descriptor: Descriptor,
    pub generator_version: String,
    pub parser_version: u64,
    pub alias: String,
    pub native_library: String,
    pub descriptor_path: String,
    pub descriptor_sha256: String,
    pub abi_fingerprint: String,
    pub source_path: String,
    pub source_size: u64,
    pub source_sha256: String,
    pub shim_path: String,
    pub shim_size: u64,
    pub shim_sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ForeignAbi {
    C,
    System,
    CUnwind,
}

impl ForeignAbi {
    const fn source_name(self) -> &'static str {
        match self {
            Self::C => "C",
            Self::System => "System",
            Self::CUnwind => "CUnwind",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PointerOwnership {
    Borrowed,
    Owned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum AbiType {
    Scalar(String),
    Record(String),
    CallbackContext,
    CallbackFunction(Box<CallbackSignature>),
    Pointer {
        constructor: PointerConstructor,
        element: Box<Self>,
    },
}

impl AbiType {
    pub fn render(&self, output: &mut String) {
        match self {
            Self::Scalar(name) | Self::Record(name) => output.push_str(name),
            Self::CallbackContext => output.push_str("Ffi.CallbackContext"),
            Self::CallbackFunction(signature) => {
                output.push_str("Ffi.Function<");
                signature.render(output);
                output.push('>');
            }
            Self::Pointer {
                constructor,
                element,
            } => {
                output.push_str(constructor.source_name());
                output.push('<');
                element.render(output);
                output.push('>');
            }
        }
    }

    pub const fn is_pointer(&self) -> bool {
        matches!(self, Self::Pointer { .. })
    }

    const fn is_callback_context(&self) -> bool {
        matches!(self, Self::CallbackContext)
    }

    const fn is_callback_function(&self) -> bool {
        matches!(self, Self::CallbackFunction(_))
    }

    fn contains_callback_type(&self) -> bool {
        match self {
            Self::CallbackContext | Self::CallbackFunction(_) => true,
            Self::Pointer { element, .. } => element.contains_callback_type(),
            Self::Scalar(_) | Self::Record(_) => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PointerConstructor {
    Mutable,
    OptionalMutable,
    ReadOnly,
    OptionalReadOnly,
}

impl PointerConstructor {
    const fn source_name(self) -> &'static str {
        match self {
            Self::Mutable => "Ffi.Pointer",
            Self::OptionalMutable => "Ffi.OptionalPointer",
            Self::ReadOnly => "Ffi.ReadOnlyPointer",
            Self::OptionalReadOnly => "Ffi.OptionalReadOnlyPointer",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Token {
    At,
    LeftParenthesis,
    RightParenthesis,
    LeftAngle,
    RightAngle,
    Comma,
    Equal,
    Colon,
    Dot,
    Identifier(String),
    String(String),
    Number(u64),
    End,
}

pub(super) fn parse_descriptor(
    bytes: &[u8],
    target: &TargetSpec,
) -> Result<Descriptor, FfiGenerationError> {
    if bytes.len() > MAX_DESCRIPTOR_BYTES {
        return Err(error(
            FfiGenerationErrorKind::ResourceLimit,
            "descriptor exceeds the 4 MiB schema limit",
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| {
        error(
            FfiGenerationErrorKind::InvalidDescriptor,
            "descriptor is not canonical UTF-8",
        )
    })?;
    let tokens = lex(text)?;
    let mut parser = Parser { tokens, cursor: 0 };
    let descriptor = parser.parse_descriptor()?;
    parser.expect(Token::End)?;
    validate_descriptor(&descriptor, target)?;
    if render_descriptor(&descriptor).as_bytes() != bytes {
        return Err(error(
            FfiGenerationErrorKind::InvalidDescriptor,
            "descriptor is not in canonical `.popc` form",
        ));
    }
    Ok(descriptor)
}

pub(super) fn parse_generated_metadata(
    bytes: &[u8],
    target: &TargetSpec,
) -> Result<GeneratedMetadata, FfiGenerationError> {
    if bytes.len() > MAX_DESCRIPTOR_BYTES {
        return Err(error(
            FfiGenerationErrorKind::ResourceLimit,
            "generated metadata exceeds the 4 MiB schema limit",
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| {
        error(
            FfiGenerationErrorKind::PublicationIo,
            "generated metadata is not UTF-8",
        )
    })?;
    let tokens = lex(text)?;
    let mut parser = Parser { tokens, cursor: 0 };
    parser.expect(Token::At)?;
    parser.expect_path("Ffi.GeneratedBindings")?;
    parser.expect(Token::LeftParenthesis)?;
    parser.expect_name("schemaVersion")?;
    parser.expect(Token::Equal)?;
    let schema_version = parser.number()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("generatorVersion")?;
    parser.expect(Token::Equal)?;
    let generator_version = parser.string()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("parserVersion")?;
    parser.expect(Token::Equal)?;
    let parser_version = parser.number()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("alias")?;
    parser.expect(Token::Equal)?;
    let alias = parser.string()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("platformTarget")?;
    parser.expect(Token::Equal)?;
    let platform_target = parser.string()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("nativeLibrary")?;
    parser.expect(Token::Equal)?;
    let native_library = parser.string()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("producerName")?;
    parser.expect(Token::Equal)?;
    let producer_name = parser.string()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("producerVersion")?;
    parser.expect(Token::Equal)?;
    let producer_version = parser.string()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("descriptorPath")?;
    parser.expect(Token::Equal)?;
    let descriptor_path = parser.string()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("descriptorSha256")?;
    parser.expect(Token::Equal)?;
    let descriptor_sha256 = parser.string()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("abiFingerprint")?;
    parser.expect(Token::Equal)?;
    let abi_fingerprint = parser.string()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("sourcePath")?;
    parser.expect(Token::Equal)?;
    let source_path = parser.string()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("sourceSize")?;
    parser.expect(Token::Equal)?;
    let source_size = parser.number()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("sourceSha256")?;
    parser.expect(Token::Equal)?;
    let source_sha256 = parser.string()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("shimPath")?;
    parser.expect(Token::Equal)?;
    let shim_path = parser.string()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("shimSize")?;
    parser.expect(Token::Equal)?;
    let shim_size = parser.number()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("shimSha256")?;
    parser.expect(Token::Equal)?;
    let shim_sha256 = parser.string()?;
    parser.expect(Token::Comma)?;
    parser.expect_name("outputNamespace")?;
    parser.expect(Token::Equal)?;
    let output_namespace = parser.path()?;
    parser.expect(Token::Comma)?;
    parser.expect(Token::RightParenthesis)?;
    parser.expect_name("namespace")?;
    let binding_namespace = parser.path()?;
    let (records, functions) = parser.parse_declarations()?;
    parser.expect(Token::End)?;
    let descriptor = Descriptor {
        schema_version,
        platform_target,
        producer_name,
        producer_version,
        output_namespace,
        binding_namespace,
        records,
        functions,
    };
    validate_descriptor(&descriptor, target)?;
    if !matches!((schema_version, parser_version), (1, 1) | (2, 2))
        || generator_version.is_empty()
        || !valid_pascal(&alias)
        || (!native_library.is_empty() && !valid_pascal(&native_library))
        || !valid_relative_path(&descriptor_path, ".popc")
        || source_path != "bindings.pop"
        || shim_path != "bindings.c"
        || !valid_sha256(&descriptor_sha256)
        || descriptor_sha256 != abi_fingerprint
        || !valid_sha256(&source_sha256)
        || !valid_sha256(&shim_sha256)
    {
        return Err(error(
            FfiGenerationErrorKind::PublicationIo,
            "generated metadata violates its closed typed contract",
        ));
    }
    Ok(GeneratedMetadata {
        descriptor,
        generator_version,
        parser_version,
        alias,
        native_library,
        descriptor_path,
        descriptor_sha256,
        abi_fingerprint,
        source_path,
        source_size,
        source_sha256,
        shim_path,
        shim_size,
        shim_sha256,
    })
}

fn lex(text: &str) -> Result<Vec<Token>, FfiGenerationError> {
    let mut tokens = Vec::new();
    let mut characters = text.char_indices().peekable();
    while let Some((offset, character)) = characters.next() {
        let token = match character {
            character if character.is_ascii_whitespace() => continue,
            '@' => Token::At,
            '(' => Token::LeftParenthesis,
            ')' => Token::RightParenthesis,
            '<' => Token::LeftAngle,
            '>' => Token::RightAngle,
            ',' => Token::Comma,
            '=' => Token::Equal,
            ':' => Token::Colon,
            '.' => Token::Dot,
            '0'..='9' => {
                let start = offset;
                let mut end = offset + character.len_utf8();
                while let Some((next, value)) = characters.peek().copied() {
                    if !value.is_ascii_digit() {
                        break;
                    }
                    characters.next();
                    end = next + value.len_utf8();
                }
                Token::Number(text[start..end].parse().map_err(|_| {
                    error(
                        FfiGenerationErrorKind::InvalidDescriptor,
                        "descriptor integer is out of range",
                    )
                })?)
            }
            'A'..='Z' | 'a'..='z' | '_' => {
                let start = offset;
                let mut end = offset + character.len_utf8();
                while let Some((next, value)) = characters.peek().copied() {
                    if !(value.is_ascii_alphanumeric() || value == '_') {
                        break;
                    }
                    characters.next();
                    end = next + value.len_utf8();
                }
                Token::Identifier(text[start..end].to_owned())
            }
            '"' => {
                let mut value = String::new();
                let mut closed = false;
                for (_, next) in characters.by_ref() {
                    if next == '"' {
                        closed = true;
                        break;
                    }
                    if next == '\\' || next.is_control() || !next.is_ascii() {
                        return Err(error(
                            FfiGenerationErrorKind::InvalidDescriptor,
                            "descriptor strings accept printable ASCII without escapes",
                        ));
                    }
                    value.push(next);
                    if value.len() > MAX_TEXT_BYTES {
                        return Err(error(
                            FfiGenerationErrorKind::ResourceLimit,
                            "descriptor string exceeds the schema limit",
                        ));
                    }
                }
                if !closed {
                    return Err(error(
                        FfiGenerationErrorKind::InvalidDescriptor,
                        "unterminated descriptor string",
                    ));
                }
                Token::String(value)
            }
            _ => {
                return Err(error(
                    FfiGenerationErrorKind::InvalidDescriptor,
                    format!("forbidden descriptor character at byte {offset}"),
                ));
            }
        };
        tokens.push(token);
        if tokens.len() > MAX_DESCRIPTOR_BYTES / 2 {
            return Err(error(
                FfiGenerationErrorKind::ResourceLimit,
                "descriptor token budget exhausted",
            ));
        }
    }
    tokens.push(Token::End);
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
}

impl Parser {
    fn parse_descriptor(&mut self) -> Result<Descriptor, FfiGenerationError> {
        self.expect(Token::At)?;
        self.expect_path("Ffi.Binding")?;
        self.expect(Token::LeftParenthesis)?;
        self.expect_name("schemaVersion")?;
        self.expect(Token::Equal)?;
        let schema_version = self.number()?;
        self.expect(Token::Comma)?;
        self.expect_name("platformTarget")?;
        self.expect(Token::Equal)?;
        let platform_target = self.string()?;
        self.expect(Token::Comma)?;
        self.expect_name("producerName")?;
        self.expect(Token::Equal)?;
        let producer_name = self.string()?;
        self.expect(Token::Comma)?;
        self.expect_name("producerVersion")?;
        self.expect(Token::Equal)?;
        let producer_version = self.string()?;
        self.expect(Token::Comma)?;
        self.expect_name("outputNamespace")?;
        self.expect(Token::Equal)?;
        let output_namespace = self.path()?;
        self.expect(Token::Comma)?;
        self.expect(Token::RightParenthesis)?;
        self.expect_name("namespace")?;
        let binding_namespace = self.path()?;

        let (records, functions) = self.parse_declarations()?;
        Ok(Descriptor {
            schema_version,
            platform_target,
            producer_name,
            producer_version,
            output_namespace,
            binding_namespace,
            records,
            functions,
        })
    }

    fn parse_declarations(&mut self) -> Result<(Vec<Record>, Vec<Function>), FfiGenerationError> {
        let mut records = Vec::new();
        let mut functions = Vec::new();
        while !matches!(self.peek(), Token::End) {
            self.expect(Token::At)?;
            let attribute = self.path()?;
            if attribute == "Ffi.C.Layout" {
                if !functions.is_empty() {
                    return Err(error(
                        FfiGenerationErrorKind::InvalidDescriptor,
                        "records must precede functions",
                    ));
                }
                records.push(self.parse_record()?);
                if records.len() > MAX_DECLARATIONS {
                    return Err(error(
                        FfiGenerationErrorKind::ResourceLimit,
                        "record count exceeds schema limit",
                    ));
                }
            } else if attribute == "Ffi.Foreign" {
                functions.push(self.parse_function()?);
                if functions.len() > MAX_DECLARATIONS {
                    return Err(error(
                        FfiGenerationErrorKind::ResourceLimit,
                        "function count exceeds schema limit",
                    ));
                }
            } else {
                return Err(error(
                    FfiGenerationErrorKind::InvalidDescriptor,
                    "unsupported descriptor declaration attribute",
                ));
            }
        }
        Ok((records, functions))
    }

    fn parse_record(&mut self) -> Result<Record, FfiGenerationError> {
        self.expect(Token::LeftParenthesis)?;
        self.expect_name("size")?;
        self.expect(Token::Equal)?;
        let size = self.number()?;
        self.expect(Token::Comma)?;
        self.expect_name("alignment")?;
        self.expect(Token::Equal)?;
        let alignment = self.number()?;
        self.expect(Token::RightParenthesis)?;
        self.expect_name("internal")?;
        self.expect_name("record")?;
        let name = self.identifier()?;
        let mut fields = Vec::new();
        while !self.peek_name("end") {
            self.expect(Token::At)?;
            self.expect_path("Ffi.C.Offset")?;
            self.expect(Token::LeftParenthesis)?;
            let offset = self.number()?;
            self.expect(Token::RightParenthesis)?;
            let field_name = self.identifier()?;
            self.expect(Token::Colon)?;
            let type_name = self.abi_type(0)?;
            fields.push(Field {
                name: field_name,
                type_name,
                offset,
            });
            if fields.len() > MAX_MEMBERS {
                return Err(error(
                    FfiGenerationErrorKind::ResourceLimit,
                    "record field count exceeds schema limit",
                ));
            }
        }
        self.expect_name("end")?;
        Ok(Record {
            name,
            size,
            alignment,
            fields,
        })
    }

    fn parse_function(&mut self) -> Result<Function, FfiGenerationError> {
        self.expect(Token::LeftParenthesis)?;
        let symbol = self.string()?;
        self.expect(Token::Comma)?;
        self.expect_name("abi")?;
        self.expect(Token::Equal)?;
        let abi = match self.string()?.as_str() {
            "C" => ForeignAbi::C,
            "System" => ForeignAbi::System,
            "CUnwind" => ForeignAbi::CUnwind,
            _ => {
                return Err(error(
                    FfiGenerationErrorKind::UnsupportedAbi,
                    "foreign ABI is outside the closed schema",
                ));
            }
        };
        self.expect(Token::RightParenthesis)?;
        self.expect(Token::At)?;
        self.expect_path("Ffi.Binding.CallPolicy")?;
        self.expect(Token::LeftParenthesis)?;
        self.expect_name("nonblocking")?;
        self.expect(Token::Equal)?;
        let nonblocking = self.boolean()?;
        self.expect(Token::RightParenthesis)?;

        let mut pointer_parameters = Vec::new();
        let mut result_ownership = None;
        let mut callback_pairs = Vec::new();
        while matches!(self.peek(), Token::At) {
            self.expect(Token::At)?;
            let attribute = self.path()?;
            self.expect(Token::LeftParenthesis)?;
            match attribute.as_str() {
                "Ffi.Binding.ParameterPointer" => {
                    self.expect_name("parameter")?;
                    self.expect(Token::Equal)?;
                    pointer_parameters.push(self.identifier()?);
                    self.expect(Token::Comma)?;
                    self.expect_name("retention")?;
                    self.expect(Token::Equal)?;
                    self.expect_path("Ffi.Binding.Retention.Call")?;
                }
                "Ffi.Binding.ResultPointer" => {
                    self.expect_name("ownership")?;
                    self.expect(Token::Equal)?;
                    let ownership = match self.path()?.as_str() {
                        "Ffi.Binding.Ownership.Borrowed" => PointerOwnership::Borrowed,
                        "Ffi.Binding.Ownership.Owned" => PointerOwnership::Owned,
                        _ => {
                            return Err(error(
                                FfiGenerationErrorKind::PolicyMismatch,
                                "pointer result ownership is not closed",
                            ));
                        }
                    };
                    if result_ownership.replace(ownership).is_some() {
                        return Err(error(
                            FfiGenerationErrorKind::PolicyMismatch,
                            "duplicate pointer result policy",
                        ));
                    }
                }
                "Ffi.Binding.CallbackPair" => {
                    callback_pairs.push(self.parse_callback_pair()?);
                    self.expect(Token::RightParenthesis)?;
                    continue;
                }
                _ => {
                    return Err(error(
                        FfiGenerationErrorKind::InvalidDescriptor,
                        "unsupported descriptor attribute",
                    ));
                }
            }
            self.expect(Token::RightParenthesis)?;
        }

        self.expect_name("internal")?;
        self.expect_name("function")?;
        let name = self.identifier()?;
        self.expect(Token::LeftParenthesis)?;
        let mut parameters = Vec::new();
        while !matches!(self.peek(), Token::RightParenthesis) {
            let parameter_name = self.identifier()?;
            self.expect(Token::Colon)?;
            let type_name = self.abi_type(0)?;
            parameters.push(Parameter {
                name: parameter_name,
                type_name,
            });
            if parameters.len() > MAX_MEMBERS {
                return Err(error(
                    FfiGenerationErrorKind::ResourceLimit,
                    "function parameter count exceeds schema limit",
                ));
            }
            self.expect(Token::Comma)?;
        }
        self.expect(Token::RightParenthesis)?;
        let result = if matches!(self.peek(), Token::Colon) {
            self.expect(Token::Colon)?;
            Some(self.abi_type(0)?)
        } else {
            None
        };
        self.expect_name("end")?;
        Ok(Function {
            name,
            symbol,
            abi,
            nonblocking,
            pointer_parameters,
            result_ownership,
            callback_pairs,
            parameters,
            result,
        })
    }

    fn parse_callback_pair(&mut self) -> Result<CallbackPair, FfiGenerationError> {
        self.expect_name("callbackParameterIndex")?;
        self.expect(Token::Equal)?;
        let callback_parameter_index = self.number()?;
        self.expect(Token::Comma)?;
        self.expect_name("contextParameterIndex")?;
        self.expect(Token::Equal)?;
        let context_parameter_index = self.number()?;
        self.expect(Token::Comma)?;
        self.expect_name("lifetime")?;
        self.expect(Token::Equal)?;
        let lifetime = match self.path()?.as_str() {
            "Ffi.Binding.CallbackLifetime.CallScoped" => CallbackLifetime::CallScoped,
            "Ffi.Binding.CallbackLifetime.Registered" => CallbackLifetime::Registered,
            _ => {
                return Err(error(
                    FfiGenerationErrorKind::PolicyMismatch,
                    "callback lifetime is outside the closed policy",
                ));
            }
        };
        self.expect(Token::Comma)?;
        self.expect_name("callbackAbi")?;
        self.expect(Token::Equal)?;
        let abi = match self.path()?.as_str() {
            "Ffi.Binding.CallbackAbi.C" => CallbackAbi::C,
            "Ffi.Binding.CallbackAbi.System" => CallbackAbi::System,
            _ => {
                return Err(error(
                    FfiGenerationErrorKind::UnsupportedAbi,
                    "callback ABI is outside the closed policy",
                ));
            }
        };
        self.expect(Token::Comma)?;
        self.expect_name("signatureFingerprint")?;
        self.expect(Token::Equal)?;
        let signature_fingerprint = self.string()?;
        self.expect(Token::Comma)?;
        self.expect_name("thread")?;
        self.expect(Token::Equal)?;
        let thread = match self.path()?.as_str() {
            "Ffi.Binding.CallbackThread.CallingThread" => CallbackThread::CallingThread,
            "Ffi.Binding.CallbackThread.AttachedThread" => CallbackThread::AttachedThread,
            _ => {
                return Err(error(
                    FfiGenerationErrorKind::PolicyMismatch,
                    "callback thread is outside the closed policy",
                ));
            }
        };
        self.expect(Token::Comma)?;
        self.expect_name("concurrency")?;
        self.expect(Token::Equal)?;
        self.expect_path("Ffi.Binding.CallbackConcurrency.Serialized")?;
        self.expect(Token::Comma)?;
        self.expect_name("reentrancy")?;
        self.expect(Token::Equal)?;
        self.expect_path("Ffi.Binding.CallbackReentrancy.Forbidden")?;
        self.expect(Token::Comma)?;
        self.expect_name("panicPolicy")?;
        self.expect(Token::Equal)?;
        self.expect_path("Ffi.Binding.CallbackPanic.AbortProcess")?;
        self.expect(Token::Comma)?;
        Ok(CallbackPair {
            callback_parameter_index,
            context_parameter_index,
            lifetime,
            abi,
            signature_fingerprint,
            thread,
        })
    }

    fn abi_type(&mut self, depth: usize) -> Result<AbiType, FfiGenerationError> {
        self.abi_type_with_callback_context(depth, false)
    }

    fn abi_type_with_callback_context(
        &mut self,
        depth: usize,
        inside_callback: bool,
    ) -> Result<AbiType, FfiGenerationError> {
        if depth > 1 {
            return Err(error(
                FfiGenerationErrorKind::UnsupportedAbi,
                "nested pointers are unsupported in descriptor schema 1",
            ));
        }
        let name = self.path()?;
        if name == "Ffi.Function" {
            if inside_callback || depth != 0 {
                return Err(error(
                    FfiGenerationErrorKind::UnsupportedAbi,
                    "nested callback function types are unsupported",
                ));
            }
            self.expect(Token::LeftAngle)?;
            let signature = self.callback_signature()?;
            self.expect(Token::RightAngle)?;
            return Ok(AbiType::CallbackFunction(Box::new(signature)));
        }
        if matches!(self.peek(), Token::LeftAngle) {
            let constructor = match name.as_str() {
                "Ffi.Pointer" => PointerConstructor::Mutable,
                "Ffi.OptionalPointer" => PointerConstructor::OptionalMutable,
                "Ffi.ReadOnlyPointer" => PointerConstructor::ReadOnly,
                "Ffi.OptionalReadOnlyPointer" => PointerConstructor::OptionalReadOnly,
                _ => {
                    return Err(error(
                        FfiGenerationErrorKind::UnsupportedAbi,
                        "generic ABI type is unsupported",
                    ));
                }
            };
            self.expect(Token::LeftAngle)?;
            let element = self.abi_type_with_callback_context(depth + 1, inside_callback)?;
            self.expect(Token::RightAngle)?;
            if element.is_pointer() || element.contains_callback_type() {
                return Err(error(
                    FfiGenerationErrorKind::UnsupportedAbi,
                    "nested pointers are unsupported in descriptor schema 1",
                ));
            }
            return Ok(AbiType::Pointer {
                constructor,
                element: Box::new(element),
            });
        }
        if name == "Ffi.CallbackContext" {
            return Ok(AbiType::CallbackContext);
        }
        if scalar_layout_name(&name).is_some() {
            Ok(AbiType::Scalar(name))
        } else if name.contains('.') {
            Err(error(
                FfiGenerationErrorKind::UnsupportedAbi,
                "ABI type is outside the closed scalar and record set",
            ))
        } else {
            Ok(AbiType::Record(name))
        }
    }

    fn callback_signature(&mut self) -> Result<CallbackSignature, FfiGenerationError> {
        self.expect_name("function")?;
        self.expect(Token::LeftParenthesis)?;
        let mut parameters = Vec::new();
        while !matches!(self.peek(), Token::RightParenthesis) {
            let name = self.identifier()?;
            self.expect(Token::Colon)?;
            let type_name = self.abi_type_with_callback_context(0, true)?;
            parameters.push(Parameter { name, type_name });
            if parameters.len() > MAX_MEMBERS {
                return Err(error(
                    FfiGenerationErrorKind::ResourceLimit,
                    "callback parameter count exceeds schema limit",
                ));
            }
            if !matches!(self.peek(), Token::Comma) {
                break;
            }
            self.expect(Token::Comma)?;
        }
        self.expect(Token::RightParenthesis)?;
        let result = if matches!(self.peek(), Token::Colon) {
            self.expect(Token::Colon)?;
            Some(self.abi_type_with_callback_context(0, true)?)
        } else {
            None
        };
        Ok(CallbackSignature { parameters, result })
    }

    fn expect_path(&mut self, expected: &str) -> Result<(), FfiGenerationError> {
        let actual = self.path()?;
        if actual == expected {
            Ok(())
        } else {
            Err(error(
                FfiGenerationErrorKind::InvalidDescriptor,
                format!("expected `{expected}`, found `{actual}`"),
            ))
        }
    }

    fn path(&mut self) -> Result<String, FfiGenerationError> {
        let mut path = self.identifier()?;
        while matches!(self.peek(), Token::Dot) {
            self.expect(Token::Dot)?;
            path.push('.');
            path.push_str(&self.identifier()?);
        }
        Ok(path)
    }

    fn identifier(&mut self) -> Result<String, FfiGenerationError> {
        match self.next() {
            Token::Identifier(value) if value.len() <= MAX_IDENTIFIER_BYTES => Ok(value),
            _ => Err(error(
                FfiGenerationErrorKind::InvalidDescriptor,
                "expected bounded descriptor identifier",
            )),
        }
    }

    fn string(&mut self) -> Result<String, FfiGenerationError> {
        match self.next() {
            Token::String(value) => Ok(value),
            _ => Err(error(
                FfiGenerationErrorKind::InvalidDescriptor,
                "expected descriptor string",
            )),
        }
    }

    fn number(&mut self) -> Result<u64, FfiGenerationError> {
        match self.next() {
            Token::Number(value) => Ok(value),
            _ => Err(error(
                FfiGenerationErrorKind::InvalidDescriptor,
                "expected descriptor unsigned integer",
            )),
        }
    }

    fn boolean(&mut self) -> Result<bool, FfiGenerationError> {
        match self.next() {
            Token::Identifier(value) if value == "true" => Ok(true),
            Token::Identifier(value) if value == "false" => Ok(false),
            _ => Err(error(
                FfiGenerationErrorKind::InvalidDescriptor,
                "expected descriptor Boolean",
            )),
        }
    }

    fn expect_name(&mut self, expected: &str) -> Result<(), FfiGenerationError> {
        match self.next() {
            Token::Identifier(value) if value == expected => Ok(()),
            _ => Err(error(
                FfiGenerationErrorKind::InvalidDescriptor,
                format!("expected `{expected}`"),
            )),
        }
    }

    fn peek_name(&self, expected: &str) -> bool {
        matches!(self.peek(), Token::Identifier(value) if value == expected)
    }

    #[allow(clippy::needless_pass_by_value)] // Keeps closed-grammar call sites token-shaped.
    fn expect(&mut self, expected: Token) -> Result<(), FfiGenerationError> {
        let actual = self.next();
        if actual == expected {
            Ok(())
        } else {
            Err(error(
                FfiGenerationErrorKind::InvalidDescriptor,
                "descriptor does not match the closed grammar",
            ))
        }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.cursor).unwrap_or(&Token::End)
    }

    fn next(&mut self) -> Token {
        let token = self.tokens.get(self.cursor).cloned().unwrap_or(Token::End);
        self.cursor = self.cursor.saturating_add(1);
        token
    }
}

fn validate_descriptor(
    descriptor: &Descriptor,
    target: &TargetSpec,
) -> Result<(), FfiGenerationError> {
    if !matches!(descriptor.schema_version, 1 | 2) {
        return Err(error(
            FfiGenerationErrorKind::InvalidDescriptor,
            "unsupported `.popc` schema version",
        ));
    }
    if descriptor.platform_target != target.triple() {
        return Err(error(
            FfiGenerationErrorKind::PolicyMismatch,
            "descriptor platform target does not match command selection",
        ));
    }
    if !valid_qualified_pascal(&descriptor.output_namespace)
        || !descriptor.output_namespace.ends_with(".Unsafe")
        || !valid_qualified_pascal(&descriptor.binding_namespace)
        || !valid_producer(&descriptor.producer_name)
        || !valid_producer(&descriptor.producer_version)
    {
        return Err(error(
            FfiGenerationErrorKind::InvalidDescriptor,
            "descriptor header identities are invalid",
        ));
    }
    let mut layouts = BTreeMap::new();
    let mut last_record = None;
    for record in &descriptor.records {
        if !valid_pascal(&record.name)
            || record.name == "Ffi"
            || scalar_layout_name(&record.name).is_some()
            || last_record.is_some_and(|last| last >= record.name.as_str())
            || record.fields.is_empty()
        {
            return Err(error(
                FfiGenerationErrorKind::InvalidDescriptor,
                "records must have unique sorted PascalCase identities and fields",
            ));
        }
        last_record = Some(record.name.as_str());
        if record
            .fields
            .iter()
            .any(|field| field.type_name.contains_callback_type())
        {
            return Err(error(
                FfiGenerationErrorKind::UnsupportedAbi,
                "callback types cannot appear in record storage",
            ));
        }
        let layout = validate_record_layout(record, target, &layouts)?;
        layouts.insert(record.name.clone(), layout);
    }
    let record_definitions = descriptor
        .records
        .iter()
        .map(|record| (record.name.as_str(), record))
        .collect::<BTreeMap<_, _>>();

    let mut last_function = None;
    let mut symbols = BTreeSet::new();
    for function in &descriptor.functions {
        if !valid_camel(&function.name)
            || last_function.is_some_and(|last| last >= function.name.as_str())
            || !valid_symbol(&function.symbol)
            || !symbols.insert(function.symbol.as_str())
        {
            return Err(error(
                FfiGenerationErrorKind::InvalidDescriptor,
                "functions and symbols must have unique sorted validated identities",
            ));
        }
        last_function = Some(function.name.as_str());
        let has_callback_types = function
            .parameters
            .iter()
            .any(|parameter| parameter.type_name.contains_callback_type())
            || function
                .result
                .as_ref()
                .is_some_and(AbiType::contains_callback_type);
        if descriptor.schema_version == 1
            && (has_callback_types || !function.callback_pairs.is_empty())
        {
            return Err(error(
                FfiGenerationErrorKind::UnsupportedAbi,
                "callbacks require `.popc` descriptor schema 2",
            ));
        }
        let mut parameter_names = BTreeSet::new();
        for parameter in &function.parameters {
            if !valid_camel(&parameter.name) || !parameter_names.insert(parameter.name.as_str()) {
                return Err(error(
                    FfiGenerationErrorKind::InvalidDescriptor,
                    "function parameters require unique camelCase identities",
                ));
            }
            validate_type(&parameter.type_name, &layouts, target)?;
        }
        if let Some(result) = &function.result {
            if result.contains_callback_type() {
                return Err(error(
                    FfiGenerationErrorKind::UnsupportedAbi,
                    "foreign callback and context results are unsupported",
                ));
            }
            validate_type(result, &layouts, target)?;
        }
        let actual_pointers = function
            .parameters
            .iter()
            .filter(|parameter| parameter.type_name.is_pointer())
            .map(|parameter| parameter.name.as_str())
            .collect::<Vec<_>>();
        if function
            .pointer_parameters
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != actual_pointers
            || function.result.as_ref().is_some_and(AbiType::is_pointer)
                != function.result_ownership.is_some()
        {
            return Err(error(
                FfiGenerationErrorKind::PolicyMismatch,
                "pointer policy does not exactly cover the static signature",
            ));
        }
        validate_callback_pairs(function, descriptor, &layouts, &record_definitions, target)?;
    }
    Ok(())
}

fn validate_callback_pairs(
    function: &Function,
    descriptor: &Descriptor,
    layouts: &BTreeMap<String, (u64, u64)>,
    records: &BTreeMap<&str, &Record>,
    target: &TargetSpec,
) -> Result<(), FfiGenerationError> {
    let mut callback_indices = BTreeSet::new();
    let mut context_indices = BTreeSet::new();
    let mut last_callback_index = None;
    for pair in &function.callback_pairs {
        if function.nonblocking {
            return Err(error(
                FfiGenerationErrorKind::PolicyMismatch,
                "callback-bearing foreign declarations must be blocking",
            ));
        }
        if target.pointer_width() != PointerWidth::Bits64 {
            return Err(error(
                FfiGenerationErrorKind::UnsupportedAbi,
                "callbacks require an exact 64-bit target pointer ABI",
            ));
        }
        if last_callback_index.is_some_and(|last| last >= pair.callback_parameter_index) {
            return Err(error(
                FfiGenerationErrorKind::PolicyMismatch,
                "callback-pair attachments must be sorted by unique callback index",
            ));
        }
        last_callback_index = Some(pair.callback_parameter_index);
        let callback_index = usize::try_from(pair.callback_parameter_index).map_err(|_| {
            error(
                FfiGenerationErrorKind::ResourceLimit,
                "callback parameter index exceeds the schema limit",
            )
        })?;
        let context_index = usize::try_from(pair.context_parameter_index).map_err(|_| {
            error(
                FfiGenerationErrorKind::ResourceLimit,
                "callback context index exceeds the schema limit",
            )
        })?;
        if callback_index == context_index
            || !callback_indices.insert(callback_index)
            || !context_indices.insert(context_index)
            || callback_indices.contains(&context_index)
            || context_indices.contains(&callback_index)
        {
            return Err(error(
                FfiGenerationErrorKind::PolicyMismatch,
                "callback-pair parameter indices overlap or are duplicated",
            ));
        }
        let Some(callback) = function.parameters.get(callback_index) else {
            return Err(error(
                FfiGenerationErrorKind::PolicyMismatch,
                "callback parameter index is out of range",
            ));
        };
        let Some(context) = function.parameters.get(context_index) else {
            return Err(error(
                FfiGenerationErrorKind::PolicyMismatch,
                "callback context parameter index is out of range",
            ));
        };
        let AbiType::CallbackFunction(signature) = &callback.type_name else {
            return Err(error(
                FfiGenerationErrorKind::PolicyMismatch,
                "callback index does not name Ffi.Function<TSignature>",
            ));
        };
        if !context.type_name.is_callback_context() {
            return Err(error(
                FfiGenerationErrorKind::PolicyMismatch,
                "callback context index does not name Ffi.CallbackContext",
            ));
        }
        validate_callback_signature(signature, layouts, target)?;
        if !valid_sha256(&pair.signature_fingerprint)
            || pair.signature_fingerprint
                != callback_signature_fingerprint(
                    signature,
                    pair.abi,
                    &descriptor.platform_target,
                    records,
                    target,
                )?
        {
            return Err(error(
                FfiGenerationErrorKind::PolicyMismatch,
                "callback signature fingerprint does not match the typed signature",
            ));
        }
        if !matches!(
            (pair.lifetime, pair.thread),
            (CallbackLifetime::CallScoped, CallbackThread::CallingThread)
                | (CallbackLifetime::Registered, CallbackThread::AttachedThread)
        ) {
            return Err(error(
                FfiGenerationErrorKind::PolicyMismatch,
                "callback lifetime and thread policy do not match the stable mapping",
            ));
        }
    }

    for (index, parameter) in function.parameters.iter().enumerate() {
        if parameter.type_name.is_callback_function() != callback_indices.contains(&index)
            || parameter.type_name.is_callback_context() != context_indices.contains(&index)
        {
            return Err(error(
                FfiGenerationErrorKind::PolicyMismatch,
                "callback-pair metadata does not exactly cover the static signature",
            ));
        }
    }
    Ok(())
}

fn validate_callback_signature(
    signature: &CallbackSignature,
    layouts: &BTreeMap<String, (u64, u64)>,
    target: &TargetSpec,
) -> Result<(), FfiGenerationError> {
    let mut names = BTreeSet::new();
    let mut contexts = 0_usize;
    for parameter in &signature.parameters {
        if !valid_camel(&parameter.name) || !names.insert(parameter.name.as_str()) {
            return Err(error(
                FfiGenerationErrorKind::InvalidDescriptor,
                "callback parameters require unique camelCase identities",
            ));
        }
        if parameter.type_name.is_callback_function() {
            return Err(error(
                FfiGenerationErrorKind::UnsupportedAbi,
                "nested callback function types are unsupported",
            ));
        }
        if parameter.type_name.is_callback_context() {
            contexts = contexts.saturating_add(1);
        }
        validate_type(&parameter.type_name, layouts, target)?;
    }
    if contexts != 1 {
        return Err(error(
            FfiGenerationErrorKind::PolicyMismatch,
            "callback signature requires exactly one Ffi.CallbackContext parameter",
        ));
    }
    if let Some(result) = &signature.result {
        if result.contains_callback_type() {
            return Err(error(
                FfiGenerationErrorKind::UnsupportedAbi,
                "callback results cannot contain callback or context types",
            ));
        }
        validate_type(result, layouts, target)?;
    }
    Ok(())
}

fn callback_signature_fingerprint(
    signature: &CallbackSignature,
    abi: CallbackAbi,
    platform_target: &str,
    records: &BTreeMap<&str, &Record>,
    target: &TargetSpec,
) -> Result<String, FfiGenerationError> {
    let mut descriptor = String::from("Pop.Ffi.CallbackSignature/1\n");
    writeln!(descriptor, "platformTarget={platform_target}").expect("String write");
    writeln!(descriptor, "abi={}", abi.source_name()).expect("String write");
    writeln!(descriptor, "parameterCount={}", signature.parameters.len()).expect("String write");
    for (index, parameter) in signature.parameters.iter().enumerate() {
        let layout = callback_abi_layout(&parameter.type_name, records, target)?;
        writeln!(descriptor, "parameter[{index}]={layout}").expect("String write");
    }
    if let Some(result) = &signature.result {
        descriptor.push_str("resultCount=1\n");
        let layout = callback_abi_layout(result, records, target)?;
        writeln!(descriptor, "result[0]={layout}").expect("String write");
    } else {
        descriptor.push_str("resultCount=0\n");
    }
    Ok(sha256_hex(descriptor.as_bytes()))
}

fn callback_abi_layout(
    type_name: &AbiType,
    records: &BTreeMap<&str, &Record>,
    target: &TargetSpec,
) -> Result<String, FfiGenerationError> {
    let mut active_records = BTreeSet::new();
    callback_abi_layout_inner(type_name, records, target, &mut active_records)
}

fn callback_abi_layout_inner(
    type_name: &AbiType,
    records: &BTreeMap<&str, &Record>,
    target: &TargetSpec,
    active_records: &mut BTreeSet<String>,
) -> Result<String, FfiGenerationError> {
    match type_name {
        AbiType::Scalar(name) => {
            let (size, alignment) = scalar_layout(name, target).ok_or_else(|| {
                error(
                    FfiGenerationErrorKind::UnsupportedAbi,
                    format!("target does not support ABI scalar `{name}`"),
                )
            })?;
            Ok(format!("{name}(size={size},alignment={alignment})"))
        }
        AbiType::Record(name) => {
            let record = records.get(name.as_str()).ok_or_else(|| {
                error(
                    FfiGenerationErrorKind::UnsupportedAbi,
                    format!("record `{name}` is not declared before use"),
                )
            })?;
            if !active_records.insert(name.clone()) {
                return Err(error(
                    FfiGenerationErrorKind::UnsupportedAbi,
                    "recursive by-value callback record layout is unsupported",
                ));
            }
            let mut output = format!(
                "record(size={},alignment={},fields=[",
                record.size, record.alignment
            );
            for (index, field) in record.fields.iter().enumerate() {
                if index != 0 {
                    output.push(';');
                }
                let layout =
                    callback_abi_layout_inner(&field.type_name, records, target, active_records)?;
                write!(output, "{}@{}:{layout}", field.name, field.offset).expect("String write");
            }
            output.push_str("])");
            active_records.remove(name);
            Ok(output)
        }
        AbiType::Pointer {
            constructor,
            element,
        } => {
            let (size, alignment) = target.ffi_pointer_layout().ok_or_else(|| {
                error(
                    FfiGenerationErrorKind::UnsupportedAbi,
                    "target does not support FFI pointers",
                )
            })?;
            let element = callback_abi_layout_inner(element, records, target, active_records)?;
            Ok(format!(
                "{}<{element}>(size={size},alignment={alignment})",
                constructor.source_name()
            ))
        }
        AbiType::CallbackContext => Ok("Ffi.CallbackContext(pointerWidth=64)".to_owned()),
        AbiType::CallbackFunction(_) => Err(error(
            FfiGenerationErrorKind::UnsupportedAbi,
            "nested callback function types are unsupported",
        )),
    }
}

fn validate_record_layout(
    record: &Record,
    target: &TargetSpec,
    records: &BTreeMap<String, (u64, u64)>,
) -> Result<(u64, u64), FfiGenerationError> {
    if record.size == 0 || !record.alignment.is_power_of_two() {
        return Err(error(
            FfiGenerationErrorKind::PolicyMismatch,
            format!("record `{}` has invalid geometry", record.name),
        ));
    }
    let mut cursor = 0_u64;
    let mut alignment = 1_u64;
    let mut field_names = BTreeSet::new();
    for field in &record.fields {
        if !valid_camel(&field.name) || !field_names.insert(field.name.as_str()) {
            return Err(error(
                FfiGenerationErrorKind::InvalidDescriptor,
                format!("record `{}` has invalid field identities", record.name),
            ));
        }
        let (field_size, field_alignment) = type_layout(&field.type_name, records, target)?;
        let expected_offset = align_up(cursor, field_alignment)?;
        if field.offset != expected_offset {
            return Err(error(
                FfiGenerationErrorKind::PolicyMismatch,
                format!(
                    "record `{}` field `{}` offset {} does not match target ABI offset {expected_offset}",
                    record.name, field.name, field.offset
                ),
            ));
        }
        cursor = field
            .offset
            .checked_add(field_size)
            .ok_or_else(|| error(FfiGenerationErrorKind::ResourceLimit, "layout overflow"))?;
        alignment = alignment.max(field_alignment);
    }
    let expected_size = align_up(cursor, alignment)?;
    if record.alignment != alignment || record.size != expected_size {
        return Err(error(
            FfiGenerationErrorKind::PolicyMismatch,
            format!(
                "record `{}` declares size/alignment {}/{}, target ABI requires {expected_size}/{alignment}",
                record.name, record.size, record.alignment
            ),
        ));
    }
    Ok((record.size, record.alignment))
}

fn validate_type(
    type_name: &AbiType,
    records: &BTreeMap<String, (u64, u64)>,
    target: &TargetSpec,
) -> Result<(), FfiGenerationError> {
    type_layout(type_name, records, target).map(|_| ())
}

fn type_layout(
    type_name: &AbiType,
    records: &BTreeMap<String, (u64, u64)>,
    target: &TargetSpec,
) -> Result<(u64, u64), FfiGenerationError> {
    match type_name {
        AbiType::Scalar(name) => scalar_layout(name, target).ok_or_else(|| {
            error(
                FfiGenerationErrorKind::UnsupportedAbi,
                format!("target does not support ABI scalar `{name}`"),
            )
        }),
        AbiType::Record(name) => records.get(name).copied().ok_or_else(|| {
            error(
                FfiGenerationErrorKind::UnsupportedAbi,
                format!("record `{name}` is not declared before use"),
            )
        }),
        AbiType::CallbackContext | AbiType::CallbackFunction(_) => {
            if target.pointer_width() != PointerWidth::Bits64 {
                return Err(error(
                    FfiGenerationErrorKind::UnsupportedAbi,
                    "callbacks require an exact 64-bit target pointer ABI",
                ));
            }
            target.ffi_pointer_layout().ok_or_else(|| {
                error(
                    FfiGenerationErrorKind::UnsupportedAbi,
                    "target does not support FFI callback pointers",
                )
            })
        }
        AbiType::Pointer { .. } => target.ffi_pointer_layout().ok_or_else(|| {
            error(
                FfiGenerationErrorKind::UnsupportedAbi,
                "target does not support FFI pointers",
            )
        }),
    }
}

fn scalar_layout(name: &str, target: &TargetSpec) -> Option<(u64, u64)> {
    scalar_layout_name(name).and_then(|layout| match layout {
        ScalarLayout::Fixed(size) => Some((size, size)),
        ScalarLayout::C(kind) => target
            .c_abi_scalar_layout(kind)
            .map(|layout| (layout.size(), layout.alignment())),
    })
}

#[derive(Clone, Copy)]
enum ScalarLayout {
    Fixed(u64),
    C(CAbiScalarKind),
}

fn scalar_layout_name(name: &str) -> Option<ScalarLayout> {
    Some(match name {
        "Byte" | "Int8" | "UInt8" => ScalarLayout::Fixed(1),
        "Int16" | "UInt16" => ScalarLayout::Fixed(2),
        "Int32" | "UInt32" | "Float32" => ScalarLayout::Fixed(4),
        "Int64" | "UInt64" | "Float64" => ScalarLayout::Fixed(8),
        "Ffi.C.Char" => ScalarLayout::C(CAbiScalarKind::Char),
        "Ffi.C.SignedChar" => ScalarLayout::C(CAbiScalarKind::SignedChar),
        "Ffi.C.UnsignedChar" => ScalarLayout::C(CAbiScalarKind::UnsignedChar),
        "Ffi.C.Short" => ScalarLayout::C(CAbiScalarKind::Short),
        "Ffi.C.UnsignedShort" => ScalarLayout::C(CAbiScalarKind::UnsignedShort),
        "Ffi.C.Int" => ScalarLayout::C(CAbiScalarKind::Int),
        "Ffi.C.UnsignedInt" => ScalarLayout::C(CAbiScalarKind::UnsignedInt),
        "Ffi.C.Long" => ScalarLayout::C(CAbiScalarKind::Long),
        "Ffi.C.UnsignedLong" => ScalarLayout::C(CAbiScalarKind::UnsignedLong),
        "Ffi.C.LongLong" => ScalarLayout::C(CAbiScalarKind::LongLong),
        "Ffi.C.UnsignedLongLong" => ScalarLayout::C(CAbiScalarKind::UnsignedLongLong),
        "Ffi.C.Size" => ScalarLayout::C(CAbiScalarKind::Size),
        "Ffi.C.PointerDifference" => ScalarLayout::C(CAbiScalarKind::PointerDifference),
        _ => return None,
    })
}

fn align_up(value: u64, alignment: u64) -> Result<u64, FfiGenerationError> {
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
        .ok_or_else(|| error(FfiGenerationErrorKind::ResourceLimit, "layout overflow"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("String write");
            output
        })
}

pub(super) fn render_descriptor(descriptor: &Descriptor) -> String {
    let mut output = String::new();
    writeln!(output, "@Ffi.Binding(").expect("String write");
    writeln!(output, "    schemaVersion = {},", descriptor.schema_version).expect("String write");
    writeln!(
        output,
        "    platformTarget = \"{}\",",
        descriptor.platform_target
    )
    .expect("String write");
    writeln!(
        output,
        "    producerName = \"{}\",",
        descriptor.producer_name
    )
    .expect("String write");
    writeln!(
        output,
        "    producerVersion = \"{}\",",
        descriptor.producer_version
    )
    .expect("String write");
    writeln!(
        output,
        "    outputNamespace = {},",
        descriptor.output_namespace
    )
    .expect("String write");
    writeln!(output, ")\nnamespace {}", descriptor.binding_namespace).expect("String write");
    render_declarations(descriptor, &mut output, true);
    output
}

pub(super) fn render_declarations(
    descriptor: &Descriptor,
    output: &mut String,
    descriptor_attributes: bool,
) {
    for record in &descriptor.records {
        output.push('\n');
        if descriptor_attributes {
            writeln!(
                output,
                "@Ffi.C.Layout(size = {}, alignment = {})",
                record.size, record.alignment
            )
            .expect("String write");
        } else {
            output.push_str("@Ffi.C.Layout\n");
        }
        writeln!(output, "internal record {}", record.name).expect("String write");
        for field in &record.fields {
            if descriptor_attributes {
                writeln!(output, "    @Ffi.C.Offset({})", field.offset).expect("String write");
            }
            write!(output, "    {}: ", field.name).expect("String write");
            field.type_name.render(output);
            output.push('\n');
        }
        output.push_str("end\n");
    }
    for function in &descriptor.functions {
        output.push('\n');
        if descriptor_attributes {
            writeln!(
                output,
                "@Ffi.Foreign(\"{}\", abi = \"{}\")",
                function.symbol,
                function.abi.source_name()
            )
            .expect("String write");
            writeln!(
                output,
                "@Ffi.Binding.CallPolicy(nonblocking = {})",
                function.nonblocking
            )
            .expect("String write");
            for pointer in &function.pointer_parameters {
                writeln!(output, "@Ffi.Binding.ParameterPointer(parameter = {pointer}, retention = Ffi.Binding.Retention.Call)")
                    .expect("String write");
            }
            if let Some(ownership) = function.result_ownership {
                let ownership = match ownership {
                    PointerOwnership::Borrowed => "Borrowed",
                    PointerOwnership::Owned => "Owned",
                };
                writeln!(
                    output,
                    "@Ffi.Binding.ResultPointer(ownership = Ffi.Binding.Ownership.{ownership})"
                )
                .expect("String write");
            }
            for pair in &function.callback_pairs {
                output.push_str("@Ffi.Binding.CallbackPair(\n");
                writeln!(
                    output,
                    "    callbackParameterIndex = {},",
                    pair.callback_parameter_index
                )
                .expect("String write");
                writeln!(
                    output,
                    "    contextParameterIndex = {},",
                    pair.context_parameter_index
                )
                .expect("String write");
                writeln!(
                    output,
                    "    lifetime = Ffi.Binding.CallbackLifetime.{},",
                    pair.lifetime.source_name()
                )
                .expect("String write");
                writeln!(
                    output,
                    "    callbackAbi = Ffi.Binding.CallbackAbi.{},",
                    pair.abi.source_name()
                )
                .expect("String write");
                writeln!(
                    output,
                    "    signatureFingerprint = \"{}\",",
                    pair.signature_fingerprint
                )
                .expect("String write");
                writeln!(
                    output,
                    "    thread = Ffi.Binding.CallbackThread.{},",
                    pair.thread.source_name()
                )
                .expect("String write");
                output.push_str("    concurrency = Ffi.Binding.CallbackConcurrency.Serialized,\n");
                output.push_str("    reentrancy = Ffi.Binding.CallbackReentrancy.Forbidden,\n");
                output.push_str("    panicPolicy = Ffi.Binding.CallbackPanic.AbortProcess,\n");
                output.push_str(")\n");
            }
            writeln!(output, "internal function {}(", function.name).expect("String write");
            for parameter in &function.parameters {
                write!(output, "    {}: ", parameter.name).expect("String write");
                parameter.type_name.render(output);
                output.push_str(",\n");
            }
            output.push(')');
        } else {
            if function.abi == ForeignAbi::C {
                writeln!(output, "@Ffi.Foreign(\"{}\")", function.symbol).expect("String write");
            } else {
                writeln!(
                    output,
                    "@Ffi.Foreign(\"{}\", abi = \"{}\")",
                    function.symbol,
                    function.abi.source_name()
                )
                .expect("String write");
            }
            if function.nonblocking {
                output.push_str("@Ffi.Nonblocking\n");
            }
            write!(output, "internal function {}(", function.name).expect("String write");
            for (index, parameter) in function.parameters.iter().enumerate() {
                if index != 0 {
                    output.push_str(", ");
                }
                write!(output, "{}: ", parameter.name).expect("String write");
                parameter.type_name.render(output);
            }
            output.push(')');
        }
        if let Some(result) = &function.result {
            output.push_str(": ");
            result.render(output);
        }
        output.push_str("\nend\n");
    }
}

fn valid_pascal(value: &str) -> bool {
    value.len() <= MAX_IDENTIFIER_BYTES
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_uppercase())
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && !reserved_word(value)
}

fn valid_camel(value: &str) -> bool {
    value.len() <= MAX_IDENTIFIER_BYTES
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && !reserved_word(value)
}

fn reserved_word(value: &str) -> bool {
    matches!(
        value,
        "and"
            | "as"
            | "async"
            | "await"
            | "break"
            | "class"
            | "const"
            | "continue"
            | "defer"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "error"
            | "false"
            | "for"
            | "function"
            | "if"
            | "in"
            | "interface"
            | "internal"
            | "local"
            | "match"
            | "namespace"
            | "nil"
            | "not"
            | "or"
            | "private"
            | "public"
            | "record"
            | "repeat"
            | "return"
            | "then"
            | "true"
            | "type"
            | "union"
            | "until"
            | "using"
            | "when"
            | "while"
    )
}

fn valid_qualified_pascal(value: &str) -> bool {
    value.split('.').all(valid_pascal)
}

fn valid_producer(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn valid_symbol(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT_BYTES
        && !value.starts_with(['-', '@'])
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'$' | b'?' | b'@')
        })
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_relative_path(value: &str, suffix: &str) -> bool {
    !value.is_empty()
        && value.ends_with(suffix)
        && !value.starts_with(['/', '@', '-'])
        && !value.contains('\\')
        && value.split('/').all(|component| {
            !component.is_empty()
                && component != "."
                && component != ".."
                && component
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

fn error(kind: FfiGenerationErrorKind, reason: impl Into<String>) -> FfiGenerationError {
    FfiGenerationError::new(kind, reason)
}
