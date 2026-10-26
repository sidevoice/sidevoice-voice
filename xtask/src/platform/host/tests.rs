use super::description;

#[test]
fn names_the_architecture_and_has_no_line_break() {
    let text = description();
    assert!(text.contains(std::env::consts::ARCH), "{text}");
    assert!(!text.contains('\n'), "{text}");
}
