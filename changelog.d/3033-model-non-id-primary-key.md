### Fixed

- **`#[model]`:** a model whose primary key is not `id` now compiles (issue
  #3033). The generated upsert/`on_conflict` paths, the list filter/sort
  arms, and the association preload loaders address the model's physical
  primary-key column — rename-aware through `#[diesel(column_name = …)]` —
  instead of a hard-coded `id`. Both spellings work: a renamed `#[id]` field
  (`#[id] pub post_uuid: i64` over `table! { posts (post_uuid) }`) and a
  `#[diesel(column_name = post_uuid)]` rename on an `id` field. `parent_pk =
  "<column>"` also stands alone on `#[belongs_to]` now (no `counter_cache`
  required), so the preload loader can address a non-`id` parent key
  directly. Expansion for `id`-keyed models is token-identical. Known
  follow-up: `#[repository]`'s `delete_by_id`/batch loaders still address
  `<table>::id`; a model with a non-`id` key and a repository needs that
  macro to learn the key next.
