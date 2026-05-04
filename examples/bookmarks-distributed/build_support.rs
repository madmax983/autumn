#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailwindFailureAction {
    FailBuild,
    UseCheckedInCss,
}

pub fn tailwind_failure_action(
    checked_in_css_exists: bool,
    require_tailwind: bool,
) -> TailwindFailureAction {
    if require_tailwind || !checked_in_css_exists {
        TailwindFailureAction::FailBuild
    } else {
        TailwindFailureAction::UseCheckedInCss
    }
}

#[allow(dead_code)]
pub fn require_tailwind_from_env() -> bool {
    std::env::var("AUTUMN_REQUIRE_TAILWIND").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}
