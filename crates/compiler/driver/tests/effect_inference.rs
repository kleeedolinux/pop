use std::collections::BTreeMap;

use pop_driver::{
    FrontEndBubbleInput, FrontEndModule, FrontEndResult, ReferenceMetadata, analyze_bubble,
    decode_reference_metadata, encode_reference_metadata,
};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_hir::{HirCallDispatch, HirExpressionKind, HirStatementKind};
use pop_mir::{
    MirEffect, MirInstructionKind, MirVerificationError, lower_hir_bubble, parse_mir_dump,
    verify_mir_bubble,
};
use pop_source::SourceFile;
use pop_types::{Effect, EffectSummary, SemanticType};

const PRODUCER_SOURCE: &str = "@Ffi.Link(\"SystemC\")\n\
     namespace Effects\n\
     @Ffi.Foreign(\"effect_unwind\", abi = \"CUnwind\")\n\
     @Ffi.Nonblocking\n\
     private function nativeUnwind(value: Ffi.C.Int): Ffi.C.Int\n\
     end\n\
     public function allocate(value: Int): {Int}\n\
         return { value }\n\
     end\n\
     public function trap(value: Int): Int\n\
         return value + 1\n\
     end\n\
     public function unwind(value: Ffi.C.Int): Ffi.C.Int\n\
         return nativeUnwind(value)\n\
     end\n\
     private async function ready(value: Int): Int\n\
         return value\n\
     end\n\
     public async function wait(value: Int): Int\n\
         return await ready(value)\n\
     end\n\
     public function left(value: Boolean): Boolean\n\
         if value then\n\
             return right(false)\n\
         end\n\
         return value\n\
     end\n\
     public function right(value: Boolean): Boolean\n\
         if value then\n\
             local values: {Boolean} = { value }\n\
             return values[1] ?? false\n\
         end\n\
         return left(false)\n\
     end\n";

const CONSUMER_SOURCE: &str = "namespace Consumer\n\
     public function allocate(value: Int): {Int}\n\
         return Effects.allocate(value)\n\
     end\n\
     public function trap(value: Int): Int\n\
         return Effects.trap(value)\n\
     end\n\
     public function unwind(value: Ffi.C.Int): Ffi.C.Int\n\
         return Effects.unwind(value)\n\
     end\n\
     public async function wait(value: Int): Int\n\
         return await Effects.wait(value)\n\
     end\n\
     public function recurse(value: Boolean): Boolean\n\
         return Effects.left(value)\n\
     end\n";

fn module(raw: u32, path: &str, text: &str) -> FrontEndModule {
    FrontEndModule::new(
        ModuleId::from_raw(raw),
        SourceFile::new(FileId::from_raw(raw), path, text).expect("test source"),
    )
}

fn summary(effects: impl IntoIterator<Item = Effect>) -> EffectSummary {
    effects
        .into_iter()
        .fold(EffectSummary::empty(), EffectSummary::with)
}

fn analyze_effect_producer(ffi_bubble: BubbleId, producer_bubble: BubbleId) -> FrontEndResult {
    analyze_bubble(
        FrontEndBubbleInput::new(
            producer_bubble,
            NamespaceId::from_raw(41),
            vec![ffi_bubble],
            vec![module(0, "src/effects.pop", PRODUCER_SOURCE)],
        )
        .with_ffi_dependency(ffi_bubble),
    )
}

fn analyze_effect_consumer(
    ffi_bubble: BubbleId,
    producer_bubble: BubbleId,
    metadata: ReferenceMetadata,
) -> FrontEndResult {
    analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(42),
            NamespaceId::from_raw(42),
            vec![ffi_bubble, producer_bubble],
            vec![module(0, "src/consumer.pop", CONSUMER_SOURCE)],
        )
        .with_ffi_dependency(ffi_bubble)
        .with_reference_metadata(vec![metadata]),
    )
}

