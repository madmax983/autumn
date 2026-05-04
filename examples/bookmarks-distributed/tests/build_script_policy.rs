#[path = "../build_support.rs"]
mod build_support;

use build_support::{TailwindFailureAction, tailwind_failure_action};

#[test]
fn tailwind_failure_uses_checked_in_css_when_available() {
    assert_eq!(
        tailwind_failure_action(true, false),
        TailwindFailureAction::UseCheckedInCss
    );
}

#[test]
fn tailwind_failure_fails_when_css_is_missing() {
    assert_eq!(
        tailwind_failure_action(false, false),
        TailwindFailureAction::FailBuild
    );
}

#[test]
fn tailwind_failure_fails_when_tailwind_is_required() {
    assert_eq!(
        tailwind_failure_action(true, true),
        TailwindFailureAction::FailBuild
    );
}
