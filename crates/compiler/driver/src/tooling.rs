//! Version-coupled compiler references for private tooling queries.

use std::sync::OnceLock;

use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_source::SourceFile;

use crate::{FrontEndBubbleInput, FrontEndModule, ReferenceMetadata, analyze_bubble};

const INTERNAL_BUBBLE: BubbleId = BubbleId::from_raw(1);
pub const TOOLING_STANDARD_BUBBLE: BubbleId = BubbleId::from_raw(2);

static STANDARD_REFERENCE: OnceLock<ReferenceMetadata> = OnceLock::new();

/// Returns the compiler-verified public `Pop.Standard` reference used by
/// version-coupled editor queries.
///
/// The source is embedded into the tool binary so analysis never depends on a
/// checkout path, ambient Package cache, or CLI output. Repository validation
/// and architecture tests make malformed embedded Standard source a toolchain
/// incident rather than a user diagnostic.
///
/// # Panics
///
/// Panics only when the compiler-coupled embedded `Pop.Standard` sources exceed
/// typed ID bounds, fail their repository-validated analysis, or cannot publish
/// their required public reference metadata.
#[must_use]
pub fn tooling_standard_reference_metadata() -> &'static ReferenceMetadata {
    STANDARD_REFERENCE.get_or_init(|| {
        let modules = [
            (
                "Pop.Standard/src/lib.pop",
                include_str!("../../../libraries/standard/pop/src/lib.pop"),
            ),
            (
                "Pop.Standard/src/math.pop",
                include_str!("../../../libraries/standard/pop/src/math.pop"),
            ),
            (
                "Pop.Standard/src/sequence.pop",
                include_str!("../../../libraries/standard/pop/src/sequence.pop"),
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (path, text))| {
            let raw = u32::try_from(index).expect("embedded Standard Module count is bounded");
            let file = u32::MAX
                .checked_sub(raw)
                .expect("embedded Standard FileId range is bounded");
            let source = SourceFile::new(FileId::from_raw(file), path, text)
                .expect("repository-validated embedded Pop.Standard source");
            FrontEndModule::new(ModuleId::from_raw(raw), source)
        })
        .collect();
        let result = analyze_bubble(FrontEndBubbleInput::new(
            TOOLING_STANDARD_BUBBLE,
            NamespaceId::from_raw(TOOLING_STANDARD_BUBBLE.raw()),
            vec![INTERNAL_BUBBLE],
            modules,
        ));
        assert!(
            result.diagnostics().is_empty(),
            "repository-validated embedded Pop.Standard must analyze without diagnostics: {}",
            result.diagnostic_snapshot()
        );
        result
            .reference_metadata()
            .expect("verified Pop.Standard publishes reference metadata")
            .clone()
    })
}
