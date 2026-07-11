use pop_foundation::{BubbleId, FileId, ModuleId, PackageId, WorkspaceId};
use pop_projects::{ProjectGraph, ProjectGraphError};

#[test]
fn ownership_is_item_module_bubble_package_workspace_without_widening() {
    let mut graph = ProjectGraph::new();
    graph
        .add_workspace(WorkspaceId::from_raw(0), "Game")
        .expect("workspace");
    graph
        .add_package(
            PackageId::from_raw(0),
            WorkspaceId::from_raw(0),
            "Game.Server",
        )
        .expect("package");
    graph
        .add_bubble(
            BubbleId::from_raw(0),
            PackageId::from_raw(0),
            "Game.Server.Core",
        )
        .expect("core bubble");
    graph
        .add_bubble(
            BubbleId::from_raw(1),
            PackageId::from_raw(0),
            "Game.Server.Tool",
        )
        .expect("tool bubble");
    graph
        .add_module(
            ModuleId::from_raw(0),
            BubbleId::from_raw(0),
            FileId::from_raw(0),
            "src/core.pop",
        )
        .expect("module");

    assert_eq!(
        graph.module_bubble(ModuleId::from_raw(0)),
        Some(BubbleId::from_raw(0))
    );
    assert_eq!(
        graph.bubble_package(BubbleId::from_raw(0)),
        Some(PackageId::from_raw(0))
    );
    assert_eq!(
        graph.bubble_package(BubbleId::from_raw(1)),
        Some(PackageId::from_raw(0))
    );
    assert_ne!(BubbleId::from_raw(0), BubbleId::from_raw(1));
}

#[test]
fn a_source_file_cannot_be_owned_by_two_modules() {
    let mut graph = ProjectGraph::new();
    graph
        .add_workspace(WorkspaceId::from_raw(0), "Game")
        .expect("workspace");
    graph
        .add_package(PackageId::from_raw(0), WorkspaceId::from_raw(0), "Game")
        .expect("package");
    graph
        .add_bubble(BubbleId::from_raw(0), PackageId::from_raw(0), "Game.Core")
        .expect("bubble");
    graph
        .add_module(
            ModuleId::from_raw(0),
            BubbleId::from_raw(0),
            FileId::from_raw(7),
            "src/main.pop",
        )
        .expect("first owner");

    let error = graph
        .add_module(
            ModuleId::from_raw(1),
            BubbleId::from_raw(0),
            FileId::from_raw(7),
            "other/path.pop",
        )
        .expect_err("duplicate file ownership must fail");
    assert_eq!(
        error,
        ProjectGraphError::FileAlreadyOwned(FileId::from_raw(7))
    );
}

#[test]
fn missing_parents_are_rejected_deterministically() {
    let mut graph = ProjectGraph::new();
    let error = graph
        .add_package(PackageId::from_raw(1), WorkspaceId::from_raw(99), "Missing")
        .expect_err("unknown workspace");

    assert_eq!(
        error,
        ProjectGraphError::UnknownWorkspace(WorkspaceId::from_raw(99))
    );
}
