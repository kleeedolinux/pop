use std::fmt::Write;

use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::{
    MirEffect, MirInstructionKind, MirTerminator, MirVerificationError, lower_hir_bubble,
    parse_mir_dump, verify_mir_bubble,
};
use pop_runtime_interface::{ArrayElementMap, ObjectSlot, TrapKind};
use pop_source::SourceFile;

fn lower(text: &str) -> (pop_mir::MirBubble, pop_types::TypeArena) {
    let source = SourceFile::new(FileId::from_raw(0), "src/runtime.pop", text).expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let mir = lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    (mir, front_end.types().clone())
}

#[test]
fn allocating_functions_declare_effects_and_emit_precise_safe_points_and_array_maps() {
    let (mir, types) = lower(
        "namespace Main\n\
         public function keepFirst(): {Int}\n\
             local first: {Int} = { 1 }\n\
             local second: {Int} = { 2 }\n\
             return first\n\
         end\n",
    );
    let function = &mir.functions()[0];
    assert!(function.effects().contains(MirEffect::Allocates));
    assert!(function.effects().contains(MirEffect::GcSafePoint));
    let allocations: Vec<_> = function
        .blocks()
        .iter()
        .flat_map(pop_mir::MirBlock::instructions)
        .filter_map(|instruction| match instruction.kind() {
            MirInstructionKind::ArrayMake {
                elements,
                element_map,
            } => Some((instruction.result(), elements, element_map)),
            _ => None,
        })
        .collect();
    assert_eq!(allocations.len(), 2);
    assert!(
        allocations
            .iter()
            .all(|(_, _, map)| **map == ArrayElementMap::Scalar)
    );
    let safe_points: Vec<_> = function
        .blocks()
        .iter()
        .flat_map(pop_mir::MirBlock::instructions)
        .filter_map(|instruction| match instruction.kind() {
            MirInstructionKind::GcSafePoint {
                roots, stack_map, ..
            } => Some((roots, stack_map)),
            _ => None,
        })
        .collect();
    assert_eq!(safe_points.len(), 2);
    assert!(safe_points[0].0.is_empty());
    assert_eq!(safe_points[1].0, &[allocations[0].0]);
    assert_eq!(safe_points[1].1.root_slots().len(), safe_points[1].0.len());

    let dump = mir.dump();
    assert!(dump.contains("effects[Allocates,MayUnwind,GcSafePoint]"));
    assert!(dump.contains("arrayMake scalar"));
    assert!(dump.contains("gcSafePoint sp"));
    let reparsed = parse_mir_dump(&dump).expect("runtime MIR round trip");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, &types).is_ok());
}

#[test]
fn checked_operations_name_every_portable_trap_kind() {
    let (mir, _) = lower(
        "namespace Main\n\
         public function divide(left: Int, right: Int): Int\n\
             return left / right\n\
         end\n",
    );
    let divide = mir.functions()[0]
        .blocks()
        .iter()
        .flat_map(pop_mir::MirBlock::instructions)
        .find(|instruction| {
            matches!(
                instruction.kind(),
                MirInstructionKind::CheckedIntegerDivide { .. }
            )
        })
        .expect("checked division");
    assert_eq!(
        divide.kind().possible_traps(),
        vec![TrapKind::IntegerOverflow, TrapKind::DivisionByZero]
    );
    assert!(divide.effects().contains(MirEffect::MayTrap));
}

#[test]
fn effect_summary_keeps_blocking_distinct_from_suspension_and_io() {
    let effects = pop_mir::MirEffectSummary::from_effects([
        MirEffect::Blocks,
        MirEffect::Suspends,
        MirEffect::AmbientIo,
    ]);
    assert!(effects.contains(MirEffect::Blocks));
    assert!(effects.contains(MirEffect::Suspends));
    assert!(effects.contains(MirEffect::AmbientIo));
    assert_eq!(
        effects.iter().collect::<Vec<_>>(),
        vec![MirEffect::Suspends, MirEffect::Blocks, MirEffect::AmbientIo]
    );
}

