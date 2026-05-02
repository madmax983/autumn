use autumn_web::prelude::*;

#[tokio::test]
async fn test_htmx_oob_swap_injection_blocked_by_maud() {
    let attacker_input = r#"<div hx-swap-oob="true" id="admin-panel">Hacked</div>"#;
    let template = html! {
        div { (attacker_input) }
    };

    let rendered = template.into_string();

    // The `<` and `"` characters should be escaped by Maud
    assert!(!rendered.contains("<div hx-swap-oob"));
    assert!(rendered.contains("&lt;div hx-swap-oob=&quot;true&quot;"));
}
