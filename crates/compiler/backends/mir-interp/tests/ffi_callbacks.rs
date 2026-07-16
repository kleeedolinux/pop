use pop_backend_mir_interp::{
    ExecutionError, MirInterpreter, MirValue, ReferenceRuntimeEvent, TypedForeignAdapter,
};
use pop_driver::{
    FrontEndBubbleInput, FrontEndModule, FrontEndResult, VerifiedFfiGeneratedBindings,
    analyze_bubble, generate_ffi_bindings, verify_ffi_generated_bindings,
};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId, ResultCaseId, SymbolId};
use pop_mir::{MirBubble, lower_hir_bubble};
use pop_projects::{parse_package_manifest, sha256_hex};
use pop_source::SourceFile;
use pop_types::{IntegerKind, IntegerValue};
use std::path::PathBuf;

const CALLBACK_SOURCE: &str = "namespace CallbackDemo\n\
     private type CallbackSignature = function(value: Ffi.C.Int, context: Ffi.CallbackContext): Ffi.C.Int\n\
     public function scoped(captured: Ffi.C.Int): Ffi.C.Int\n\
         return Ffi.withCallback(\n\
             function(ignoredValue: Ffi.C.Int, ignoredContext: Ffi.CallbackContext): Ffi.C.Int\n\
                 return captured\n\
             end,\n\
             function(callbackFunction: Ffi.Function<CallbackSignature>, context: Ffi.CallbackContext): Ffi.C.Int\n\
                 return Native.Callbacks.Unsafe.visitScoped(callbackFunction, captured, context)\n\
             end\n\
         )\n\
     end\n\
     public function openCallback(captured: Ffi.C.Int): Result<Ffi.RegisteredCallback<CallbackSignature>, Ffi.CallbackOpenError>\n\
         return Ffi.Callback.open(\n\
             function(ignoredValue: Ffi.C.Int, ignoredContext: Ffi.CallbackContext): Ffi.C.Int\n\
                 return captured\n\
             end,\n\
             Ffi.CallbackThread.AttachedThread\n\
         )\n\
     end\n\
     public function useCallback(callback: Ffi.RegisteredCallback<CallbackSignature>, value: Ffi.C.Int): Result<Ffi.C.Int, Ffi.CallbackClosedError>\n\
         return Ffi.Callback.withPair(\n\
             callback,\n\
             function(callbackFunction: Ffi.Function<CallbackSignature>, context: Ffi.CallbackContext): Ffi.C.Int\n\
                 return Native.Callbacks.Unsafe.visitRegistered(callbackFunction, value, context)\n\
             end\n\
         )\n\
     end\n\
     public function closeCallback(callback: Ffi.RegisteredCallback<CallbackSignature>): Result<nil, Ffi.CallbackInUseError>\n\
         return Ffi.Callback.close(callback)\n\
     end\n";

fn integer(value: &str) -> MirValue {
    MirValue::Integer(IntegerValue::parse_decimal(value, IntegerKind::Int32).expect("integer"))
}

fn callback_signature_fingerprint(target: &str) -> String {
    sha256_hex(
        format!(
            "Pop.Ffi.CallbackSignature/1\n\
             platformTarget={target}\n\
             abi=C\n\
             parameterCount=2\n\
             parameter[0]=Ffi.C.Int(size=4,alignment=4)\n\
             parameter[1]=Ffi.CallbackContext(pointerWidth=64)\n\
             resultCount=1\n\
             result[0]=Ffi.C.Int(size=4,alignment=4)\n"
        )
        .as_bytes(),
    )
}

