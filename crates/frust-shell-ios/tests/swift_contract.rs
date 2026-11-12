//! Source-scan contract for the iOS text-input view's accessibility surface, because no
//! cargo gate compiles Swift. `FrustView` is the `UITextInput` responder and keeps the
//! focused field's real text in `mirror`; it must never report that text to accessibility
//! (a password field would be read in clear). The accesskit nodes are the source of text.

use std::fs;
use std::path::Path;

const VIEW: &str = "platform/ios/FrustEmbedding/Sources/FrustEmbedding/FrustView.swift";
const INPUT: &str = "platform/ios/FrustEmbedding/Sources/FrustEmbedding/FrustTextInput.swift";

fn read(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// `src` without its comment lines.
fn code(src: &str) -> String {
    src.lines()
        .filter(|line| {
            let line = line.trim_start();
            !(line.starts_with("//") || line.starts_with("/*") || line.starts_with('*'))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The brace-matched body following the first occurrence of `signature` in `src`.
fn body<'a>(src: &'a str, signature: &str) -> &'a str {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` not found"));
    let open = start + src[start..].find('{').expect("opening brace");
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[open..open + i + 1];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces after `{signature}`");
}

#[test]
fn accessibility_value_override_returns_nil() {
    let view = code(&read(VIEW));
    let b = body(&view, "override var accessibilityValue: String?");
    let getter = body(b, "get");
    assert_eq!(
        getter.trim_matches(|c: char| c == '{' || c == '}').trim(),
        "nil"
    );
}

#[test]
fn view_claims_no_keyboard_key_trait() {
    let view = code(&read(VIEW));
    let b = body(
        &view,
        "override var accessibilityTraits: UIAccessibilityTraits",
    );
    let getter = body(b, "get");
    assert!(getter.contains(".subtracting(.keyboardKey)"), "{getter}");
    assert!(!view.contains("accessibilityTraits = .keyboardKey"));
    assert!(!view.contains("accessibilityTraits.insert"));
}

#[test]
fn mirror_never_reaches_an_accessibility_property() {
    for rel in [VIEW, INPUT] {
        for line in code(&read(rel)).lines() {
            let assigns_a11y = line.contains("accessibilityValue")
                || line.contains("accessibilityLabel")
                || line.contains("accessibilityHint")
                || line.contains("accessibilityAttributedValue");
            if assigns_a11y && line.contains('=') && !line.contains("override var") {
                assert!(
                    !line.contains("mirror") && !line.contains("text(in"),
                    "{rel}: accessibility property fed from the mirror: {line}"
                );
            }
        }
    }
}

#[test]
fn is_accessibility_element_is_not_overridden_in_swift() {
    for rel in [VIEW, INPUT] {
        let src = code(&read(rel));
        assert!(
            !src.contains("isAccessibilityElement"),
            "{rel}: the accesskit adapter owns `isAccessibilityElement`"
        );
    }
}
