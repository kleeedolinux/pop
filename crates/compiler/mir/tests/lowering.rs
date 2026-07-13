use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::{
    MirDeclarationKind, MirInstruction, MirInstructionKind, MirTerminator, MirVerificationError,
    lower_hir_bubble, parse_mir_dump, verify_mir_bubble,
};
use pop_source::SourceFile;

#[test]
fn structured_hir_lowers_to_explicit_verified_cfg_in_source_evaluation_order() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\n\
         public function calculate(left: Int, right: Int): Int\n\
             local sum = left + right\n\
             if left < right then\n\
                 return sum\n\
             else\n\
                 while false do\n\
                     right\n\
                 end\n\
                 return right\n\
             end\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");

    assert!(verify_mir_bubble(&mir, front_end.types()).is_ok());
    let function = &mir.functions()[0];
    assert!(function.blocks().len() >= 6);
    assert!(matches!(
        function.blocks()[0].instructions()[0].kind(),
        MirInstructionKind::CheckedIntegerAdd { .. }
    ));
    assert!(matches!(
        function.blocks()[0].terminator(),
        MirTerminator::ConditionalBranch { .. }
    ));
    assert!(
        function
            .blocks()
            .iter()
            .all(|block| !matches!(block.terminator(), MirTerminator::Missing))
    );
    let dump = mir.dump();
    assert_eq!(dump, mir.dump());
    assert!(dump.contains("integer.checkedAdd Int64"));
    assert!(dump.contains("condBranch"));
    assert!(!dump.to_ascii_lowercase().contains("dynamic"));
    assert!(!dump.to_ascii_lowercase().contains("llvm"));

    let reparsed = parse_mir_dump(&dump).expect("MIR dump parses");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, front_end.types()).is_ok());

    let malformed = dump.replacen(" b1 b2", " b999 b2", 1);
    let malformed = parse_mir_dump(&malformed).expect("structurally parseable malformed MIR");
    assert!(matches!(
        verify_mir_bubble(&malformed, front_end.types()),
        Err(errors) if errors.contains(&MirVerificationError::InvalidBlock(pop_foundation::BlockId::from_raw(999)))
    ));
}

#[test]
fn fixed_packs_preserve_hir_grouping_and_lower_to_tuple_projection() {
    // ADR 0045: HIR owns grouped assignment order and MIR exposes only typed
    // tuple construction/projection plus ordinary stores.
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/fixedPack.pop",
        "namespace Main\n\
         private function split(value: Int): (Int, Int)\n\
             return value, value + 1\n\
         end\n\
         public function exchange(value: Int): Int\n\
             local left, right = split(value)\n\
             local result = split(value)\n\
             local projected = result[2]\n\
             left, right = right, left\n\
             return left + projected\n\
         end\n",
    )
    .expect("source");
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
    let hir = front_end.hir().expect("HIR");
    let hir_dump = hir.dump(front_end.types());
    assert!(hir_dump.contains("multipleLocal"), "{hir_dump}");
    assert!(hir_dump.contains("multipleAssignment"), "{hir_dump}");
    assert!(hir_dump.contains("tuple.get 1"), "{hir_dump}");

    let mir = lower_hir_bubble(hir, front_end.types()).expect("verified MIR");
    let dump = mir.dump();
    assert!(dump.contains("tupleMake"), "{dump}");
    assert!(dump.contains("tupleGet 0"), "{dump}");
    assert!(dump.contains("tupleGet 1"), "{dump}");
    assert!(!dump.contains("multipleAssignment"), "{dump}");
    assert!(verify_mir_bubble(&mir, front_end.types()).is_ok());
    let reparsed = parse_mir_dump(&dump).expect("MIR dump parses");
    assert_eq!(reparsed.dump(), dump);
}

