// The comment says:
// Calling `Session::rotate_id()` more than once in the same request overwrites `old_id` each time,
// but `SessionService` only destroys the single stored `old_id` at response time.
// Preserve the initial pre-rotation ID (or track all prior IDs) so every superseded ID is invalidated.
// We can change `old_id: Option<String>` to `old_ids: Vec<String>` or simply keep the VERY FIRST `old_id` if it's already set!
// If it's already set, it means we rotated multiple times, but the ONLY ID that exists in the *store* from before this request is the first `old_id`.
// Actually, intermediate generated IDs in the same request were never saved to the store!
// So destroying the VERY FIRST ID is sufficient!
// `if inner.old_id.is_none() { inner.old_id = Some(inner.id.clone()); }`