#[test]
fn ordinary_effects_reach_consumers_and_mutual_recursion_converges() {
    let ffi_bubble = BubbleId::from_raw(40);
    let producer_bubble = BubbleId::from_raw(41);
    let producer = analyze_effect_producer(ffi_bubble, producer_bubble);
    assert!(
        producer.diagnostics().is_empty(),
        "{}",
        producer.diagnostic_snapshot()
    );

    let hir = producer.hir().expect("producer HIR");
    let hir_effects = hir
        .functions()
        .iter()
        .map(|function| (function.name(), function.effects()))
        .collect::<BTreeMap<_, _>>();
    let allocation = summary([Effect::Allocates, Effect::MayUnwind, Effect::GcSafePoint]);
    assert_eq!(hir_effects["allocate"], allocation);
    assert_eq!(hir_effects["trap"], summary([Effect::MayTrap]));
    assert_eq!(hir_effects["left"], allocation);
    assert_eq!(
        hir_effects["right"], allocation,
        "the recursive component must converge to the same least fixed point"
    );
    assert!(hir_effects["unwind"].contains(Effect::MayUnwind));
    assert!(hir_effects["wait"].contains(Effect::Suspends));
    for name in ["allocate", "trap", "left", "right", "wait"] {
        assert!(
            !hir_effects[name].contains(Effect::ForeignFunction),
            "{name}"
        );
        assert!(!hir_effects[name].contains(Effect::UnsafeMemory), "{name}");
        assert!(!hir_effects[name].contains(Effect::AmbientIo), "{name}");
    }

    let metadata = producer
        .reference_metadata()
        .expect("ordinary effect metadata");
    for function in metadata.functions() {
        assert_eq!(
            function.effects(),
            hir_effects[function.name()],
            "producer HIR and reference metadata disagree for {}",
            function.name()
        );
    }
    let encoded = encode_reference_metadata(metadata).expect("encode effect metadata");
    let decoded = decode_reference_metadata(&encoded).expect("decode effect metadata");
    let encoded_text = String::from_utf8(encoded).expect("UTF-8 effect metadata");
    let allocation_marker = format!("\"effects\":{}", allocation.bits());
    let corrupt_effects = encoded_text.replacen(&allocation_marker, "\"effects\":32768", 1);
    assert_ne!(
        corrupt_effects, encoded_text,
        "effect field must be present"
    );
    assert!(
        decode_reference_metadata(corrupt_effects.as_bytes()).is_err(),
        "unknown effect bits must fail closed"
    );

    let consumer = analyze_effect_consumer(ffi_bubble, producer_bubble, decoded);
    assert!(
        consumer.diagnostics().is_empty(),
        "{}",
        consumer.diagnostic_snapshot()
    );
    let consumer_hir = consumer.hir().expect("consumer HIR");
    let reference_effects = consumer_hir
        .function_references()
        .iter()
        .map(|function| (function.identity(), function.effects()))
        .collect::<BTreeMap<_, _>>();
    assert!(
        reference_effects
            .values()
            .any(|effects| *effects == allocation)
    );
    assert!(
        reference_effects
            .values()
            .any(|effects| *effects == summary([Effect::MayTrap]))
    );
    assert!(
        reference_effects
            .values()
            .any(|effects| effects.contains(Effect::MayUnwind))
    );
    assert!(
        reference_effects
            .values()
            .any(|effects| effects.contains(Effect::Suspends))
    );
    let consumer_effects = consumer_hir
        .functions()
        .iter()
        .map(|function| (function.name(), function.effects()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(consumer_effects["allocate"], allocation);
    assert_eq!(consumer_effects["trap"], summary([Effect::MayTrap]));
    assert_eq!(consumer_effects["recurse"], allocation);
    assert!(consumer_effects["unwind"].contains(Effect::MayUnwind));
    assert!(consumer_effects["wait"].contains(Effect::Suspends));
}

#[test]
fn exact_function_value_and_closure_effects_drive_indirect_calls() {
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(50),
        NamespaceId::from_raw(50),
        Vec::new(),
        vec![module(
            0,
            "src/indirect.pop",
            "namespace Indirect\n\
             private function increment(value: Int): Int\n\
                 return value + 1\n\
             end\n\
             public function callFunction(value: Int): Int\n\
                 local operation = increment\n\
                 return operation(value)\n\
             end\n\
             public function callClosure(value: Int): Int\n\
                 local operation = function(input: Int): Int\n\
                     return input + 1\n\
                 end\n\
                 return operation(value)\n\
             end\n",
        )],
    ));
    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let hir = result.hir().expect("indirect HIR");
    let exact = summary([Effect::MayTrap]);
    for function_name in ["callFunction", "callClosure"] {
        let function = hir
            .functions()
            .iter()
            .find(|function| function.name() == function_name)
            .unwrap_or_else(|| panic!("missing {function_name}"));
        let HirStatementKind::Local { initializer, .. } = function.body()[0].kind() else {
            panic!("{function_name} function-value local");
        };
        let Some(SemanticType::Function { effects, .. }) =
            result.types().get(initializer.type_id())
        else {
            panic!("{function_name} initializer must have function type");
        };
        assert_eq!(*effects, exact, "{function_name} function type");
        if let HirExpressionKind::Closure(closure) = initializer.kind() {
            assert_eq!(closure.effects(), exact, "closure body summary");
        }
        let HirStatementKind::Return { values } = function.body()[1].kind() else {
            panic!("{function_name} indirect return");
        };
        assert!(matches!(
            values[0].kind(),
            HirExpressionKind::Call {
                dispatch: HirCallDispatch::Indirect { .. },
                ..
            }
        ));
    }

    let mir = lower_hir_bubble(hir, result.types()).expect("indirect MIR");
    let mut indirect_effects = Vec::new();
    for function in mir.functions() {
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let MirInstructionKind::CallIndirect {
                    declared_effects, ..
                } = instruction.kind()
                {
                    indirect_effects.push(*declared_effects);
                }
            }
        }
    }
    assert_eq!(indirect_effects.len(), 2);
    for effects in indirect_effects {
        assert!(effects.contains(MirEffect::MayTrap));
        assert!(!effects.contains(MirEffect::ForeignFunction));
        assert!(!effects.contains(MirEffect::AmbientIo));
        assert!(!effects.contains(MirEffect::Suspends));
        assert!(!effects.contains(MirEffect::Blocks));
    }
}