#[test]
fn typed_table_access_lowers_to_verified_optional_get_and_insert_or_replace_set() {
    // ADR 0046 keeps associative access typed and backend-neutral.
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/tables.pop",
        "namespace Main\n\
         public function lookup(key: String): Int?\n\
             local scores: {[String]: Int} = { alice = 10 }\n\
             scores[\"alice\"] = 11\n\
             scores[\"bruno\"] = 12\n\
             return scores[key]\n\
         end\n",
    )
    .expect("source");
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
    let hir = front_end.hir().expect("HIR");
    let hir_dump = hir.dump(front_end.types());
    assert!(hir_dump.contains("table.set"), "{hir_dump}");
    assert!(hir_dump.contains("table.get"), "{hir_dump}");

    let mir = lower_hir_bubble(hir, front_end.types()).expect("verified MIR");
    let dump = mir.dump();
    assert!(dump.contains("tableSet managed scalar"), "{dump}");
    assert!(dump.contains("tableGet"), "{dump}");
    assert!(verify_mir_bubble(&mir, front_end.types()).is_ok());
    let reparsed = parse_mir_dump(&dump).expect("MIR dump parses");
    assert_eq!(reparsed.dump(), dump);
}

#[test]
fn optional_control_lowers_to_typed_presence_cfg_without_backend_reconstruction() {
    // ADR 0051: optional binding/defaulting/propagation all become explicit
    // presence tests and dominated typed extraction in canonical MIR.
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/optionals.pop",
        "namespace Main\n\
         private function choose(value: String?, fallback: String): String?\n\
             local selected = value ?? fallback\n\
             local propagated = value?\n\
             if local bound = value then\n\
                 use(bound)\n\
             end\n\
             while local bound = value do\n\
                 use(bound)\n\
                 break\n\
             end\n\
             if value ~= nil then\n\
                 use(value)\n\
             end\n\
             return value\n\
         end\n\
         private function use(value: String)\n\
         end\n",
    )
    .expect("source");
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
    let hir_dump = front_end
        .hir()
        .expect("optional HIR")
        .dump(front_end.types());
    assert!(hir_dump.contains("optional.default"), "{hir_dump}");
    assert!(hir_dump.contains("optional.propagate"), "{hir_dump}");
    assert!(hir_dump.contains("optionalIf"), "{hir_dump}");
    assert!(hir_dump.contains("optionalWhile"), "{hir_dump}");

    let mir = lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types())
        .expect("verified optional MIR");
    let dump = mir.dump();
    assert!(dump.contains("optionalIsPresent"), "{dump}");
    assert!(dump.contains("optionalGet"), "{dump}");
    assert!(dump.contains("condBranch"), "{dump}");
    assert!(!dump.contains("optionalIf"), "{dump}");
    assert!(!dump.contains("optionalWhile"), "{dump}");
    assert!(!dump.to_ascii_lowercase().contains("dynamic"), "{dump}");
    assert!(!dump.to_ascii_lowercase().contains("llvm"), "{dump}");
    assert!(verify_mir_bubble(&mir, front_end.types()).is_ok());
    let reparsed = parse_mir_dump(&dump).expect("optional MIR dump parses");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, front_end.types()).is_ok());

    let unguarded = dump.replacen("optionalIsPresent v0", "compareEqual v0 v0", 1);
    let unguarded = parse_mir_dump(&unguarded).expect("unguarded optional MIR parses");
    assert!(matches!(
        verify_mir_bubble(&unguarded, front_end.types()),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::OptionalGetWithoutPresence { .. }
        ))
    ));
}

