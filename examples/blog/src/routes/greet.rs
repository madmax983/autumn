//! Demonstrates the autumn-web i18n module end-to-end.
//!
//! Loads its strings from `i18n/{en,es}.ftl` via the bundle registered in
//! `main.rs` with `.i18n_auto()`. Browse to `/greet`, then click the
//! language switcher (or append `?locale=es`) to see the same content in
//! Spanish — `Locale` resolves the request locale, `t!()` performs the
//! lookup, and the bundle handles fallback and missing-key warnings.

use autumn_web::prelude::*;

use super::posts::layout;

#[get("/greet")]
pub async fn greet(locale: Locale) -> Markup {
    let title = t!(locale, "greet.title");
    let body = html! {
        article class="space-y-6" {
            header class="border-b border-stone-200 pb-4" {
                h1 class="text-3xl font-bold tracking-tight text-stone-900" {
                    (title)
                }
            }

            p class="text-stone-700 leading-relaxed" {
                (t!(locale, "greet.greeting", name = "Ada"))
            }

            // Locale switcher — preserves the path, just swaps `?locale=`.
            // Each link sets the cookie via the next request hitting the
            // same handler with `?locale=xx`, so the choice persists across
            // navigations.
            div class="bg-amber-50 border border-amber-200 rounded-lg p-4 space-y-2" {
                p class="text-sm text-amber-900 font-medium" {
                    (t!(locale, "nav.locale.label")) ":"
                }
                p class="text-sm text-amber-900" {
                    a href="/greet?locale=en" class="underline mr-3" {
                        (t!(locale, "nav.locale.en"))
                    }
                    a href="/greet?locale=es" class="underline" {
                        (t!(locale, "nav.locale.es"))
                    }
                }
                p class="text-xs text-amber-800 mt-2" {
                    (t!(locale, "greet.switcher_help"))
                }
                p class="text-xs text-amber-800" {
                    (t!(locale, "greet.try"))
                }
            }

            p class="text-xs text-stone-500" {
                "Locale: " code class="px-1 bg-stone-100 rounded font-mono" { (locale.tag()) }
            }
        }
    };
    layout(&title, body)
}
