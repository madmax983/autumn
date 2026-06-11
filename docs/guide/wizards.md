# Multi-Step Form Wizards

Wizards are session-backed multi-step forms where each step is validated
independently before the user advances. The final "commit" step assembles
all validated step data and performs the real write.

Autumn ships a `WizardContext` runtime struct plus a generator that
scaffolds the full route file. Start with the generator and edit the
generated TODOs into real field definitions.

## Quick start

```bash
autumn generate wizard checkout shipping payment review
```

This emits three files:

```
src/wizards/checkout.rs        # step structs + GET/POST handlers + confirm/commit/cancel
src/wizards/mod.rs             # pub mod checkout;  (created or appended)
tests/checkout_wizard.rs       # ignored integration test skeletons
```

Mount the routes and add `mod wizards;` in `src/main.rs`:

```rust
mod wizards;

// inside autumn_web::app() builder:
.routes(routes![
    wizards::checkout::show_shipping,
    wizards::checkout::submit_shipping,
    wizards::checkout::show_payment,
    wizards::checkout::submit_payment,
    wizards::checkout::show_review,
    wizards::checkout::submit_review,
    wizards::checkout::show_confirm,
    wizards::checkout::commit,
    wizards::checkout::cancel,
])
```

Then fill in the `// TODO` sections in `src/wizards/checkout.rs`.

## The generated file layout

```
WIZARD_NAME   = "checkout"
STEPS         = ["shipping", "payment", "review"]
wizard_context(session) -> WizardContext

ShippingForm  { /* fill in */ }
PaymentForm   { /* fill in */ }
ReviewForm    { /* fill in */ }

GET  /checkout/shipping  → show_shipping    (guard → form)
POST /checkout/shipping  → submit_shipping  (validate → save → redirect)
GET  /checkout/payment   → show_payment
POST /checkout/payment   → submit_payment
GET  /checkout/review    → show_review
POST /checkout/review    → submit_review
GET  /checkout/confirm   → show_confirm     (summary + CSRF-protected submit button)
POST /checkout/commit    → commit           (assemble all steps → write → clear)
POST /checkout/cancel    → cancel           (clear → redirect)
```

`commit` and `cancel` are POST-only. Commit mutates state; cancel discards
accumulated session data. Using GET for either would make them vulnerable to
CSRF or prefetching.

## `WizardContext`

`WizardContext` is the runtime handle for a wizard session. The generator
produces a `wizard_context(session: Session) -> WizardContext` helper so
you never construct it directly in handler code.

```rust
use autumn_web::wizard::{WizardContext, wizard_progress};

pub fn wizard_context(session: Session) -> WizardContext {
    WizardContext::new(session, WIZARD_NAME, STEPS.iter().map(|s| s.to_string()))
}
```

### `guard_step`

Ensures the user cannot jump to step N without completing steps 1..N-1.
If a prerequisite step is missing, `guard_step` returns an `Err(redirect_url)`
pointing at the first incomplete step.

```rust
if let Err(redirect_url) = wizard.guard_step("payment", "/checkout/shipping").await {
    return Redirect::to(&redirect_url).into_response();
}
```

The second argument is the fallback path used when the first step itself has
no prerequisites to check — typically the path to step 1.

### `save_step`

Serializes a step struct into the session under a namespaced key. The key
is `{wizard_name}/{step_name}` so multiple wizards in the same session cannot
collide.

```rust
wizard.save_step("shipping", &validated_data).await?;
```

`T` must implement `Serialize + Sync`.

### `step_data`

Deserializes a previously saved step. Returns `None` if the step hasn't been
saved yet or if deserialization fails.

```rust
let shipping: Option<ShippingForm> = wizard.step_data("shipping").await;
let data = shipping.unwrap_or_default(); // safe — generated structs derive Default
```

### `is_step_complete`

Returns `true` if the step has valid JSON saved in the session. Used
internally by `guard_step` and `first_incomplete_step`.

```rust
if wizard.is_step_complete("payment").await {
    // ...
}
```

### `first_incomplete_step`