#[test]
fn repeat_until_lowers_to_portable_body_condition_exit_and_backedge_cfg() {
    // ADR 0032 deliberately keeps repeat-until out of the MIR instruction set.
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/repeat_until.pop",
        "namespace Main\n\
         public function count(): Int\n\
             local value = 0\n\
             repeat\n\
                 value = value + 1\n\
             until value == 3\n\
             return value\n\
         end\n",
    )
    .expect("source");
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
    let hir = front_end.hir().expect("repeat-until HIR");
    let hir_dump = hir.dump(front_end.types());
    assert!(hir_dump.contains("repeat"), "{hir_dump}");
    let mir = lower_hir_bubble(hir, front_end.types()).expect("verified repeat-until MIR");

    assert!(verify_mir_bubble(&mir, front_end.types()).is_ok());
    let function = &mir.functions()[0];
    let backedge = function
        .blocks()
        .iter()
        .find(|block| {
            matches!(
                block.terminator(),
                MirTerminator::Branch { target, .. } if *target <= block.block()
            )
        })
        .expect("repeat condition reaches a CFG backedge");
    assert!(
        matches!(
            backedge.instructions().last().map(MirInstruction::kind),
            Some(MirInstructionKind::GcSafePoint { .. })
        ),
        "repeat backedge requires a GC safe point: {}",
        mir.dump()
    );

    let dump = mir.dump();
    assert!(dump.contains("condBranch"), "{dump}");
    assert!(!dump.contains("repeat"), "{dump}");
    assert!(!dump.contains("until"), "{dump}");
    assert!(!dump.to_ascii_lowercase().contains("llvm"), "{dump}");
    let reparsed = parse_mir_dump(&dump).expect("repeat-until MIR dump parses");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, front_end.types()).is_ok());
}

#[test]
fn collections_lower_to_typed_portable_operations_and_round_trip() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/collections.pop",
        "namespace Main\n\
         public function collections(): ({String}, {[String]: Int})\n\
             local names: {String} = { \"first\", \"second\" }\n\
             local scores: {[String]: Int} = { first = 1, second = 2 }\n\
             names[2] = \"updated\"\n\
             local firstName: String? = names[1]\n\
             local numbers = Array.create<<Int>>(4, 0)\n\
             Array.fill(numbers, 7)\n\
             local count = Array.length(numbers)\n\
             local first = Array.get(numbers, 1)\n\
             return (names, scores)\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let instructions = mir.functions()[0].blocks()[0].instructions();
    let array_position = instructions
        .iter()
        .position(|instruction| matches!(instruction.kind(), MirInstructionKind::ArrayMake { .. }))
        .expect("array operation");
    assert!(matches!(
        instructions[array_position].kind(),
        MirInstructionKind::ArrayMake { elements, .. }
            if elements.len() == 2
    ));
    let table_position = instructions
        .iter()
        .position(|instruction| matches!(instruction.kind(), MirInstructionKind::TableMake { .. }))
        .expect("table operation");
    assert!(matches!(
        instructions[table_position].kind(),
        MirInstructionKind::TableMake { entries, .. }
            if entries.len() == 2
    ));
    let array_get_position = instructions
        .iter()
        .position(|instruction| matches!(instruction.kind(), MirInstructionKind::ArrayGet { .. }))
        .expect("array get operation");
    assert!(matches!(
        instructions[array_get_position].kind(),
        MirInstructionKind::ArrayGet { array, index }
            if *array == instructions[array_position].result()
                && *index == instructions[array_get_position - 1].result()
    ));

    let dump = mir.dump();
    assert!(dump.contains("arrayMake"));
    assert!(dump.contains("tableMake"));
    assert!(dump.contains("arrayGet"));
    assert!(dump.contains("arraySet"));
    assert!(dump.contains("arrayCreate scalar"));
    assert!(dump.contains("arrayFill scalar"));
    assert!(dump.contains("arrayLength"));
    assert!(dump.contains("arrayGetChecked"));
    let reparsed = parse_mir_dump(&dump).expect("collection MIR parses");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, front_end.types()).is_ok());

    assert_malformed_collection_operands_are_rejected(
        &dump,
        front_end.types(),
        instructions,
        array_position,
        array_get_position,
    );
}