#[test]
fn standard_string_output_identity_requires_a_string_argument() {
    let (mir, types) = lower(
        "namespace Main\n\
         public function write()\n\
             print(\"teste\")\n\
         end\n",
    );
    let invalid_dump = mir.dump().replace("callStandard sf1", "callStandard sf0");
    let invalid = parse_mir_dump(&invalid_dump).expect("structurally valid MIR");
    let errors = verify_mir_bubble(&invalid, &types).expect_err("identity mismatch");

    assert!(
        errors
            .iter()
            .any(|error| matches!(error, MirVerificationError::WrongOperandType { .. }))
    );
}

#[test]
fn table_allocations_carry_homogeneous_key_and_value_maps() {
    let (mir, _) = lower(
        "namespace Main\n\
         public function scores(): {[String]: Int}\n\
             return { first = 1, second = 2 }\n\
         end\n",
    );
    let maps = mir.functions()[0]
        .blocks()
        .iter()
        .flat_map(pop_mir::MirBlock::instructions)
        .find_map(|instruction| match instruction.kind() {
            MirInstructionKind::TableMake {
                key_map, value_map, ..
            } => Some((*key_map, *value_map)),
            _ => None,
        })
        .expect("table element maps");
    assert_eq!(
        maps,
        (ArrayElementMap::ManagedReference, ArrayElementMap::Scalar,)
    );
}

#[test]
fn long_straight_line_work_receives_a_deterministic_bounded_poll() {
    let mut source = String::from("namespace Main\npublic function work(): Int\n");
    for value in 0..257 {
        let _ = writeln!(source, "    {value}");
    }
    source.push_str("    return 257\nend\n");
    let (mir, types) = lower(&source);
    let function = &mir.functions()[0];
    let safe_points: Vec<_> = function
        .blocks()
        .iter()
        .flat_map(pop_mir::MirBlock::instructions)
        .filter(|instruction| matches!(instruction.kind(), MirInstructionKind::GcSafePoint { .. }))
        .collect();
    assert_eq!(safe_points.len(), 1);
    assert!(function.effects().contains(MirEffect::GcSafePoint));

    let dump = mir.dump();
    let safe_point_line = dump
        .lines()
        .find(|line| line.contains("gcSafePoint"))
        .expect("bounded poll");
    let without_poll = parse_mir_dump(
        &dump
            .lines()
            .filter(|line| *line != safe_point_line)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("MIR without bounded poll");
    assert!(matches!(
        verify_mir_bubble(&without_poll, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::MissingGcSafePoint { .. }
        ))
    ));
}

#[test]
fn verifier_rejects_underdeclared_effects_and_incomplete_stack_maps() {
    let (mir, types) = lower(
        "namespace Main\n\
         public function keepFirst(): {Int}\n\
             local first: {Int} = { 1 }\n\
             local second: {Int} = { 2 }\n\
             return first\n\
         end\n",
    );
    let dump = mir.dump();
    let effects =
        parse_mir_dump(&dump.replacen("effects[Allocates,MayUnwind,GcSafePoint]", "effects[]", 1))
            .expect("structurally valid underdeclared effects");
    assert!(matches!(
        verify_mir_bubble(&effects, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::FunctionEffectMismatch { .. }
        ))
    ));

    let root_line = dump
        .lines()
        .find(|line| line.contains("gcSafePoint") && !line.ends_with("roots ()"))
        .expect("safe point with a live root");
    let malformed_line = root_line
        .split_once(" roots ")
        .map(|(prefix, _)| format!("{prefix} roots ()"))
        .expect("root clause");
    let incomplete = parse_mir_dump(&dump.replacen(root_line, &malformed_line, 1))
        .expect("structurally valid incomplete map");
    assert!(matches!(
        verify_mir_bubble(&incomplete, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::IncompleteStackMap { .. }
        ))
    ));
}