Returns the name of the first step that is not yet complete, or `None` if
all steps are done. Used in `show_confirm` and `commit` to guard against
direct URL access.

```rust
if let Some(incomplete) = wizard.first_incomplete_step().await {
    return Redirect::to(&format!("/checkout/{incomplete}")).into_response();
}
```

### `clear`

Removes all step data for this wizard from the session. Call at the end of
`commit` (after the write succeeds) and in `cancel`.

```rust
wizard.clear().await;
```

### `wizard_progress`

Renders an accessible `<ol>` progress indicator that marks the active step.
Returns `Markup` so you can embed it directly in a Maud template.

```rust
html! {
    (wizard_progress(&wizard, "payment").await)
    // ... rest of the step form
}
```

Pass `"confirm"` as the step name on the confirm page.

## Step structs

Each step is a plain Rust struct that derives `Serialize`, `Deserialize`,
`Validate`, and `Default`. The generator emits empty structs with a `TODO`
comment; replace the comment with the fields that belong to that step.

```rust
#[derive(Debug, Default, Clone, Serialize, Deserialize, Validate)]
pub struct ShippingForm {
    #[validate(length(min = 1))]
    pub name: String,
    pub address_line_1: String,
    pub city: String,
    pub postcode: String,
}
```

Validation attributes from the `validator` crate (`url`, `email`,
`length(min=N, max=N)`, `range`, custom validators) are all supported.
`ChangesetForm<T>` calls `T::validate()` in `into_valid()` and collects
field-level errors automatically.

## CSRF

The generated GET handlers extract `Option<CsrfToken>` and
`Option<CsrfFormField>` so the wizard works with and without the CSRF
middleware enabled. When the middleware is active, both extractors resolve to
`Some`; when absent, they resolve to `None` and the form omits the hidden
field rather than panicking.

The generated `show_confirm` handler renders the hidden CSRF input manually
(not via `ChangesetForm`) because the confirm page has two forms — commit and
cancel — each needing the token.

## Session storage

Wizard state lives in the session store, namespaced by wizard name. If the
session expires, all step data is lost and the user must restart from step 1
(the guards redirect there automatically). This is intentional: partial
wizard data that outlives its session would be stale.

For wizards where step data is expensive to re-enter, consider increasing the
session TTL in `autumn.toml`:

```toml
[session]
ttl_seconds = 3600  # 1 hour
```

## Worked example

See [`examples/bookmarks/src/wizards/add_bookmark.rs`](../../examples/bookmarks/src/wizards/add_bookmark.rs)
for a fully filled-in wizard that saves a `Bookmark` through the repository.
The generator skeleton is visible in the commit that added `src/wizards/checkout.rs`;
the diff between skeleton and final is exactly the work left after generation.

## Validation and the 422 pattern

Submit handlers follow the same 422/re-render pattern as scaffold routes:

```rust
match form.into_valid() {
    Ok(data) => {
        wizard.save_step("shipping", &data).await?;
        Redirect::to("/checkout/payment").into_response()
    }
    Err(form) => (
        StatusCode::UNPROCESSABLE_ENTITY,
        html! {
            (wizard_progress(&wizard, "shipping").await)
            (form.form_tag("/checkout/shipping", "post", html! {
                // same fields as the GET handler
            }))
        },
    ).into_response(),
}
```

`ChangesetForm::into_valid()` re-packages the form with errors attached so
`form.text_input("name", "")` automatically renders error messages alongside
the relevant field.

## Name constraints

The wizard name and all step names must be valid Rust identifiers:

- Only ASCII letters, digits, and underscores.
- Must start with an ASCII letter or `_`.
- Must not be a Rust keyword (`type`, `mod`, `crate`, …).
- Step names `confirm`, `commit`, and `cancel` are reserved — the generator
  emits handlers with those names and collisions would produce a compile error.
- Duplicate step names (after snake\_case normalization) are rejected.

Use underscores for multi-word names: `add_bookmark`, `shipping_address`.
The generator converts PascalCase step names to snake\_case automatically
(`ShippingAddress` → `shipping_address`), but the CLI rejects hyphens.