fn assert_malformed_collection_operands_are_rejected(
    dump: &str,
    types: &pop_types::TypeArena,
    instructions: &[MirInstruction],
    array_position: usize,
    array_get_position: usize,
) {
    let string = types.source_type("String").expect("String");
    let integer = types.source_type("Int").expect("Int");
    let malformed = dump.replacen(
        &format!("v0:t{} = const.string", string.raw()),
        &format!("v0:t{} = const.string", integer.raw()),
        1,
    );
    assert_ne!(malformed, dump);
    let malformed = parse_mir_dump(&malformed).expect("structurally valid malformed MIR");
    let first_element = match instructions[array_position].kind() {
        MirInstructionKind::ArrayMake { elements, .. } => elements[0],
        _ => unreachable!("array instruction"),
    };
    assert!(matches!(
        verify_mir_bubble(&malformed, types),
        Err(errors) if errors.contains(&MirVerificationError::WrongOperandType {
            instruction: instructions[array_position].result(),
            operand: first_element,
            expected: string,
            found: integer,
        })
    ));

    let index = instructions[array_get_position - 1].result();
    let boolean = types.source_type("Boolean").expect("Boolean");
    let malformed_index = dump.replacen(
        &format!(
            "v{}:t{} = const.integer Int64 1",
            index.raw(),
            integer.raw()
        ),
        &format!(
            "v{}:t{} = const.integer Int64 1",
            index.raw(),
            boolean.raw()
        ),
        1,
    );
    assert_ne!(malformed_index, dump);
    let malformed_index =
        parse_mir_dump(&malformed_index).expect("structurally valid malformed MIR");
    assert!(matches!(
        verify_mir_bubble(&malformed_index, types),
        Err(errors) if errors.contains(&MirVerificationError::WrongOperandType {
            instruction: instructions[array_get_position].result(),
            operand: index,
            expected: integer,
            found: boolean,
        })
    ));
}

#[test]
fn native_methods_lower_by_method_id_and_round_trip_with_their_bodies() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/counter.pop",
        "namespace Main\n\
         public class Counter\n\
             public value: Int\n\
             public function Counter.new(value: Int): Counter\n\
                 return Counter { value = value }\n\
             end\n\
             public function Counter:add(delta: Int): Counter\n\
                 self.value = self.value + delta\n\
                 return self\n\
             end\n\
             public function Counter:get(): Int\n\
                 return self.value\n\
             end\n\
         end\n\
         public function read(value: Int): Int\n\
             local counter = Counter.new(value)\n\
             counter:add(2)\n\
             return counter:get()\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");

    assert_eq!(mir.methods().len(), 3);
    let calls: Vec<_> = mir.functions()[0].blocks()[0]
        .instructions()
        .iter()
        .filter_map(|instruction| match instruction.kind() {
            MirInstructionKind::CallDirectMethod { method, .. } => Some(*method),
            _ => None,
        })
        .collect();
    assert_eq!(
        calls,
        [
            mir.methods()[0].method(),
            mir.methods()[1].method(),
            mir.methods()[2].method()
        ]
    );
    let dump = mir.dump();
    assert!(dump.contains("method m0 c0"));
    assert!(dump.contains("fieldSet"));
    assert!(dump.contains("callDirectMethod m2"));
    let reparsed = parse_mir_dump(&dump).expect("method MIR parses");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, front_end.types()).is_ok());
}

#[test]
fn typed_function_values_lower_to_indirect_calls_and_round_trip() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/functions.pop",
        "namespace Main\n\
         private function increment(value: Int): Int\n\
             return value + 1\n\
         end\n\
         public function apply(operation: function(value: Int): Int, value: Int): Int\n\
             return operation(value)\n\
         end\n\
         public function run(value: Int): Int\n\
             local operation: function(value: Int): Int = increment\n\
             return apply(operation, value)\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let dump = mir.dump();

    assert!(dump.contains("callIndirect"));
    let reparsed = parse_mir_dump(&dump).expect("indirect-call MIR parses");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, front_end.types()).is_ok());

    let apply = &mir.functions()[1];
    let callable = apply.parameters()[0];
    let integer = apply.parameters()[1];
    let malformed = dump.replacen(
        &format!("b0(v0:t{}, v1:t{})", callable.raw(), integer.raw()),
        &format!("b0(v0:t{}, v1:t{})", integer.raw(), integer.raw()),
        1,
    );
    assert_ne!(malformed, dump);
    let malformed = parse_mir_dump(&malformed).expect("structurally valid malformed MIR");
    assert!(matches!(
        verify_mir_bubble(&malformed, front_end.types()),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidCallableOperand { found, .. } if *found == integer
        ))
    ));

    let malformed_reference = dump.replacen(
        &format!(":t{} = functionReference s0", callable.raw()),
        &format!(":t{} = functionReference s0", integer.raw()),
        1,
    );
    assert_ne!(malformed_reference, dump);
    let malformed_reference =
        parse_mir_dump(&malformed_reference).expect("structurally valid malformed reference");
    assert!(matches!(
        verify_mir_bubble(&malformed_reference, front_end.types()),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidInstructionType { result_type, .. }
                if *result_type == integer
        ))
    ));
}