#[test]
fn interface_calls_use_the_exact_declared_member_summary() {
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(51),
        NamespaceId::from_raw(51),
        Vec::new(),
        vec![module(
            0,
            "src/interfaceEffects.pop",
            "namespace InterfaceEffects\n\
             private interface Reader\n\
                 function read(count: Int): Int\n\
             end\n\
             private class FileReader implements Reader\n\
                 public function FileReader:read(count: Int): Int\n\
                     return count\n\
                 end\n\
             end\n\
             public function read(reader: FileReader, count: Int): Int\n\
                 local contract: Reader = reader\n\
                 return contract:read(count)\n\
             end\n",
        )],
    ));
    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let hir = result.hir().expect("interface-effect HIR");
    let function = hir
        .functions()
        .iter()
        .find(|function| function.name() == "read")
        .expect("reader function");
    assert_eq!(function.effects(), EffectSummary::empty());

    let mir = lower_hir_bubble(hir, result.types()).expect("interface-effect MIR");
    let declared = mir
        .functions()
        .iter()
        .flat_map(pop_mir::MirFunction::blocks)
        .flat_map(pop_mir::MirBlock::instructions)
        .find_map(|instruction| match instruction.kind() {
            MirInstructionKind::CallInterface {
                declared_effects, ..
            } => Some(*declared_effects),
            _ => None,
        })
        .expect("interface call");
    assert!(!declared.contains(MirEffect::MayTrap));
    assert!(!declared.contains(MirEffect::ForeignFunction));
    assert!(!declared.contains(MirEffect::AmbientIo));
    assert!(!declared.contains(MirEffect::Suspends));
    assert!(!declared.contains(MirEffect::Blocks));

    let dump = mir.dump();
    let interface_call = dump
        .lines()
        .find(|line| line.contains("call.interface"))
        .expect("interface call text");
    let forged_call = interface_call.replacen("effects[]", "effects[MayTrap]", 1);
    let forged = parse_mir_dump(&dump.replacen(interface_call, &forged_call, 1))
        .expect("structurally valid forged interface effects");
    let errors = verify_mir_bubble(&forged, result.types())
        .expect_err("forged interface effects must fail closed");
    assert!(errors.iter().any(|error| matches!(
        error,
        MirVerificationError::InstructionEffectMismatch { .. }
    )));
}

#[test]
fn interface_implementation_cannot_widen_the_declared_member_summary() {
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(52),
        NamespaceId::from_raw(52),
        Vec::new(),
        vec![module(
            0,
            "src/interfaceEffectWidening.pop",
            "namespace InterfaceEffectWidening\n\
             private interface Reader\n\
                 function read(count: Int): Int\n\
             end\n\
             private class FileReader implements Reader\n\
                 public function FileReader:read(count: Int): Int\n\
                     return count + 1\n\
                 end\n\
             end\n",
        )],
    ));

    assert_eq!(
        result
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect::<Vec<_>>(),
        ["POP2019"],
        "{}",
        result.diagnostic_snapshot()
    );
    assert!(result.hir().is_none());
}