#[test]
fn precise_liveness_translates_managed_block_arguments_across_cfg_edges() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let valid = parse_mir_dump(&format!(
        concat!(
            "mir bubble b0 namespace n0\n",
            "dependencies\n",
            "function s0 f0(t{array}) -> (t{array}) effects[GcSafePoint]\n",
            "  b0(v0:t{array}):\n",
            "    do v2 gcSafePoint sp0 roots (v0)\n",
            "    branch b1 (v0)\n",
            "  b1(v1:t{array}):\n",
            "    do v3 gcSafePoint sp1 roots (v1)\n",
            "    return (v1)\n",
        ),
        array = array.raw(),
    ))
    .expect("block argument roots");
    assert!(verify_mir_bubble(&valid, &types).is_ok());

    let missing_incoming = parse_mir_dump(&valid.dump().replacen(
        "gcSafePoint sp0 roots (v0)",
        "gcSafePoint sp0 roots ()",
        1,
    ))
    .expect("missing incoming root");
    assert!(matches!(
        verify_mir_bubble(&missing_incoming, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::IncompleteStackMap { .. }
                | MirVerificationError::InvalidStackMapRoot { .. }
        ))
    ));
}

#[test]
fn pin_handles_are_private_balanced_gc_transitions() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let mir = parse_mir_dump(&format!(
        concat!(
            "mir bubble b0 namespace n0\n",
            "dependencies\n",
            "function s0 f0() -> (t{integer}) effects[Allocates,MayUnwind,GcSafePoint,Roots]\n",
            "  b0():\n",
            "    do v0 gcSafePoint sp0 roots ()\n",
            "    v1:t{array} = arrayMake scalar ()\n",
            "    do v2 pin v1\n",
            "    do v3 unpin v2\n",
            "    v4:t{integer} = const.integer Int64 0\n",
            "    return (v4)\n",
        ),
        integer = integer.raw(),
        array = array.raw(),
    ))
    .expect("pin MIR");

    assert!(mir.dump().contains("pin v1"));
    assert!(mir.dump().contains("unpin v2"));
    assert!(verify_mir_bubble(&mir, &types).is_ok());

    let unbalanced = parse_mir_dump(&mir.dump().replace("    do v3 unpin v2\n", ""))
        .expect("unbalanced pin MIR");
    assert!(matches!(
        verify_mir_bubble(&unbalanced, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::UnreleasedPin { .. }
        ))
    ));
}

#[test]
fn managed_field_writes_have_an_explicit_barrier_before_the_store() {
    let (mir, types) = lower(
        "namespace Main\n\
         public class Holder\n\
             public values: {Int}\n\
             public function Holder.new(values: {Int}): Holder\n\
                 return Holder { values = values }\n\
             end\n\
             public function Holder:set(values: {Int})\n\
                 self.values = values\n\
             end\n\
         end\n",
    );
    let setter = mir
        .methods()
        .iter()
        .find(|method| method.function().results().is_empty())
        .expect("setter");
    let class_map = mir
        .methods()
        .iter()
        .flat_map(|method| method.function().blocks())
        .flat_map(pop_mir::MirBlock::instructions)
        .find_map(|instruction| match instruction.kind() {
            MirInstructionKind::ClassMake { object_map, .. } => Some(object_map),
            _ => None,
        })
        .expect("class allocation map");
    assert_eq!(class_map.slot_count(), 1);
    assert_eq!(class_map.reference_slots(), &[ObjectSlot::new(0)]);
    let instructions: Vec<_> = setter.function().blocks()[0].instructions().to_vec();
    let barrier = instructions
        .iter()
        .position(|instruction| {
            matches!(instruction.kind(), MirInstructionKind::WriteBarrier { .. })
        })
        .expect("write barrier");
    let store = instructions
        .iter()
        .position(|instruction| matches!(instruction.kind(), MirInstructionKind::FieldSet { .. }))
        .expect("field store");
    assert!(barrier < store);
    assert!(
        setter
            .function()
            .effects()
            .contains(MirEffect::WritesManagedReference)
    );
    assert!(verify_mir_bubble(&mir, &types).is_ok());
}

