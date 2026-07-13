#[path = "../benches/compilation_workload.rs"]
mod compilation_workload;

use compilation_workload::{
    CompilationStage, CompilationWorkloadConfiguration, CompilationWorkloadKind, prepare_workload,
};

#[test]
fn compilation_benchmark_inventory_is_closed_and_prepares_every_stage() {
    let names: Vec<_> = CompilationWorkloadKind::ALL
        .into_iter()
        .map(CompilationWorkloadKind::name)
        .collect();
    assert_eq!(
        names,
        [
            "many_functions",
            "large_bodies",
            "many_modules",
            "compile_time",
        ]
    );

    let configuration = CompilationWorkloadConfiguration {
        modules: 3,
        functions_per_module: 4,
        statements_per_function: 5,
    };
    let stage_names: Vec<_> = CompilationStage::ALL
        .into_iter()
        .map(CompilationStage::name)
        .collect();
    assert_eq!(
        stage_names,
        [
            "front_end",
            "hir_to_mir",
            "mir_optimize",
            "c_backend",
            "llvm_backend",
        ]
    );
    for name in names {
        let kind = CompilationWorkloadKind::parse(name).expect("known benchmark workload");
        assert_eq!(kind.name(), name);
        let prepared = prepare_workload(kind, configuration).expect("valid compiler workload");
        assert_eq!(prepared.logical_modules(), 3);
        assert!(prepared.logical_functions() >= 12);
        assert!(prepared.logical_statements() >= 60);
        assert!(prepared.source_bytes() > 0);
        assert!(prepared.hir().functions().len() >= 12);
        assert!(!prepared.mir().functions().is_empty());
        assert!(!prepared.optimized_mir().functions().is_empty());
        for stage_name in &stage_names {
            let stage = CompilationStage::parse(stage_name).expect("known benchmark stage");
            let observation = prepared
                .run_stage(stage)
                .expect("prepared compilation stage");
            assert!(observation.semantic_items > 0);
            if matches!(
                stage,
                CompilationStage::CBackend | CompilationStage::LlvmBackend
            ) {
                assert!(observation.output_bytes > 0);
            }
        }
    }
    assert!(CompilationWorkloadKind::parse("unknown").is_none());
    assert!(CompilationStage::parse("unknown").is_none());
}

#[test]
fn compilation_benchmark_rejects_empty_dimensions() {
    for configuration in [
        CompilationWorkloadConfiguration {
            modules: 0,
            functions_per_module: 1,
            statements_per_function: 1,
        },
        CompilationWorkloadConfiguration {
            modules: 1,
            functions_per_module: 0,
            statements_per_function: 1,
        },
        CompilationWorkloadConfiguration {
            modules: 1,
            functions_per_module: 1,
            statements_per_function: 0,
        },
    ] {
        assert!(prepare_workload(CompilationWorkloadKind::ManyFunctions, configuration).is_err());
    }
}
