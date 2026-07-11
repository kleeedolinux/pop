use pop_foundation::{BubbleId, FileId, ModuleId, SourceSpan, SymbolId, TextRange, TextSize};
use pop_resolve::{Declaration, DeclarationKind, Visibility};

fn span() -> SourceSpan {
    SourceSpan::new(
        FileId::from_raw(0),
        TextRange::new(TextSize::from_u32(0), TextSize::from_u32(4)).expect("range"),
    )
}

fn declaration(visibility: Visibility) -> Declaration {
    Declaration::new(
        SymbolId::from_raw(1),
        ModuleId::from_raw(2),
        BubbleId::from_raw(3),
        "loadPlayer",
        DeclarationKind::Function,
        visibility,
        span(),
    )
}

#[test]
fn public_is_visible_to_dependent_bubbles() {
    assert!(
        declaration(Visibility::Public)
            .is_accessible_from(ModuleId::from_raw(90), BubbleId::from_raw(80))
    );
}

#[test]
fn internal_stops_at_the_bubble_boundary() {
    let declaration = declaration(Visibility::Internal);

    assert!(declaration.is_accessible_from(ModuleId::from_raw(90), BubbleId::from_raw(3)));
    assert!(!declaration.is_accessible_from(ModuleId::from_raw(2), BubbleId::from_raw(4)));
}

#[test]
fn private_stops_at_the_module_boundary() {
    let declaration = declaration(Visibility::Private);

    assert!(declaration.is_accessible_from(ModuleId::from_raw(2), BubbleId::from_raw(3)));
    assert!(!declaration.is_accessible_from(ModuleId::from_raw(4), BubbleId::from_raw(3)));
}

#[test]
fn only_public_declarations_enter_reference_metadata() {
    assert!(declaration(Visibility::Public).is_in_public_reference_surface());
    assert!(!declaration(Visibility::Internal).is_in_public_reference_surface());
    assert!(!declaration(Visibility::Private).is_in_public_reference_surface());
}