#[test]
fn typed_equality_lowers_to_portable_comparisons_and_round_trips() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/equality.pop",
        "namespace Main\n\
         public function equal(left: Int, right: Int): Boolean\n\
             return left == right\n\
         end\n\
         public function different(left: String, right: String): Boolean\n\
             return left ~= right\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let dump = mir.dump();

    assert!(dump.contains("compareEqual"));
    assert!(dump.contains("compareNotEqual"));
    let reparsed = parse_mir_dump(&dump).expect("equality MIR parses");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, front_end.types()).is_ok());

    let boolean = front_end.types().source_type("Boolean").expect("Boolean");
    let integer = front_end.types().source_type("Int").expect("Int");
    let malformed_result = dump.replacen(
        &format!(":t{} = compareEqual", boolean.raw()),
        &format!(":t{} = compareEqual", integer.raw()),
        1,
    );
    assert_ne!(malformed_result, dump);
    let malformed_result =
        parse_mir_dump(&malformed_result).expect("structurally valid malformed equality");
    assert!(matches!(
        verify_mir_bubble(&malformed_result, front_end.types()),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidInstructionType { result_type, .. }
                if *result_type == integer
        ))
    ));
}

#[test]
fn logical_operators_lower_to_short_circuit_cfg_and_round_trip() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/logical.pop",
        "namespace Main\n\
         private function identity(value: Boolean): Boolean\n\
             return value\n\
         end\n\
         public function choose(left: Boolean, right: Boolean): (Boolean, Boolean)\n\
             return (left and identity(right), left or identity(right))\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let dump = mir.dump();

    assert!(!dump.contains("booleanAnd"));
    assert!(!dump.contains("booleanOr"));
    assert!(dump.matches("condBranch").count() >= 2);
    assert!(dump.lines().any(|line| line.trim_start().starts_with('b')
        && line.contains('v')
        && line.contains(":t")));
    let reparsed = parse_mir_dump(&dump).expect("short-circuit MIR parses");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, front_end.types()).is_ok());
}

