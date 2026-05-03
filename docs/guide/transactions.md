# Transactions

Use `Db::tx` when a handler must perform **multiple writes atomically**.

If every write in the closure succeeds, the transaction commits. If any step
returns `Err`, the transaction rolls back.

```rust,no_run
use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use scoped_futures::ScopedFutureExt;

async fn create_two_rows(mut db: Db) -> AutumnResult<i64> {
    let id = db
        .tx(|conn| {
            async move {
                let id: i64 = diesel::insert_into(crate::schema::posts::table)
                    .values(crate::schema::posts::title.eq("hello"))
                    .returning(crate::schema::posts::id)
                    .get_result(conn)
                    .await?;

                diesel::insert_into(crate::schema::votes::table)
                    .values((
                        crate::schema::votes::post_id.eq(id),
                        crate::schema::votes::user_id.eq(1_i64),
                        crate::schema::votes::value.eq(1_i16),
                    ))
                    .execute(conn)
                    .await?;

                Ok::<_, AutumnError>(id)
            }
            .scope_boxed()
        })
        .await?;

    Ok(id)
}
```

## `db.tx` vs hooks

- Use repository hooks (`before_create`, `before_update`, `before_delete`) for
  model-local mutation concerns.
- Use `db.tx` when orchestration spans multiple writes and/or multiple tables in
  one route or service operation.

Hooks executed inside `db.tx` participate in the same database transaction.

## Panic and rollback

`Db::tx` delegates to Diesel async transaction handling. Operationally:

- `Ok(_)` commits
- `Err(_)` rolls back
- panics unwind through the transaction boundary and do not commit partial work

## Nesting policy

Nested `Db::tx` calls are currently **rejected at runtime** with:

`Nested Db::tx calls are not supported`

This avoids ambiguity and keeps transaction boundaries explicit.
