use crate::FeatureFlagStore;
use crate::routes::FeatureFlagUpdate;
use autumn_web::AppState;
use autumn_web::prelude::*;
use axum::extract::{Form, Path, State};
use std::sync::Arc;

#[derive(serde::Deserialize)]
pub struct CreateFlagForm {
    pub name: String,
    pub enabled: Option<String>,
}

#[get("/admin/feature-flags")]
#[allow(clippy::missing_errors_doc)]
pub async fn admin_index(State(state): State<AppState>) -> AutumnResult<Markup> {
    let store = state
        .extension::<Arc<dyn FeatureFlagStore>>()
        .ok_or_else(|| {
            AutumnError::internal_server_error_msg("FeatureFlagStore not found in AppState")
        })?;
    let flags = store.get_all();

    Ok(html! {
        div class="p-6 max-w-4xl mx-auto" {
            h1 class="text-2xl font-bold mb-6" { "Feature Flags" }

            form hx-post="/admin/feature-flags" hx-target="#flag-list" class="mb-8 flex gap-4 items-end bg-white p-4 rounded-lg shadow-sm border border-gray-200" {
                div class="flex-1" {
                    label class="block text-sm font-medium text-gray-700 mb-1" { "Flag Name" }
                    input type="text" name="name" required class="w-full rounded-md border-gray-300 shadow-sm focus:border-primary focus:ring focus:ring-primary focus:ring-opacity-50" placeholder="e.g. new_dashboard" {}
                }
                div class="flex items-center gap-2 mb-2" {
                    input type="checkbox" name="enabled" id="enabled-new" value="true" class="rounded text-primary focus:ring-primary" {}
                    label for="enabled-new" class="text-sm text-gray-700" { "Enabled" }
                }
                button type="submit" class="bg-primary hover:bg-primary-hover text-white px-4 py-2 rounded-md font-medium transition-colors" { "Add Flag" }
            }

            div id="flag-list" {
                (render_flag_list(&flags))
            }
        }
    })
}

#[post("/admin/feature-flags")]
#[allow(clippy::missing_errors_doc)]
pub async fn create_flag(
    State(state): State<AppState>,
    Form(form): Form<CreateFlagForm>,
) -> AutumnResult<Markup> {
    let store = state
        .extension::<Arc<dyn FeatureFlagStore>>()
        .ok_or_else(|| {
            AutumnError::internal_server_error_msg("FeatureFlagStore not found in AppState")
        })?;

    let enabled = form.enabled.is_some();
    store.set(&form.name, enabled);

    // Broadcast
    broadcast_update(&state, &form.name, enabled);

    let flags = store.get_all();
    Ok(render_flag_list(&flags))
}

#[post("/admin/feature-flags/{name}/toggle")]
#[allow(clippy::missing_errors_doc)]
pub async fn toggle_flag(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AutumnResult<Markup> {
    let store = state
        .extension::<Arc<dyn FeatureFlagStore>>()
        .ok_or_else(|| {
            AutumnError::internal_server_error_msg("FeatureFlagStore not found in AppState")
        })?;

    let current = store.get(&name).unwrap_or(false);
    let new_val = !current;
    store.set(&name, new_val);

    // Broadcast
    broadcast_update(&state, &name, new_val);

    Ok(render_flag_row(&name, new_val))
}

fn broadcast_update(state: &AppState, name: &str, enabled: bool) {
    let update = FeatureFlagUpdate {
        name: name.to_string(),
        enabled,
    };
    if let Ok(json) = serde_json::to_string(&update) {
        let _ = state.channels().publish("feature-flags", json);
    }
}

fn render_flag_list(flags: &std::collections::HashMap<String, bool>) -> Markup {
    let mut sorted_flags: Vec<_> = flags.iter().collect();
    sorted_flags.sort_by_key(|(k, _)| *k);

    html! {
        div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden" {
            ul class="divide-y divide-gray-200" {
                @if sorted_flags.is_empty() {
                    li class="p-6 text-center text-gray-500" { "No feature flags configured." }
                }
                @for (name, enabled) in sorted_flags {
                    (render_flag_row(name, *enabled))
                }
            }
        }
    }
}

fn render_flag_row(name: &str, enabled: bool) -> Markup {
    let target_id = format!(
        "flag-row-{}",
        name.replace(|c: char| !c.is_alphanumeric(), "-")
    );
    html! {
        li id=(target_id) class="p-4 flex items-center justify-between hover:bg-gray-50 transition-colors" {
            div class="flex items-center gap-3" {
                div class=(format!("w-3 h-3 rounded-full {}", if enabled { "bg-success" } else { "bg-gray-300" })) {}
                span class="font-medium text-gray-900" { (name) }
            }
            button
                hx-post=(format!("/admin/feature-flags/{}/toggle", name))
                hx-target=(format!("#{}", target_id))
                hx-swap="outerHTML"
                class=(format!("px-3 py-1.5 rounded-full text-sm font-medium transition-colors border {}",
                    if enabled { "border-danger text-danger hover:bg-danger-light" }
                    else { "border-success text-success hover:bg-success-light" }
                ))
            {
                @if enabled { "Disable" } @else { "Enable" }
            }
        }
    }
}

#[must_use]
pub fn routes() -> Vec<autumn_web::Route> {
    routes![admin_index, create_flag, toggle_flag]
}
