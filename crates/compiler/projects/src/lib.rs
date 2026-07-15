//! Workspace, Package, Bubble, and Module project graphs.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use pop_foundation::{BubbleId, FileId, ModuleId, PackageId, WorkspaceId};

mod lock;
mod manifest;

pub use lock::{
    BubbleLock, LockError, LockMode, LockedBubble, LockedBubbleIdentity, LockedPackage,
    LockedSource, apply_lock_policy, decode_lock, encode_lock, sha256_hex,
};
pub use manifest::{
    BubbleKind, DependencyRequirement, DependencySource, DiscoveredBubble, ManifestError,
    NativeLibrary, NativeLibraryDiscovery, NativeLibraryKind, NativeLinkPlan, NativeLinkPlanError,
    PackageManifest, PlatformDependencies, PlatformNativeLibraries, WorkspaceManifest,
    discover_conventional_bubbles, discover_workspace_members, parse_package_manifest,
    parse_workspace_manifest,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Workspace {
    id: WorkspaceId,
    name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Package {
    id: PackageId,
    workspace: WorkspaceId,
    name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bubble {
    id: BubbleId,
    package: PackageId,
    name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Module {
    id: ModuleId,
    bubble: BubbleId,
    file: FileId,
    path: String,
}

macro_rules! entity_accessors {
    ($entity:ident, $id_type:ident) => {
        impl $entity {
            #[must_use]
            pub const fn id(&self) -> $id_type {
                self.id
            }

            #[must_use]
            pub fn name(&self) -> &str {
                &self.name
            }
        }
    };
}

entity_accessors!(Workspace, WorkspaceId);
entity_accessors!(Package, PackageId);
entity_accessors!(Bubble, BubbleId);

impl Package {
    #[must_use]
    pub const fn workspace(&self) -> WorkspaceId {
        self.workspace
    }
}

impl Bubble {
    #[must_use]
    pub const fn package(&self) -> PackageId {
        self.package
    }
}

impl Module {
    #[must_use]
    pub const fn id(&self) -> ModuleId {
        self.id
    }

    #[must_use]
    pub const fn bubble(&self) -> BubbleId {
        self.bubble
    }

    #[must_use]
    pub const fn file(&self) -> FileId {
        self.file
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectGraphError {
    DuplicateWorkspace(WorkspaceId),
    DuplicatePackage(PackageId),
    DuplicateBubble(BubbleId),
    DuplicateModule(ModuleId),
    UnknownWorkspace(WorkspaceId),
    UnknownPackage(PackageId),
    UnknownBubble(BubbleId),
    FileAlreadyOwned(FileId),
}

impl fmt::Display for ProjectGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid project graph operation: {self:?}")
    }
}

impl Error for ProjectGraphError {}

#[derive(Clone, Debug, Default)]
pub struct ProjectGraph {
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    packages: BTreeMap<PackageId, Package>,
    bubbles: BTreeMap<BubbleId, Bubble>,
    modules: BTreeMap<ModuleId, Module>,
    owned_files: BTreeSet<FileId>,
}

impl ProjectGraph {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            workspaces: BTreeMap::new(),
            packages: BTreeMap::new(),
            bubbles: BTreeMap::new(),
            modules: BTreeMap::new(),
            owned_files: BTreeSet::new(),
        }
    }

    /// Adds a Workspace identity.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectGraphError::DuplicateWorkspace`] for an existing ID.
    pub fn add_workspace(
        &mut self,
        id: WorkspaceId,
        name: impl Into<String>,
    ) -> Result<(), ProjectGraphError> {
        if self.workspaces.contains_key(&id) {
            return Err(ProjectGraphError::DuplicateWorkspace(id));
        }
        self.workspaces.insert(
            id,
            Workspace {
                id,
                name: name.into(),
            },
        );
        Ok(())
    }

    /// Adds a Package under exactly one Workspace.
    ///
    /// # Errors
    ///
    /// Returns a duplicate-ID or unknown-parent error.
    pub fn add_package(
        &mut self,
        id: PackageId,
        workspace: WorkspaceId,
        name: impl Into<String>,
    ) -> Result<(), ProjectGraphError> {
        if self.packages.contains_key(&id) {
            return Err(ProjectGraphError::DuplicatePackage(id));
        }
        if !self.workspaces.contains_key(&workspace) {
            return Err(ProjectGraphError::UnknownWorkspace(workspace));
        }
        self.packages.insert(
            id,
            Package {
                id,
                workspace,
                name: name.into(),
            },
        );
        Ok(())
    }

    /// Adds a Bubble under exactly one Package.
    ///
    /// # Errors
    ///
    /// Returns a duplicate-ID or unknown-parent error.
    pub fn add_bubble(
        &mut self,
        id: BubbleId,
        package: PackageId,
        name: impl Into<String>,
    ) -> Result<(), ProjectGraphError> {
        if self.bubbles.contains_key(&id) {
            return Err(ProjectGraphError::DuplicateBubble(id));
        }
        if !self.packages.contains_key(&package) {
            return Err(ProjectGraphError::UnknownPackage(package));
        }
        self.bubbles.insert(
            id,
            Bubble {
                id,
                package,
                name: name.into(),
            },
        );
        Ok(())
    }

    /// Adds one Module and establishes exclusive source-file ownership.
    ///
    /// # Errors
    ///
    /// Returns a duplicate-ID, duplicate-file, or unknown-parent error.
    pub fn add_module(
        &mut self,
        id: ModuleId,
        bubble: BubbleId,
        file: FileId,
        path: impl Into<String>,
    ) -> Result<(), ProjectGraphError> {
        if self.modules.contains_key(&id) {
            return Err(ProjectGraphError::DuplicateModule(id));
        }
        if !self.bubbles.contains_key(&bubble) {
            return Err(ProjectGraphError::UnknownBubble(bubble));
        }
        if !self.owned_files.insert(file) {
            return Err(ProjectGraphError::FileAlreadyOwned(file));
        }
        self.modules.insert(
            id,
            Module {
                id,
                bubble,
                file,
                path: path.into(),
            },
        );
        Ok(())
    }

    #[must_use]
    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.get(&id)
    }

    #[must_use]
    pub fn package(&self, id: PackageId) -> Option<&Package> {
        self.packages.get(&id)
    }

    #[must_use]
    pub fn bubble(&self, id: BubbleId) -> Option<&Bubble> {
        self.bubbles.get(&id)
    }

    #[must_use]
    pub fn module(&self, id: ModuleId) -> Option<&Module> {
        self.modules.get(&id)
    }

    #[must_use]
    pub fn module_bubble(&self, id: ModuleId) -> Option<BubbleId> {
        self.module(id).map(Module::bubble)
    }

    #[must_use]
    pub fn bubble_package(&self, id: BubbleId) -> Option<PackageId> {
        self.bubble(id).map(Bubble::package)
    }
}