#[test]
fn optimizer_retains_a_verified_proof_when_eliding_an_unpublished_owner_barrier() {
    let (mir, types) = lower(
        "namespace Main\n\
         public class Holder\n\
             public values: {Int}\n\
         end\n\
         public function replace(values: {Int}, replacement: {Int}): Holder\n\
             local holder = Holder { values = values }\n\
             holder.values = replacement\n\
             return holder\n\
         end\n",
    );

    let optimized = pop_mir::optimize_mir(mir, &types).expect("optimized MIR");
    let dump = optimized.dump();
    assert!(
        dump.contains("proof UnpublishedOwner"),
        "optimized MIR must retain the barrier-elision proof: {dump}"
    );
    assert!(
        optimized.functions()[0]
            .effects()
            .contains(MirEffect::WritesManagedReference)
    );
    assert!(verify_mir_bubble(&optimized, &types).is_ok());
}

#[test]
fn verifier_rejects_a_forged_unpublished_owner_barrier_proof() {
    let (mir, types) = lower(
        "namespace Main\n\
         public class Holder\n\
             public values: {Int}\n\
             public function Holder:set(values: {Int})\n\
                 self.values = values\n\
             end\n\
         end\n",
    );
    let dump = mir.dump();
    let barrier = dump
        .lines()
        .find(|line| line.contains("writeBarrier"))
        .expect("write barrier");
    let forged =
        parse_mir_dump(&dump.replacen(barrier, &format!("{barrier} proof UnpublishedOwner"), 1))
            .expect("proof-bearing MIR");

    assert!(matches!(
        verify_mir_bubble(&forged, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidBarrierElisionProof { .. }
        ))
    ));
}

#[test]
fn textual_trap_panic_unwind_and_root_actions_are_explicit_and_verified() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let trap = parse_mir_dump(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> () effects[MayTrap]\n  b0():\n    trap IntegerOverflow\n",
    )
    .expect("trap MIR");
    assert!(matches!(
        trap.functions()[0].blocks()[0].terminator(),
        MirTerminator::Trap(trap) if trap.kind() == TrapKind::IntegerOverflow
    ));
    assert!(verify_mir_bubble(&trap, &types).is_ok());

    let panic_text = "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> () effects[MayUnwind]\n  b0():\n    panic RuntimeInvariant\n";
    let panic = parse_mir_dump(panic_text).expect("panic MIR");
    assert!(matches!(
        panic.functions()[0].blocks()[0].terminator(),
        MirTerminator::Panic(_)
    ));
    assert!(verify_mir_bubble(&panic, &types).is_ok());

    let invalid_release = format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0(t{array}) -> () effects[Roots]\n  b0(v0:t{array}):\n    do v1 releaseRoot v0\n    return ()\n",
        array = array.raw(),
    );
    let invalid_release = parse_mir_dump(&invalid_release).expect("root action MIR");
    assert!(matches!(
        verify_mir_bubble(&invalid_release, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::ReleaseWithoutRetain { .. }
        ))
    ));

    let unreleased = format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0(t{array}) -> () effects[Roots]\n  b0(v0:t{array}):\n    do v1 retainRoot v0\n    return ()\n",
        array = array.raw(),
    );
    let unreleased = parse_mir_dump(&unreleased).expect("unreleased root MIR");
    assert!(matches!(
        verify_mir_bubble(&unreleased, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::UnreleasedRoot { value, .. } if value.raw() == 1
        ))
    ));

    let duplicate_release = format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0(t{array}) -> () effects[Roots]\n  b0(v0:t{array}):\n    do v1 retainRoot v0\n    do v2 releaseRoot v1\n    do v3 releaseRoot v1\n    return ()\n",
        array = array.raw(),
    );
    let duplicate_release = parse_mir_dump(&duplicate_release).expect("duplicate root MIR");
    assert!(matches!(
        verify_mir_bubble(&duplicate_release, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::ReleaseWithoutRetain { .. }
        ))
    ));

    let separately_retained = format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0(t{array}) -> () effects[Roots]\n  b0(v0:t{array}):\n    do v1 retainRoot v0\n    do v2 retainRoot v0\n    do v3 releaseRoot v1\n    do v4 releaseRoot v2\n    return ()\n",
        array = array.raw(),
    );
    let separately_retained = parse_mir_dump(&separately_retained).expect("separate roots MIR");
    assert!(verify_mir_bubble(&separately_retained, &types).is_ok());

    let balanced_edge = format!(
        concat!(
            "mir bubble b0 namespace n0\n",
            "dependencies\n",
            "function s0 f0(t{array}) -> () effects[Roots]\n",
            "  b0(v0:t{array}):\n",
            "    do v2 retainRoot v0\n",
            "    branch b1 (v0)\n",
            "  b1(v1:t{array}):\n",
            "    do v3 releaseRoot v2\n",
            "    return ()\n",
        ),
        array = array.raw(),
    );
    let balanced_edge = parse_mir_dump(&balanced_edge).expect("balanced edge root MIR");
    assert!(verify_mir_bubble(&balanced_edge, &types).is_ok());
}

