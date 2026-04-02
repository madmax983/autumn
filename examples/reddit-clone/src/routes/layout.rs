//! Shared layout and UI components used across all routes.

use autumn_web::{Markup, PreEscaped, html};

/// Render a meta-refresh redirect page.
pub fn redirect_to(url: &str) -> Markup {
    html! {
        (PreEscaped("<!DOCTYPE html>"))
        html {
            head { meta http-equiv="refresh" content=(format!("0;url={url}")); }
            body { p { "Redirecting to " a href=(url) { (url) } "..." } }
        }
    }
}

/// Base HTML layout wrapping page content.
///
/// Accepts an optional `username` to show login/logout state in the nav.
pub fn layout(title: &str, username: Option<&str>, content: Markup) -> Markup {
    html! {
        (PreEscaped("<!DOCTYPE html>"))
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " — Autumn Reddit" }
                link rel="stylesheet" href="/static/css/autumn.css";
                script src="/static/js/htmx.min.js" {}
            }
            body class="bg-gray-100 min-h-screen text-gray-900" {
                // Navigation bar
                nav class="bg-white border-b border-gray-200 shadow-sm sticky top-0 z-10" {
                    div class="max-w-5xl mx-auto px-4 py-3 flex items-center justify-between" {
                        div class="flex items-center gap-6" {
                            a href="/" class="text-xl font-bold text-orange-600 hover:text-orange-700" {
                                "autumn/reddit"
                            }
                            div class="hidden sm:flex items-center gap-4 text-sm" {
                                a href="/r" class="text-gray-600 hover:text-orange-600" { "Communities" }
                                a href="/about" class="text-gray-600 hover:text-orange-600" { "About" }
                                a href="/actuator/health" class="text-gray-500 hover:text-orange-600" { "Health" }
                            }
                        }
                        div class="flex items-center gap-3 text-sm" {
                            @if let Some(name) = username {
                                span class="text-gray-600" { "u/" (name) }
                                a href="/submit"
                                  class="px-3 py-1.5 bg-orange-500 text-white rounded hover:bg-orange-600" {
                                    "New Post"
                                }
                                form action="/logout" method="post" class="inline" {
                                    button type="submit"
                                           class="text-gray-500 hover:text-orange-600 cursor-pointer" {
                                        "Log out"
                                    }
                                }
                            } @else {
                                a href="/login" class="text-gray-600 hover:text-orange-600" { "Log in" }
                                a href="/register"
                                  class="px-3 py-1.5 bg-orange-500 text-white rounded hover:bg-orange-600" {
                                    "Sign up"
                                }
                            }
                        }
                    }
                }

                // Main content
                main class="max-w-5xl mx-auto py-6 px-4" {
                    (content)
                }

                // Footer
                footer class="border-t border-gray-200 mt-12" {
                    div class="max-w-5xl mx-auto text-center text-xs text-gray-400 py-6" {
                        "Built with "
                        a href="https://github.com/madmax983/autumn"
                          class="text-orange-600 hover:underline" { "Autumn" }
                        " — Rust + Diesel + Maud + htmx + Tailwind"
                    }
                }
            }
        }
    }
}

/// Score display with upvote/downvote buttons (htmx-powered).
pub fn vote_controls(post_id: i64, score: i64) -> Markup {
    html! {
        div id=(format!("votes-{post_id}"))
            class="flex flex-col items-center gap-0.5 text-sm select-none" {
            button
                hx-post=(format!("/posts/{post_id}/upvote"))
                hx-target=(format!("#votes-{post_id}"))
                hx-swap="outerHTML"
                class="text-gray-400 hover:text-orange-500 cursor-pointer text-lg leading-none" {
                "\u{25B2}"
            }
            span class="font-semibold text-gray-700" { (score) }
            button
                hx-post=(format!("/posts/{post_id}/downvote"))
                hx-target=(format!("#votes-{post_id}"))
                hx-swap="outerHTML"
                class="text-gray-400 hover:text-blue-500 cursor-pointer text-lg leading-none" {
                "\u{25BC}"
            }
        }
    }
}

/// Timestamp display helper.
pub fn time_ago(dt: &chrono::NaiveDateTime) -> String {
    let now = chrono::Utc::now().naive_utc();
    let diff = now - *dt;

    if diff.num_days() > 365 {
        format!("{}y ago", diff.num_days() / 365)
    } else if diff.num_days() > 30 {
        format!("{}mo ago", diff.num_days() / 30)
    } else if diff.num_days() > 0 {
        format!("{}d ago", diff.num_days())
    } else if diff.num_hours() > 0 {
        format!("{}h ago", diff.num_hours())
    } else if diff.num_minutes() > 0 {
        format!("{}m ago", diff.num_minutes())
    } else {
        "just now".to_string()
    }
}
