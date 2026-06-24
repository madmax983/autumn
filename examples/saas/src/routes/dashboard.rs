//! The tenant-scoped dashboard.
//!
//! The session carries `tenant_id` (set at signup/login). We read it back and
//! run the repository calls inside `with_tenant`, which establishes the tenant
//! context the `tenant_scoped` `PgProjectRepository` filters by — so this page
//! only ever shows the signed-in organisation's projects.

use autumn_web::prelude::*;
use autumn_web::reexports::axum::response::Response;

use crate::models::{NewProject, NewProjectForm};
use crate::repositories::{PgProjectRepository, ProjectRepository};

use super::layout::layout;

#[get("/dashboard")]
pub async fn dashboard(session: Session, repo: PgProjectRepository) -> AutumnResult<Response> {
    let Some(tenant_id) = session.get("tenant_id").await else {
        return Ok(Redirect::to("/login").into_response());
    };

    let projects = with_tenant(tenant_id.clone(), async move { repo.find_all().await }).await?;

    let page = layout(
        "Dashboard",
        true,
        html! {
            div class="flex items-center justify-between mb-6" {
                h1 class="text-2xl font-bold" { "Projects" }
                span class="text-sm text-gray-500" { "tenant: " code { (tenant_id) } }
            }

            form action="/dashboard/projects" method="post"
                 class="flex gap-2 mb-6 bg-white rounded-lg shadow p-4" {
                input name="name" required placeholder="New project name"
                      class="flex-1 border rounded px-3 py-2";
                button type="submit"
                       class="px-4 py-2 bg-indigo-600 text-white rounded hover:bg-indigo-700" {
                    "Create"
                }
            }

            ul class="space-y-2" {
                @for project in &projects {
                    li class="bg-white rounded-lg shadow p-4 flex items-center justify-between" {
                        span class="font-medium" { (project.name) }
                        span class="text-xs text-gray-400" { (project.created_at.format("%Y-%m-%d %H:%M").to_string()) }
                    }
                }
                @if projects.is_empty() {
                    li class="text-gray-400 text-center py-8" { "No projects yet — create your first above." }
                }
            }
        },
    );
    Ok(page.into_response())
}

#[post("/dashboard/projects")]
pub async fn create_project(
    session: Session,
    repo: PgProjectRepository,
    Form(form): Form<NewProjectForm>,
) -> AutumnResult<Response> {
    let Some(tenant_id) = session.get("tenant_id").await else {
        return Ok(Redirect::to("/login").into_response());
    };

    let name = form.name.trim().to_owned();
    if name.is_empty() {
        return Err(AutumnError::unprocessable_msg("Project name is required"));
    }

    // `tenant_id` is stamped by the tenant_scoped repository from the context
    // below, so it is not part of `NewProject`.
    with_tenant(
        tenant_id,
        async move { repo.save(&NewProject { name }).await },
    )
    .await?;
    Ok(Redirect::to("/dashboard").into_response())
}