#[test]
fn runtime_type_declarations_survive_mir_lowering_and_round_trip() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/declarations.pop",
        "namespace Main\n\
         public attribute Marker(value: Int = 1)\n\
         public record Point\n\
             x: Int\n\
         end\n\
         public union State\n\
             Idle\n\
             Ready(value: Int)\n\
         end\n\
         public class Counter\n\
             public value: Int\n\
         end\n\
         public function point(value: Int): Point\n\
             return { x = value }\n\
         end\n\
         public function state(value: Int): State\n\
             return State.Ready(value)\n\
         end\n\
         public function counter(value: Int): Counter\n\
             return Counter { value = value }\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");

    assert_eq!(mir.declarations().len(), 3);
    assert!(matches!(
        mir.declarations()[0].kind(),
        MirDeclarationKind::Record(_)
    ));
    assert!(matches!(
        mir.declarations()[1].kind(),
        MirDeclarationKind::Union(_)
    ));
    assert!(matches!(
        mir.declarations()[2].kind(),
        MirDeclarationKind::Class(_)
    ));
    let dump = mir.dump();
    assert!(dump.contains("type.record"));
    assert!(dump.contains("type.union"));
    assert!(dump.contains("type.class"));
    assert!(!dump.contains("Marker"));
    let reparsed = parse_mir_dump(&dump).expect("declaration MIR parses");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, front_end.types()).is_ok());

    for (malformed, expected) in [
        (
            dump.replacen("recordMake s1 {field#0=", "recordMake s1 {field#99=", 1),
            "field",
        ),
        (
            dump.replacen("unionMake s2 case#1 ", "unionMake s2 case#99 ", 1),
            "case",
        ),
        (
            dump.replacen(
                "classMake c0 map[1:] {field#1=",
                "classMake c0 map[1:] {field#99=",
                1,
            ),
            "field",
        ),
    ] {
        assert_ne!(malformed, dump);
        let malformed = parse_mir_dump(&malformed).expect("structurally valid malformed schema");
        let errors = verify_mir_bubble(&malformed, front_end.types()).expect_err("schema error");
        assert!(
            errors.iter().any(|error| match (expected, error) {
                ("field", MirVerificationError::UnknownField { field, .. }) => field.raw() == 99,
                ("case", MirVerificationError::UnknownUnionCase { case, .. }) => case.raw() == 99,
                _ => false,
            }),
            "{errors:?}"
        );
    }
}

#[test]
fn record_defaults_are_materialized_before_verified_mir() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/defaults.pop",
        "namespace Main\n\
         public record Options\n\
             name: String\n\
             attempts: Int = 3\n\
             enabled: Boolean = true\n\
         end\n\
         public function options(): Options\n\
             return { name = \"pop\", }\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let record = &mir.functions()[0].blocks()[0].instructions();
    let fields = record
        .iter()
        .find_map(|instruction| match instruction.kind() {
            MirInstructionKind::RecordMake { fields, .. } => Some(fields),
            _ => None,
        })
        .expect("record construction");

    assert_eq!(fields.len(), 3);
    let dump = mir.dump();
    let reparsed = parse_mir_dump(&dump).expect("record-default MIR parses");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, front_end.types()).is_ok());
}

#[test]
fn zero_result_calls_lower_to_explicit_effect_instructions_and_round_trip() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/resultless.pop",
        "namespace Main\n\
         private function discard(value: Int)\n\
             value\n\
         end\n\
         private function invoke(operation: function(value: Int), value: Int)\n\
             discard(value)\n\
             operation(value)\n\
         end\n\
         public function run()\n\
             local operation: function(value: Int) = discard\n\
             invoke(operation, 1)\n\
         end\n\
         private function identity(value: Int): Int\n\
             return value\n\
         end\n\
         public function value(): Int\n\
             return identity(1)\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let dump = mir.dump();
    let effects = dump
        .lines()
        .filter(|line| {
            line.trim_start().starts_with("do v")
                && (line.contains("callDirect") || line.contains("callIndirect"))
        })
        .collect::<Vec<_>>();

    assert_eq!(effects.len(), 3);
    assert!(effects.iter().any(|line| line.contains("callDirect s0")));
    assert!(effects.iter().any(|line| line.contains("callIndirect")));
    assert!(effects.iter().any(|line| line.contains("callDirect s1")));
    let reparsed = parse_mir_dump(&dump).expect("resultless-call MIR parses");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, front_end.types()).is_ok());

    let integer = front_end.types().source_type("Int").expect("Int");
    let effect_line = dump
        .lines()
        .find(|line| line.contains("do v") && line.contains("callDirect s0"))
        .expect("zero-result direct call");
    let effect = effect_line.trim().strip_prefix("do ").expect("effect");
    let (instruction, operation) = effect.split_once(' ').expect("effect operation");
    let malformed_result = dump.replacen(
        effect_line,
        &format!("    {instruction}:t{} = {operation}", integer.raw()),
        1,
    );
    let malformed_result =
        parse_mir_dump(&malformed_result).expect("structurally valid resultful call");
    assert!(matches!(
        verify_mir_bubble(&malformed_result, front_end.types()),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidCallSignature {
                expected_results: 0,
                found_results: 1,
                ..
            }
        ))
    ));

    let result_line = dump
        .lines()
        .find(|line| line.contains(" = callDirect s3"))
        .expect("one-result direct call");
    let result = result_line.trim();
    let (result, operation) = result.split_once(" = ").expect("result operation");
    let instruction = result.split_once(':').expect("typed result").0;
    let malformed_effect =
        dump.replacen(result_line, &format!("    do {instruction} {operation}"), 1);
    let malformed_effect =
        parse_mir_dump(&malformed_effect).expect("structurally valid resultless call");
    assert!(matches!(
        verify_mir_bubble(&malformed_effect, front_end.types()),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidCallSignature {
                expected_results: 1,
                found_results: 0,
                ..
            }
        ))
    ));
}

