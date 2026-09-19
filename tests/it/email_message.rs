use triptych::email::message::*;

#[test]
fn strips_preheader_padding() {
    let padded = "PNC Financial Services Group is hiring\u{A0}\u{200C}\u{200D}\u{200E}\u{200F}\u{FEFF}\u{A0}\u{200C}\u{200D}\u{200E}\u{200F}\u{FEFF}";
    assert_eq!(
        clean_snippet(padded),
        "PNC Financial Services Group is hiring"
    );
}
