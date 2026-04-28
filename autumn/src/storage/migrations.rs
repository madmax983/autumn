//! Helpers for migrations that involve [`Blob`](super::Blob) columns.
//!
//! A `Blob` lives on a Postgres `JSONB` column on a `#[model]`-derived
//! struct. Adding one to an existing table is a one-line `ALTER TABLE`,
//! but typing that one line by hand on every blob field invites typos
//! (wrong column type, missing `NULL`, no symmetric `down.sql`). The
//! [`add_blob_column!`] macro takes the table + column and returns a
//! `(up, down)` SQL pair you can paste into a Diesel migration or run
//! through any connection.
//!
//! ## Examples
//!
//! Generate the SQL pair for a Diesel `migrations/<name>/{up,down}.sql`:
//!
//! ```
//! use autumn_web::storage::migrations::add_blob_column;
//!
//! let (up, down) = add_blob_column!("users", "avatar");
//! assert_eq!(up, "ALTER TABLE users ADD COLUMN avatar JSONB NULL");
//! assert_eq!(down, "ALTER TABLE users DROP COLUMN avatar");
//! ```
//!
//! Run it through a runtime Diesel-async connection:
//!
//! ```rust,ignore
//! use autumn_web::storage::migrations::add_blob_column;
//! use diesel_async::RunQueryDsl;
//!
//! let (up, _down) = add_blob_column!("users", "avatar");
//! diesel::sql_query(up).execute(&mut conn).await?;
//! ```

/// Build the `(up, down)` SQL pair to add or drop a [`Blob`](super::Blob)
/// column on an existing Postgres table.
///
/// `up` produces:
///
/// ```sql
/// ALTER TABLE <table> ADD COLUMN <column> JSONB NULL
/// ```
///
/// `down` produces:
///
/// ```sql
/// ALTER TABLE <table> DROP COLUMN <column>
/// ```
///
/// Both arguments must be string literals — keys in `Diesel`/Postgres
/// identifiers can't be safely interpolated from runtime values without
/// quoting, so the macro deliberately requires compile-time strings to
/// avoid an injection footgun. For runtime-named columns, build the
/// SQL by hand.
#[macro_export]
macro_rules! add_blob_column {
    ($table:literal, $column:literal) => {{
        const UP: &str = concat!(
            "ALTER TABLE ",
            $table,
            " ADD COLUMN ",
            $column,
            " JSONB NULL"
        );
        const DOWN: &str = concat!("ALTER TABLE ", $table, " DROP COLUMN ", $column);
        (UP, DOWN)
    }};
}

#[doc(inline)]
pub use add_blob_column;

#[cfg(test)]
mod tests {
    #[test]
    fn add_blob_column_emits_postgres_jsonb() {
        let (up, down) = add_blob_column!("users", "avatar");
        assert_eq!(up, "ALTER TABLE users ADD COLUMN avatar JSONB NULL");
        assert_eq!(down, "ALTER TABLE users DROP COLUMN avatar");
    }

    #[test]
    fn add_blob_column_handles_underscored_names() {
        let (up, _down) = add_blob_column!("blog_posts", "cover_image");
        assert_eq!(
            up,
            "ALTER TABLE blog_posts ADD COLUMN cover_image JSONB NULL"
        );
    }
}