#[test]
fn fixed_width_integer_and_float_operations_have_explicit_portable_mir_kinds() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/numeric.pop",
        "namespace Main\n\
         public function addByte(left: UInt8, right: UInt8): UInt8\n\
             return left + right\n\
         end\n\
         public function lessUnsigned(left: UInt64, right: UInt64): Boolean\n\
             return left < right\n\
         end\n\
         public function addSingle(left: Float32, right: Float32): Float32\n\
             return left + right\n\
         end\n\
         public function divideDouble(left: Float64, right: Float64): Float64\n\
             return left / right\n\
         end\n\
         public function singleOne(): Float32\n\
             return 1\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let dump = mir.dump();

    assert!(dump.contains("integer.checkedAdd UInt8"));
    assert!(dump.contains("integer.compareLess UInt64"));
    assert!(dump.contains("float.add Float32"));
    assert!(dump.contains("float.divide Float64"));
    assert!(dump.contains("const.float Float32 0x3f800000"));
    assert!(!dump.contains("const.int "));
    let reparsed = parse_mir_dump(&dump).expect("numeric MIR parses");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, front_end.types()).is_ok());

    let uint8 = front_end.types().source_type("UInt8").expect("UInt8");
    let float32 = front_end.types().source_type("Float32").expect("Float32");
    let boolean = front_end.types().source_type("Boolean").expect("Boolean");

    let malformed_integer_kind =
        dump.replacen("integer.checkedAdd UInt8", "integer.checkedAdd Int8", 1);
    let malformed_integer_kind =
        parse_mir_dump(&malformed_integer_kind).expect("malformed integer MIR parses");
    assert!(matches!(
        verify_mir_bubble(&malformed_integer_kind, front_end.types()),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidInstructionType { result_type, .. }
                if *result_type == uint8
        ))
    ));

    let malformed_float_kind = dump.replacen("float.add Float32", "float.add Float64", 1);
    let malformed_float_kind =
        parse_mir_dump(&malformed_float_kind).expect("malformed float MIR parses");
    assert!(matches!(
        verify_mir_bubble(&malformed_float_kind, front_end.types()),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidInstructionType { result_type, .. }
                if *result_type == float32
        ))
    ));

    let malformed_constant = dump.replacen(
        &format!(":t{} = const.float Float32", float32.raw()),
        &format!(":t{} = const.float Float32", boolean.raw()),
        1,
    );
    let malformed_constant =
        parse_mir_dump(&malformed_constant).expect("malformed numeric constant parses");
    assert!(matches!(
        verify_mir_bubble(&malformed_constant, front_end.types()),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidInstructionType { result_type, .. }
                if *result_type == boolean
        ))
    ));

    let malformed_comparison = dump.replacen(
        &format!(":t{} = integer.compareLess UInt64", boolean.raw()),
        &format!(":t{} = integer.compareLess UInt64", uint8.raw()),
        1,
    );
    let malformed_comparison =
        parse_mir_dump(&malformed_comparison).expect("malformed comparison parses");
    assert!(matches!(
        verify_mir_bubble(&malformed_comparison, front_end.types()),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidInstructionType { result_type, .. }
                if *result_type == uint8
        ))
    ));
}