#[test]
fn typed_ffi_handle_operations_are_distinct_from_private_root_tokens() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let string = types.source_type("String").expect("String");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let handle = types
        .intern(pop_types::SemanticType::Builtin {
            definition: pop_types::FFI_HANDLE_TYPE_ID,
            arguments: vec![array],
        })
        .expect("array handle");
    let string_handle = types
        .intern(pop_types::SemanticType::Builtin {
            definition: pop_types::FFI_HANDLE_TYPE_ID,
            arguments: vec![string],
        })
        .expect("string handle");

    let valid = format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> (t{array}) effects[Allocates,MayTrap,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    v2:t{handle} = ffiHandleOpen v1\n    v3:t{array} = ffiHandleGet v2\n    do v4 ffiHandleClose v2\n    return (v3)\n",
        array = array.raw(),
        handle = handle.raw(),
    );
    let valid = parse_mir_dump(&valid).expect("typed FFI handle MIR");
    assert!(verify_mir_bubble(&valid, &types).is_ok());
    let dump = valid.dump();
    assert!(dump.contains("ffiHandleOpen v1"));
    assert!(dump.contains("ffiHandleGet v2"));
    assert!(dump.contains("ffiHandleClose v2"));
    assert_eq!(parse_mir_dump(&dump).expect("handle round trip"), valid);
    let optimized = pop_mir::optimize_mir(valid, &types).expect("optimized handles");
    let optimized_dump = optimized.dump();
    assert!(optimized_dump.contains("ffiHandleOpen v1"));
    assert!(optimized_dump.contains("ffiHandleGet v2"));
    assert!(optimized_dump.contains("ffiHandleClose v2"));

    let invalid_cases = [
        format!(
            "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> () effects[MayTrap,Roots]\n  b0():\n    v0:t{integer} = const.integer Int64 1\n    v1:t{handle} = ffiHandleOpen v0\n    return ()\n",
            integer = integer.raw(),
            handle = handle.raw(),
        ),
        format!(
            "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> () effects[Allocates,MayTrap,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    v2:t{string_handle} = ffiHandleOpen v1\n    return ()\n",
            array = array.raw(),
            string_handle = string_handle.raw(),
        ),
        format!(
            "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> () effects[Allocates,MayTrap,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    v2:t{handle} = ffiHandleOpen v1\n    v3:t{string} = ffiHandleGet v2\n    do v4 ffiHandleClose v2\n    return ()\n",
            array = array.raw(),
            handle = handle.raw(),
            string = string.raw(),
        ),
        format!(
            "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> () effects[Allocates,MayTrap,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    v2:t{array} = ffiHandleGet v1\n    return ()\n",
            array = array.raw(),
        ),
        format!(
            "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> () effects[Allocates,MayTrap,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    do v2 retainRoot v1\n    v3:t{array} = ffiHandleGet v2\n    do v4 releaseRoot v2\n    return ()\n",
            array = array.raw(),
        ),
    ];
    for text in invalid_cases {
        let invalid = parse_mir_dump(&text).expect("parse invalid typed handle MIR");
        assert!(
            verify_mir_bubble(&invalid, &types).is_err(),
            "invalid typed handle MIR was accepted:\n{text}"
        );
    }
}

