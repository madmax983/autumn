use autumn_web::extract::Path;
use autumn_web::prelude::*;

use crate::models::Bookmark;

fn layout(title: &str, content: Markup) -> Markup {
    html! {
        (PreEscaped("<!DOCTYPE html>"))
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " — Bookmarks" }
                link rel="stylesheet" href="/static/css/autumn.css";
                script src="/static/js/htmx.min.js" {}
            }
            body class="bg-gray-50 min-h-screen" {
                nav class="bg-indigo-600 text-white p-4" {
                    div class="max-w-3xl mx-auto flex justify-between items-center" {
                        a href="/" class="text-xl font-bold" { "Bookmarks" }
                        div class="space-x-4 text-sm" {
                            a href="/actuator/health" class="opacity-75 hover:opacity-100" { "Health" }
                            a href="/actuator/info" class="opacity-75 hover:opacity-100" { "Info" }
                        }
                    }
                }
                main class="max-w-3xl mx-auto p-6" { (content) }
            }
        }
    }
}

fn bookmark_card(b: &Bookmark) -> Markup {
    html! {
        li id=(format!("bookmark-{}", b.id))
           class="p-4 bg-white rounded shadow flex justify-between items-center" {
            div {
                a href=(b.url) target="_blank"
                  class="text-indigo-600 font-medium hover:underline" {
                    (b.title)
                }
                span class="ml-2 text-xs bg-gray-200 rounded px-2 py-0.5" { (b.tag) }
                @if !b.alive {
                    span class="ml-2 text-xs bg-red-100 text-red-600 rounded px-2 py-0.5" {
                        "dead link"
                    }
                }
            }
            button
                hx-delete=(format!("/api/bookmarks/{}", b.id))
                hx-target=(format!("#bookmark-{}", b.id))
                hx-swap="outerHTML"
                hx-confirm="Delete this bookmark?"
                class="text-red-500 text-sm hover:text-red-700" {
                "Delete"
            }
        }
    }
}

#[get("/")]
pub async fn list(mut db: Db) -> AutumnResult<Markup> {
    let all = Bookmark::all(&mut db).await?;
    Ok(layout(
        "All",
        html! {
            div class="flex justify-between items-center mb-6" {
                h1 class="text-2xl font-bold" { "All Bookmarks" }
                a href="/new"
                  class="bg-indigo-600 text-white px-4 py-2 rounded hover:bg-indigo-700" {
                    "+ Add"
                }
            }
            ul class="space-y-3" {
                @for b in &all {
                    (bookmark_card(b))
                }
                @if all.is_empty() {
                    li class="text-gray-400 text-center py-8" { "No bookmarks yet." }
                }
            }
        },
    ))
}

#[get("/tag/{tag}")]
pub async fn by_tag(Path(tag): Path<String>, mut db: Db) -> AutumnResult<Markup> {
    let tagged = Bookmark::find_by_tag(&tag, &mut db).await?;
    Ok(layout(
        &format!("#{tag}"),
        html! {
            h1 class="text-2xl font-bold mb-6" { "Tag: " (tag) }
            ul class="space-y-3" {
                @for b in &tagged { (bookmark_card(b)) }
            }
        },
    ))
}

#[get("/new")]
pub async fn new_form() -> Markup {
    layout(
        "Add Bookmark",
        html! {
            h1 class="text-2xl font-bold mb-6" { "Add Bookmark" }
            form action="/api/bookmarks" method="post" class="space-y-4" {
                div {
                    label for="url" class="block text-sm font-medium" { "URL" }
                    input type="url" id="url" name="url" required
                          placeholder="https://example.com"
                          class="w-full border rounded p-2 mt-1";
                }
                div {
                    label for="title" class="block text-sm font-medium" { "Title" }
                    input type="text" id="title" name="title" required
                          placeholder="My favorite site"
                          class="w-full border rounded p-2 mt-1";
                }
                div {
                    label for="tag" class="block text-sm font-medium" { "Tag" }
                    input type="text" id="tag" name="tag" value="general"
                          class="w-full border rounded p-2 mt-1";
                }
                button type="submit"
                       class="bg-indigo-600 text-white px-6 py-2 rounded hover:bg-indigo-700" {
                    "Save"
                }
            }
        },
    )
}
