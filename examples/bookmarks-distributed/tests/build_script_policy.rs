#[path = "../build_support.rs"]
mod build_support;

use build_support::{TailwindFailureAction, tailwind_failure_action};

#[test]
fn tailwind_failure_is_optional_when_css_exists() {
    assert_eq!(
        tailwind_failure_action(false),
        TailwindFailureAction::SkipRegeneration
    );
}

#[test]
fn tailwind_failure_is_optional_when_css_is_missing() {
    assert_eq!(
        tailwind_failure_action(false),
        TailwindFailureAction::SkipRegeneration
    );
}

#[test]
fn tailwind_failure_fails_when_tailwind_is_required() {
    assert_eq!(
        tailwind_failure_action(true),
        TailwindFailureAction::FailBuild
    );
}
