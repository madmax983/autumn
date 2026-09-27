use autumn_web::error::{AutumnError, AutumnResult};
use autumn_web::pagination::{Page, PageRequest};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::schema::todos;

/// A todo item loaded from the database.
#[derive(Queryable, Selectable, Serialize)]
#[diesel(table_name = todos)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Todo {
    pub id: i64,
    pub title: String,
    pub completed: bool,
    pub created_at: chrono::NaiveDateTime,
}

impl Todo {
    /// Load all todos ordered by creation date (newest first).
    pub async fn all(db: &mut AsyncPgConnection) -> AutumnResult<Vec<Self>> {
        Ok(todos::table
            .order(todos::created_at.desc())
            .select(Self::as_select())
            .load(db)
            .await?)
    }

    /// Load a page of todos ordered by creation date (newest first).
    ///
    /// Accepts a [`PageRequest`] and returns a [`Page`] containing the items
    /// together with total-elements / total-pages metadata.
    pub async fn page(req: &PageRequest, db: &mut AsyncPgConnection) -> AutumnResult<Page<Self>> {
        let total: i64 = todos::table.count().get_result(db).await?;
        let items = todos::table
            .order((todos::created_at.desc(), todos::id.desc()))
            .limit(req.limit())
            .offset(req.offset())
            .select(Self::as_select())
            .load(db)
            .await?;
        Ok(Page::new(items, total, req))
    }

    /// Find a single todo by ID, returning 404 if not found.
    pub async fn find(id: i64, db: &mut AsyncPgConnection) -> AutumnResult<Self> {
        todos::table
            .find(id)
            .select(Self::as_select())
            .first(db)
            .await
            .map_err(AutumnError::not_found)
    }
}

impl autumn_web::data::csv::CsvSchema for Todo {
    fn csv_columns() -> &'static [&'static str] {
        &["id", "title", "completed", "created_at"]
    }

    fn to_csv_record(&self) -> Vec<String> {
        vec![
            self.id.to_string(),
            self.title.clone(),
            self.completed.to_string(),
            self.created_at.to_string(),
        ]
    }
}

/// Data needed to insert a new todo.
///
/// Derives [`Validate`] so it can be used directly with
/// [`ChangesetForm<NewTodo>`](autumn_web::form::ChangesetForm) when
/// the form shape matches the model shape — no separate form struct needed.
///
/// When the form requires extra fields, different validation rules, or
/// UI-specific concerns (e.g. a `confirm_password` field), define a
/// dedicated form struct instead and convert it to `NewTodo` on success.
#[derive(Insertable, Deserialize, Serialize, Validate, Debug)]
#[diesel(table_name = todos)]
pub struct NewTodo {
    #[validate(
        length(min = 1, max = 255, message = "Title must be 1–255 characters"),
        custom(function = "title_not_blank")
    )]
    pub title: String,
}

/// Validate that a title has at least one non-whitespace character.
pub(crate) fn title_not_blank(s: &str) -> Result<(), validator::ValidationError> {
    if s.trim().is_empty() {
        let mut e = validator::ValidationError::new("blank");
        e.message = Some("Title must not be blank or whitespace-only".into());
        return Err(e);
    }
    Ok(())
}

impl NewTodo {
    /// Validate and normalize the title.
    ///
    /// Runs the model's own derived [`validator`] rules first — the same
    /// rules the HTML form path enforces through `ChangesetForm<NewTodo>` —
    /// then trims the title. Returns 422 on any violation.
    pub fn validated(self) -> AutumnResult<Self> {
        // The derived rules (length, custom) run against the submitted
        // value, mirroring `ChangesetForm::into_valid`, which validates
        // before the handler trims for storage. Without this the JSON API
        // silently accepted titles past `length(max = 255)` (#2972).
        if let Err(errors) = self.validate() {
            let details = errors
                .field_errors()
                .into_iter()
                .map(|(field, errs)| {
                    let messages = errs
                        .iter()
                        .map(|e| {
                            e.message.as_ref().map_or_else(
                                || format!("validation failed: {}", e.code),
                                ToString::to_string,
                            )
                        })
                        .collect();
                    (field.to_string(), messages)
                })
                .collect();
            return Err(AutumnError::validation(details));
        }
        let title = self.title.trim().to_owned();
        if title.is_empty() {
            return Err(AutumnError::unprocessable_msg("Title must not be empty"));
        }
        Ok(Self { title })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use autumn_web::prelude::StatusCode;

    #[test]
    fn validated_accepts_a_255_char_title() {
        let todo = NewTodo {
            title: "x".repeat(255),
        }
        .validated()
        .unwrap();
        assert_eq!(todo.title.len(), 255);
    }

    #[test]
    fn validated_rejects_a_256_char_title_with_422() {
        // Issue #2972: the JSON API silently accepted titles past the
        // model's declared `length(max = 255)` because the hand-rolled
        // `validated()` never ran the derived rules.
        let err = NewTodo {
            title: "x".repeat(256),
        }
        .validated()
        .unwrap_err();
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn validated_rejects_an_empty_title_with_422() {
        let err = NewTodo {
            title: String::new(),
        }
        .validated()
        .unwrap_err();
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn validated_rejects_a_whitespace_only_title_with_422() {
        let err = NewTodo {
            title: "   ".to_owned(),
        }
        .validated()
        .unwrap_err();
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn validated_trims_a_valid_title() {
        let todo = NewTodo {
            title: "  Buy milk  ".to_owned(),
        }
        .validated()
        .unwrap();
        assert_eq!(todo.title, "Buy milk");
    }
}