#[test]
fn effectful_function_value_cannot_widen_a_closed_parameter_contract() {
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(53),
        NamespaceId::from_raw(53),
        Vec::new(),
        vec![module(
            0,
            "src/functionEffectWidening.pop",
            "namespace FunctionEffectWidening\n\
             private function increment(value: Int): Int\n\
                 return value + 1\n\
             end\n\
             private function invoke(operation: function(value: Int): Int, value: Int): Int\n\
                 return operation(value)\n\
             end\n\
             public function run(value: Int): Int\n\
                 return invoke(increment, value)\n\
             end\n",
        )],
    ));

    assert_eq!(
        result
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect::<Vec<_>>(),
        ["POP2003"],
        "{}",
        result.diagnostic_snapshot()
    );
    assert!(result.hir().is_none());
}

#[test]
fn reserved_iterator_calls_use_the_fixed_exact_upper_summary() {
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(54),
        NamespaceId::from_raw(54),
        Vec::new(),
        vec![module(
            0,
            "src/iteratorEffects.pop",
            "namespace IteratorEffects\n\
             public function exhaust(source: Iterator<Int>)\n\
                 for value in source do\n\
                     local observed = value\n\
                 end\n\
             end\n",
        )],
    ));
    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let expected = summary([
        Effect::WritesManagedReference,
        Effect::MayTrap,
        Effect::GcSafePoint,
    ]);
    let hir = result.hir().expect("reserved iterator HIR");
    let exhaust = hir
        .functions()
        .iter()
        .find(|function| function.name() == "exhaust")
        .expect("generic exhaust function");
    assert_eq!(exhaust.effects(), expected);

    let mir = lower_hir_bubble(hir, result.types()).expect("reserved iterator MIR");
    let protocol = pop_types::embedded_bootstrap_schema()
        .expect("bootstrap schema")
        .iteration_protocol()
        .expect("iteration protocol");
    let calls = mir
        .functions()
        .iter()
        .flat_map(pop_mir::MirFunction::blocks)
        .flat_map(pop_mir::MirBlock::instructions)
        .filter_map(|instruction| match instruction.kind() {
            MirInstructionKind::CallBuiltinInterface {
                interface,
                method,
                declared_effects,
                ..
            } => Some((*interface, *method, *declared_effects)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(calls.iter().any(|(interface, method, effects)| {
        *interface == protocol.iterator()
            && *method == protocol.iterator_method()
            && *effects == pop_mir::MirEffectSummary::empty()
    }));
    let next = pop_mir::MirEffectSummary::from_effects([
        MirEffect::WritesManagedReference,
        MirEffect::MayTrap,
        MirEffect::GcSafePoint,
    ]);
    assert!(calls.iter().any(|(interface, method, effects)| {
        *interface == protocol.iterator() && *method == protocol.next_method() && *effects == next
    }));

    let dump = mir.dump();
    let next_call = dump
        .lines()
        .find(|line| {
            line.contains("call.builtinInterface")
                && line.contains(&format!("method#{}", protocol.next_method().raw()))
        })
        .expect("reserved next call text");
    let effects_start = next_call.find("effects[").expect("declared next effects");
    let effects_end = next_call[effects_start..]
        .find(']')
        .map(|offset| effects_start + offset + 1)
        .expect("closed next effects");
    let forged_call = format!(
        "{}effects[]{}",
        &next_call[..effects_start],
        &next_call[effects_end..]
    );
    let forged = parse_mir_dump(&dump.replacen(next_call, &forged_call, 1))
        .expect("structurally valid forged reserved effects");
    let errors = verify_mir_bubble(&forged, result.types())
        .expect_err("reserved next call cannot narrow its exact summary");
    assert!(errors.iter().any(|error| matches!(
        error,
        MirVerificationError::InstructionEffectMismatch { .. }
    )));
}

#[test]
fn reserved_iterator_implementation_cannot_widen_next_upper_summary() {
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(55),
        NamespaceId::from_raw(55),
        Vec::new(),
        vec![module(
            0,
            "src/iteratorEffectWidening.pop",
            "namespace IteratorEffectWidening\n\
             private class AllocatingIterator implements Iterator<Int>\n\
                 public function AllocatingIterator:iterator(): Iterator<Int>\n\
                     return self\n\
                 end\n\
                 public function AllocatingIterator:next(): Iteration<Int>\n\
                     local allocation: {Int} = { 1 }\n\
                     return Iteration.End\n\
                 end\n\
             end\n",
        )],
    ));
    assert_eq!(
        result
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect::<Vec<_>>(),
        ["POP2019"],
        "{}",
        result.diagnostic_snapshot()
    );
    assert!(result.hir().is_none());
}