fn generated_callback_bindings() -> (PathBuf, SourceFile, Vec<VerifiedFfiGeneratedBindings>) {
    let target = "x86_64-unknown-linux-gnu";
    let fingerprint = callback_signature_fingerprint(target);
    let descriptor = format!(
        concat!(
            "@Ffi.Binding(\n",
            "    schemaVersion = 2,\n",
            "    platformTarget = \"{}\",\n",
            "    producerName = \"interpreter-fixture\",\n",
            "    producerVersion = \"1.0.0\",\n",
            "    outputNamespace = Native.Callbacks.Unsafe,\n",
            ")\n",
            "namespace Native.Callbacks.Binding\n",
            "\n",
            "@Ffi.Foreign(\"visit_registered\", abi = \"C\")\n",
            "@Ffi.Binding.CallPolicy(nonblocking = false)\n",
            "@Ffi.Binding.CallbackPair(\n",
            "    callbackParameterIndex = 0,\n",
            "    contextParameterIndex = 2,\n",
            "    lifetime = Ffi.Binding.CallbackLifetime.Registered,\n",
            "    callbackAbi = Ffi.Binding.CallbackAbi.C,\n",
            "    signatureFingerprint = \"{}\",\n",
            "    thread = Ffi.Binding.CallbackThread.AttachedThread,\n",
            "    concurrency = Ffi.Binding.CallbackConcurrency.Serialized,\n",
            "    reentrancy = Ffi.Binding.CallbackReentrancy.Forbidden,\n",
            "    panicPolicy = Ffi.Binding.CallbackPanic.AbortProcess,\n",
            ")\n",
            "internal function visitRegistered(\n",
            "    callback: Ffi.Function<function(value: Ffi.C.Int, context: Ffi.CallbackContext): Ffi.C.Int>,\n",
            "    value: Ffi.C.Int,\n",
            "    context: Ffi.CallbackContext,\n",
            "): Ffi.C.Int\n",
            "end\n",
            "\n",
            "@Ffi.Foreign(\"visit_scoped\", abi = \"C\")\n",
            "@Ffi.Binding.CallPolicy(nonblocking = false)\n",
            "@Ffi.Binding.CallbackPair(\n",
            "    callbackParameterIndex = 0,\n",
            "    contextParameterIndex = 2,\n",
            "    lifetime = Ffi.Binding.CallbackLifetime.CallScoped,\n",
            "    callbackAbi = Ffi.Binding.CallbackAbi.C,\n",
            "    signatureFingerprint = \"{}\",\n",
            "    thread = Ffi.Binding.CallbackThread.CallingThread,\n",
            "    concurrency = Ffi.Binding.CallbackConcurrency.Serialized,\n",
            "    reentrancy = Ffi.Binding.CallbackReentrancy.Forbidden,\n",
            "    panicPolicy = Ffi.Binding.CallbackPanic.AbortProcess,\n",
            ")\n",
            "internal function visitScoped(\n",
            "    callback: Ffi.Function<function(value: Ffi.C.Int, context: Ffi.CallbackContext): Ffi.C.Int>,\n",
            "    value: Ffi.C.Int,\n",
            "    context: Ffi.CallbackContext,\n",
            "): Ffi.C.Int\n",
            "end\n",
        ),
        target, fingerprint, fingerprint
    );
    let root = std::env::temp_dir().join(format!(
        "pop-mir-callbacks-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("remove prior callback fixture");
    }
    std::fs::create_dir_all(root.join("native")).expect("create callback descriptor directory");
    std::fs::write(root.join("native/callbacks.popc"), &descriptor)
        .expect("write callback descriptor");
    let manifest_text = format!(
        "[package]\nname = \"Callback.Fixture\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[platform.\"{target}\".ffiGenerators]\nCallbacks = {{ descriptor = \"native/callbacks.popc\", descriptorSha256 = \"{}\", outputDirectory = \"src/generated/callbacks\" }}\n",
        sha256_hex(descriptor.as_bytes())
    );
    let manifest_path = root.join("bubble.toml");
    std::fs::write(&manifest_path, &manifest_text).expect("write callback manifest");
    generate_ffi_bindings(&manifest_path, target, "Callbacks").expect("generate callbacks");
    let manifest = parse_package_manifest(&manifest_text).expect("parse callback manifest");
    let verified = verify_ffi_generated_bindings(&root, &manifest, target)
        .expect("verify generated callbacks");
    let source_path = "src/generated/callbacks/bindings.pop";
    let source_text =
        std::fs::read_to_string(root.join(source_path)).expect("read generated callback source");
    let source = SourceFile::new(FileId::from_raw(0), source_path, source_text)
        .expect("generated callback source");
    (root, source, verified)
}

fn analyzed_callback_program() -> (PathBuf, FrontEndResult) {
    let ffi = BubbleId::from_raw(20);
    let (fixture_root, generated, verified) = generated_callback_bindings();
    let source =
        SourceFile::new(FileId::from_raw(1), "src/callbacks.pop", CALLBACK_SOURCE).expect("source");
    let front_end = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![
                FrontEndModule::new(ModuleId::from_raw(0), generated),
                FrontEndModule::new(ModuleId::from_raw(1), source),
            ],
        )
        .with_ffi_dependency(ffi)
        .with_verified_ffi_generated_bindings(verified),
    );
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    (fixture_root, front_end)
}

