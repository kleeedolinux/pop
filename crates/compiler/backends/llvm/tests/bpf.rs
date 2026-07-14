use pop_backend_api::{RuntimeContract, RuntimeContractError};
use pop_backend_llvm::{
    BpfBackendError, BpfLoweringOptions, BpfUnsupportedReason, BpfValidationPass,
    lower_mir_to_bpf_module, xdp_pass,
};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::{lower_hir_bubble, optimize_mir};
use pop_source::SourceFile;
use pop_target::TargetSpec;

fn lower(source_text: &str) -> (pop_mir::MirBubble, pop_types::TypeArena) {
    lower_with_optimization(source_text, true)
}

fn lower_unoptimized(source_text: &str) -> (pop_mir::MirBubble, pop_types::TypeArena) {
    lower_with_optimization(source_text, false)
}

fn lower_with_optimization(
    source_text: &str,
    optimize: bool,
) -> (pop_mir::MirBubble, pop_types::TypeArena) {
    let source = SourceFile::new(FileId::from_raw(0), "src/main.pop", source_text).expect("source");
    let front_end = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(0),
            NamespaceId::from_raw(0),
            Vec::new(),
            vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
        )
        .with_implicit_main_entry(ModuleId::from_raw(0)),
    );
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let mir = if optimize {
        optimize_mir(mir, front_end.types()).expect("optimized MIR")
    } else {
        mir
    };
    (mir, front_end.types().clone())
}

fn bpfel() -> TargetSpec {
    TargetSpec::for_triple("bpfel-unknown-none").expect("BPF target")
}

#[test]
fn validates_and_lowers_minimal_xdp_pass_to_bpf_llvm_ir() {
    let (mir, types) = lower(
        "namespace Main\n\
         function main(): Int\n\
             return 2\n\
         end\n",
    );
    let entry = mir.functions()[0].symbol();
    let options = BpfLoweringOptions::xdp(entry);
    BpfValidationPass
        .validate(&mir, &types, &bpfel(), options)
        .expect("valid BPF MIR");
    let module = lower_mir_to_bpf_module(&mir, &types, &bpfel(), options).expect("BPF lowering");
    let text = module.as_llvm_ir();
    assert_eq!(module.triple(), "bpfel-unknown-none");
    assert!(text.contains("target triple = \"bpfel-unknown-none\""));
    assert!(text.contains("section \"xdp\""));
    assert!(text.contains("@pop_bpf_xdp"));
    assert!(text.contains(&format!("add i64 0, {}", xdp_pass())));
    assert!(!text.contains("pop_rt_"));
    assert!(!text.contains("@main("));
}

#[test]
fn rejects_floating_point_for_bpf() {
    let (mir, types) = lower_unoptimized(
        "namespace Main\n\
         function main(): Int\n\
             local value: Float64 = 1.0\n\
             return Int(value)\n\
         end\n",
    );
    let error = BpfValidationPass
        .validate(
            &mir,
            &types,
            &bpfel(),
            BpfLoweringOptions::xdp(mir.functions()[0].symbol()),
        )
        .expect_err("float is rejected");
    assert_eq!(error.diagnostic_code(), "POP7004");
    assert!(matches!(
        error,
        BpfBackendError::UnsupportedInstruction {
            reason: BpfUnsupportedReason::FloatingPoint,
            ..
        }
    ));
}

#[test]
fn rejects_stdlib_call_when_selected_runtime_profile_lacks_contract() {
    let (mir, types) = lower(
        "namespace Main\n\
         function main(): Int\n\
             print(\"hello\")\n\
             return 2\n\
         end\n",
    );
    let error = BpfValidationPass
        .validate(
            &mir,
            &types,
            &bpfel(),
            BpfLoweringOptions::xdp(mir.functions()[0].symbol()),
        )
        .expect_err("runtime call is rejected");
    assert_eq!(error.diagnostic_code(), "POP7006");
    assert!(matches!(
        error,
        BpfBackendError::RuntimeContract(RuntimeContractError::MissingContract {
            requirement,
            ..
        }) if requirement.contract() == RuntimeContract::StandardLibraryAdapters
    ));
}

#[test]
fn rejects_invalid_xdp_entry_signature() {
    let (mir, types) = lower(
        "namespace Main\n\
         private function entry(value: Int): Int\n\
             return value\n\
         end\n\
         function main(): Int\n\
             return 2\n\
         end\n",
    );
    let error = BpfValidationPass
        .validate(
            &mir,
            &types,
            &bpfel(),
            BpfLoweringOptions::xdp(mir.functions()[0].symbol()),
        )
        .expect_err("entry parameters are rejected");
    assert_eq!(error.diagnostic_code(), "POP7000");
}

#[test]
fn rejects_direct_recursion() {
    let (mir, types) = lower(
        "namespace Main\n\
         function main(): Int\n\
             return main()\n\
         end\n",
    );
    let error = BpfValidationPass
        .validate(
            &mir,
            &types,
            &bpfel(),
            BpfLoweringOptions::xdp(mir.functions()[0].symbol()),
        )
        .expect_err("recursion is rejected");
    assert_eq!(error.diagnostic_code(), "POP7005");
}
