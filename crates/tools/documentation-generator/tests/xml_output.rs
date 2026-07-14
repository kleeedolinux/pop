use pop_documentation::XmlFragment;
use pop_documentation_generator::{DocumentationMember, DocumentationOutputError, render_xml};

#[test]
fn documentation_xml_is_sorted_deterministic_and_schema_versioned() {
    let later = DocumentationMember::new(
        "function:Studio.Gameplay.zeta()",
        XmlFragment::parse("<summary>Later &amp; safe.</summary>").expect("safe fragment"),
    );
    let earlier = DocumentationMember::new(
        "function:Studio.Gameplay.alpha(Int)",
        XmlFragment::parse(
            "<summary>Returns <see cref=\"Studio.Value\"/>.</summary><param name=\"value\">Input.</param>",
        )
        .expect("safe fragment"),
    );

    let first = render_xml("Studio.Gameplay", &[later.clone(), earlier.clone()])
        .expect("documentation output");
    let second = render_xml("Studio.Gameplay", &[earlier, later]).expect("stable ordering");
    assert_eq!(first, second);
    assert_eq!(
        first,
        concat!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n",
            "<doc schemaVersion=\"1\" bubble=\"Studio.Gameplay\">\n",
            "  <members>\n",
            "    <member id=\"function:Studio.Gameplay.alpha(Int)\"><summary>Returns <see cref=\"Studio.Value\"/>.</summary><param name=\"value\">Input.</param></member>\n",
            "    <member id=\"function:Studio.Gameplay.zeta()\"><summary>Later &amp; safe.</summary></member>\n",
            "  </members>\n",
            "</doc>\n",
        )
    );
}

#[test]
fn duplicate_member_ids_fail_closed() {
    let fragment = XmlFragment::parse("<summary>Value.</summary>").expect("safe fragment");
    let member = DocumentationMember::new("function:Studio.value()", fragment);
    assert_eq!(
        render_xml("Studio", &[member.clone(), member]),
        Err(DocumentationOutputError::DuplicateMemberId(
            "function:Studio.value()".to_owned()
        ))
    );
}