#[test]
fn call_effects_are_exact_and_cleanup_edges_are_verified_cfg_edges() {
    let types = pop_types::TypeArena::new();
    let valid = parse_mir_dump(concat!(
        "mir bubble b0 namespace n0\n",
        "dependencies\n",
        "function s0 f0() -> () effects[MayUnwind]\n",
        "  b0():\n",
        "    panic RuntimeInvariant\n",
        "function s1 f1() -> () effects[MayUnwind]\n",
        "  b0():\n",
        "    do v0 callDirect s0 () effects[MayUnwind] unwind cleanup:b1\n",
        "    return ()\n",
        "  b1() cleanup scope#0 reason unwind:\n",
        "    return ()\n",
    ))
    .expect("cleanup MIR");
    assert!(verify_mir_bubble(&valid, &types).is_ok());

    let resume_without_cleanup = parse_mir_dump(concat!(
        "mir bubble b0 namespace n0\n",
        "dependencies\n",
        "function s0 f0() -> () effects[MayUnwind]\n",
        "  b0():\n",
        "    resumeCurrentUnwind\n",
    ))
    .expect("structural unwind resume MIR");
    assert!(matches!(
        verify_mir_bubble(&resume_without_cleanup, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::ResumeOutsideCleanup { .. }
        ))
    ));

    let valid_dump = valid.dump();
    let direct_call = valid_dump
        .lines()
        .find(|line| line.contains("callDirect s0"))
        .expect("direct call text");
    let underdeclared_call = direct_call.replacen("effects[MayUnwind]", "effects[]", 1);
    assert_ne!(underdeclared_call, direct_call);
    let underdeclared = parse_mir_dump(&valid_dump.replacen(direct_call, &underdeclared_call, 1))
        .expect("underdeclared call MIR");
    let underdeclared_errors =
        verify_mir_bubble(&underdeclared, &types).expect_err("underdeclared call effects");
    assert!(
        underdeclared_errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InstructionEffectMismatch { .. }
        )),
        "{underdeclared_errors:?}"
    );

    let invalid_cleanup = parse_mir_dump(&valid.dump().replacen(
        "unwind cleanup:b1",
        "unwind cleanup:b99",
        1,
    ))
    .expect("invalid cleanup MIR");
    assert!(matches!(
        verify_mir_bubble(&invalid_cleanup, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidUnwindAction { .. }
                | MirVerificationError::InvalidBlock(_)
        ))
    ));

    assert!(parse_mir_dump(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> () effects[Unknown]\n  b0():\n    return ()\n"
    )
    .is_err());
}

#[test]
fn verifier_rejects_non_reference_roots_wrong_allocation_maps_and_missing_barriers() {
    let types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let non_reference_root = parse_mir_dump(&format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0(t{integer}) -> () effects[GcSafePoint]\n  b0(v0:t{integer}):\n    do v1 gcSafePoint sp0 roots (v0)\n    return ()\n",
        integer = integer.raw(),
    ))
    .expect("non-reference stack root MIR");
    assert!(matches!(
        verify_mir_bubble(&non_reference_root, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidStackMapRoot { .. }
        ))
    ));

    let (arrays, array_types) = lower(
        "namespace Main\n\
         public function values(): {Int}\n\
             return { 1 }\n\
         end\n",
    );
    let wrong_map = parse_mir_dump(&arrays.dump().replacen(
        "arrayMake scalar",
        "arrayMake managed",
        1,
    ))
    .expect("wrong array map MIR");
    assert!(matches!(
        verify_mir_bubble(&wrong_map, &array_types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidObjectMap { .. }
        ))
    ));

    let (class, class_types) = lower(
        "namespace Main\n\
         public class Holder\n\
             public values: {Int}\n\
             public function Holder:set(values: {Int})\n\
                 self.values = values\n\
             end\n\
         end\n",
    );
    let dump = class.dump();
    let barrier = dump
        .lines()
        .find(|line| line.contains("writeBarrier"))
        .expect("explicit barrier");
    let missing_barrier = parse_mir_dump(
        &dump
            .lines()
            .filter(|line| *line != barrier)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("missing barrier MIR");
    assert!(matches!(
        verify_mir_bubble(&missing_barrier, &class_types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::MissingWriteBarrier { .. }
        ))
    ));
}