fn callback_adapter(mir: &MirBubble, invoke: SymbolId) -> TypedForeignAdapter {
    let declaration = mir
        .foreign_functions()
        .iter()
        .find(|declaration| declaration.symbol() == invoke)
        .expect("foreign callback test declaration");
    TypedForeignAdapter::new_with_callbacks(
        invoke,
        declaration.parameters().to_vec(),
        declaration.results().to_vec(),
        |arguments, callbacks| {
            let [callback_function, value, context] = arguments else {
                return Err(ExecutionError::WrongArity);
            };
            callbacks.invoke(
                callback_function,
                context,
                &[value.clone(), context.clone()],
            )
        },
    )
}

fn assert_callback_event_balance(interpreter: &MirInterpreter<'_>) {
    let events = interpreter.runtime().events().to_vec();
    let counts = [
        events
            .iter()
            .filter(|event| matches!(event, ReferenceRuntimeEvent::EnterCallback(_)))
            .count(),
        events
            .iter()
            .filter(|event| matches!(event, ReferenceRuntimeEvent::LeaveCallback(_)))
            .count(),
        events
            .iter()
            .filter(|event| matches!(event, ReferenceRuntimeEvent::CloseCallback(_)))
            .count(),
    ];
    assert_eq!(counts, [2, 2, 2], "callback transition counts");
}

#[test]
fn executes_scoped_and_shared_callback_lifecycles_through_the_typed_adapter() {
    let (fixture_root, front_end) = analyzed_callback_program();
    let hir = front_end.hir().expect("callback HIR");
    let symbol = |name: &str| -> SymbolId {
        hir.functions()
            .iter()
            .find(|function| function.name() == name)
            .expect("function")
            .symbol()
    };
    let scoped = symbol("scoped");
    let open = symbol("openCallback");
    let use_callback = symbol("useCallback");
    let close = symbol("closeCallback");
    let foreign_symbol = |name: &str| -> SymbolId {
        hir.foreign_functions()
            .iter()
            .find(|function| function.name() == name)
            .expect("foreign function")
            .symbol()
    };
    let invoke_scoped = foreign_symbol("visitScoped");
    let invoke_registered = foreign_symbol("visitRegistered");
    let mir = lower_hir_bubble(hir, front_end.types()).expect("callback MIR");
    let unavailable = MirInterpreter::new(&mir, front_end.types()).expect("interpreter");
    assert!(matches!(
        unavailable.call(scoped, &[integer("23")]),
        Err(ExecutionError::UnsupportedForeignFunction(symbol)) if symbol == invoke_scoped
    ));
    let interpreter = MirInterpreter::new(&mir, front_end.types())
        .expect("interpreter")
        .with_foreign_adapter(callback_adapter(&mir, invoke_scoped))
        .expect("typed scoped callback adapter")
        .with_foreign_adapter(callback_adapter(&mir, invoke_registered))
        .expect("typed registered callback adapter");

    assert_eq!(
        interpreter.call(scoped, &[integer("23")]).expect("scoped"),
        vec![integer("23")]
    );
    let opened = interpreter.call(open, &[integer("29")]).expect("open");
    let [
        MirValue::Result {
            case, arguments, ..
        },
    ] = opened.as_slice()
    else {
        panic!("callback open must return Result");
    };
    assert_eq!(*case, ResultCaseId::from_raw(0));
    let callback = arguments.first().cloned().expect("registered callback");

    assert!(matches!(
        interpreter.call(use_callback, &[callback.clone(), integer("29")]).as_deref(),
        Ok([MirValue::Result { case, arguments, .. }])
            if *case == ResultCaseId::from_raw(0) && arguments == &[integer("29")]
    ));
    for _ in 0..2 {
        assert!(matches!(
            interpreter.call(close, std::slice::from_ref(&callback)).as_deref(),
            Ok([MirValue::Result { case, arguments, .. }])
                if *case == ResultCaseId::from_raw(0) && arguments == &[MirValue::Nil]
        ));
    }
    assert!(matches!(
        interpreter.call(use_callback, &[callback.clone(), integer("29")]).as_deref(),
        Ok([MirValue::Result { case, arguments, .. }])
            if *case == ResultCaseId::from_raw(1)
                && arguments == &[MirValue::FfiCallbackClosedError]
    ));

    assert_callback_event_balance(&interpreter);
    std::fs::remove_dir_all(fixture_root).expect("remove callback fixture");
}
