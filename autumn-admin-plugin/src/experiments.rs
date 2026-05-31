//! Admin panel model for `autumn_experiments`.
//!
//! Registers an experiment management page at `/admin/experiments/` with:
//! - List view: name, state, variants, winner
//! - Edit view: update description, exclusion group, state, variants
//! - History tab: per-experiment audit trail from `autumn_experiment_changes`

use serde_json::Value;

use crate::{
    AdminError, AdminField, AdminFieldKind, AdminFuture, AdminHistoryPage, AdminModel, ListParams,
    ListResult, SelectOption,
};

/// Admin panel model for A/B experiments.
///
/// Register this model with the admin plugin to get an experiment management UI
/// at `/admin/experiments/`:
///
/// ```rust,ignore
/// use autumn_admin_plugin::{prelude::*, AdminPlugin};
/// use autumn_admin_plugin::experiments::ExperimentAdminModel;
///
/// autumn_web::app()
///     .plugin(
///         AdminPlugin::new()
///             .register(ExperimentAdminModel::default()),
///     )
///     .run()
///     .await;
/// ```
#[derive(Debug, Default, Clone)]
pub struct ExperimentAdminModel;

impl AdminModel for ExperimentAdminModel {
    fn slug(&self) -> &'static str {
        "experiments"
    }

    fn display_name(&self) -> &'static str {
        "Experiment"
    }

    fn display_name_plural(&self) -> &'static str {
        "Experiments"
    }

    fn record_display(&self, record: &Value) -> String {
        record
            .get("name")
            .and_then(|v| v.as_str())
            .map_or_else(|| "Experiment".to_owned(), |n| format!("Experiment: {n}"))
    }

    fn fields(&self) -> Vec<AdminField> {
        vec![
            AdminField::new("name", AdminFieldKind::Text)
                .label("Experiment Name")
                .searchable(),
            AdminField::new("description", AdminFieldKind::TextArea)
                .label("Description")
                .optional()
                .searchable(),
            AdminField::new(
                "state",
                AdminFieldKind::Select(vec![
                    SelectOption {
                        value: "draft".into(),
                        label: "Draft".into(),
                    },
                    SelectOption {
                        value: "running".into(),
                        label: "Running".into(),
                    },
                    SelectOption {
                        value: "concluded".into(),
                        label: "Concluded".into(),
                    },
                    SelectOption {
                        value: "archived".into(),
                        label: "Archived".into(),
                    },
                ]),
            )
            .label("State"),
            AdminField::new("variants", AdminFieldKind::Json)
                .label("Variants (JSON)")
                .optional()
                .hide_from_list(),
            AdminField::new("winner", AdminFieldKind::Text)
                .label("Winner")
                .optional(),
            AdminField::new("exclusion_group", AdminFieldKind::Text)
                .label("Exclusion Group")
                .optional()
                .hide_from_list(),
            AdminField::new("updated_at", AdminFieldKind::DateTime)
                .label("Last Updated")
                .readonly()
                .optional(),
        ]
    }

    fn has_history(&self) -> bool {
        true
    }

    fn list(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        params: ListParams,
    ) -> AdminFuture<'_, ListResult> {
        use diesel_async::RunQueryDsl;

        let pool = pool.clone();
        Box::pin(async move {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| AdminError::Database(e.to_string()))?;

            let per_page = if params.per_page == 0 {
                25
            } else {
                params.per_page
            };
            let offset = (params.page.saturating_sub(1)) * per_page;
            let search_pattern = format!("%{}%", params.search.as_deref().unwrap_or(""));

            let total: i64 = diesel::sql_query(
                "SELECT COUNT(*) FROM autumn_experiments \
                 WHERE (name ILIKE $1 OR COALESCE(description,'') ILIKE $1)",
            )
            .bind::<diesel::sql_types::Text, _>(&search_pattern)
            .get_result::<CountRow>(&mut conn)
            .await
            .map_or(0, |r| r.count);

            let records: Vec<Value> = diesel::sql_query(
                "SELECT id, name, description, state::text AS state, \
                        variants::text AS variants, winner, updated_at \
                 FROM autumn_experiments \
                 WHERE (name ILIKE $1 OR COALESCE(description,'') ILIKE $1) \
                 ORDER BY name \
                 LIMIT $2 OFFSET $3",
            )
            .bind::<diesel::sql_types::Text, _>(&search_pattern)
            .bind::<diesel::sql_types::BigInt, _>(i64::try_from(per_page).unwrap_or(i64::MAX))
            .bind::<diesel::sql_types::BigInt, _>(i64::try_from(offset).unwrap_or(0))
            .load::<ExperimentRow>(&mut conn)
            .await
            .map(|rows| rows.into_iter().map(ExperimentRow::into_json).collect())
            .map_err(|e| AdminError::Database(e.to_string()))?;

            Ok(ListResult {
                total: u64::try_from(total).unwrap_or(0),
                page: params.page,
                per_page,
                records,
            })
        })
    }

    fn get(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        id: i64,
    ) -> AdminFuture<'_, Option<Value>> {
        use diesel::prelude::*;
        use diesel_async::RunQueryDsl;

        let pool = pool.clone();
        Box::pin(async move {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| AdminError::Database(e.to_string()))?;

            diesel::sql_query(
                "SELECT id, name, description, state::text AS state, \
                        variants::text AS variants, winner, exclusion_group, updated_at \
                 FROM autumn_experiments WHERE id = $1",
            )
            .bind::<diesel::sql_types::BigInt, _>(id)
            .get_result::<ExperimentDetailRow>(&mut conn)
            .await
            .optional()
            .map(|r| r.map(ExperimentDetailRow::into_json))
            .map_err(|e| AdminError::Database(e.to_string()))
        })
    }

    fn create(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        data: Value,
    ) -> AdminFuture<'_, Value> {
        use diesel_async::RunQueryDsl;

        let pool = pool.clone();
        Box::pin(async move {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| AdminError::Database(e.to_string()))?;

            let name = data
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| AdminError::Validation("'name' is required".into()))?;
            let description = data.get("description").and_then(Value::as_str);
            let state = data
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("draft");
            let variants = validate_variants_json(&extract_variants_str(&data))?;
            let exclusion_group = data.get("exclusion_group").and_then(Value::as_str);

            let row = diesel::sql_query(
                "WITH inserted AS ( \
                     INSERT INTO autumn_experiments \
                         (name, description, state, variants, exclusion_group) \
                     VALUES ($1, $2, $3::autumn_experiment_state, $4::jsonb, $5) \
                     RETURNING id, name, description, state::text AS state, \
                               variants::text AS variants, winner, updated_at \
                 ), \
                 _audit AS ( \
                     INSERT INTO autumn_experiment_changes (experiment, mutation, actor) \
                     SELECT name, 'created', NULL FROM inserted \
                 ) \
                 SELECT id, name, description, state, variants, winner, updated_at \
                 FROM inserted",
            )
            .bind::<diesel::sql_types::Text, _>(name)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(
                description.map(str::to_owned),
            )
            .bind::<diesel::sql_types::Text, _>(state)
            .bind::<diesel::sql_types::Text, _>(variants)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(
                exclusion_group.map(str::to_owned),
            )
            .get_result::<ExperimentRow>(&mut conn)
            .await
            .map_err(|e| {
                if matches!(
                    e,
                    diesel::result::Error::DatabaseError(
                        diesel::result::DatabaseErrorKind::UniqueViolation,
                        _
                    )
                ) {
                    AdminError::Validation(format!(
                        "an experiment named '{name}' already exists"
                    ))
                } else {
                    AdminError::Database(e.to_string())
                }
            })?;

            Ok(ExperimentRow::into_json(row))
        })
    }

    fn update(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        id: i64,
        data: Value,
    ) -> AdminFuture<'_, Value> {
        use diesel_async::RunQueryDsl;

        let pool = pool.clone();
        Box::pin(async move {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| AdminError::Database(e.to_string()))?;

            // name is read-only after creation; we read it only to satisfy field validation
            // and for display — the SQL does not allow renaming an experiment.
            let description = data.get("description").and_then(Value::as_str);
            let state = data
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("draft");
            let variants = validate_variants_json(&extract_variants_str(&data))?;
            let winner = data.get("winner").and_then(Value::as_str);
            let exclusion_group = data.get("exclusion_group").and_then(Value::as_str);

            let row = diesel::sql_query(
                "WITH updated AS ( \
                     UPDATE autumn_experiments \
                     SET description = $2, \
                         state = $3::autumn_experiment_state, \
                         variants = $4::jsonb, winner = $5, \
                         exclusion_group = $6, updated_at = NOW() \
                     WHERE id = $1 \
                     RETURNING id, name, description, state::text AS state, \
                               variants::text AS variants, winner, updated_at \
                 ), \
                 _audit AS ( \
                     INSERT INTO autumn_experiment_changes (experiment, mutation, actor) \
                     SELECT name, 'updated', NULL FROM updated \
                 ) \
                 SELECT id, name, description, state, variants, winner, updated_at \
                 FROM updated",
            )
            .bind::<diesel::sql_types::BigInt, _>(id)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(
                description.map(str::to_owned),
            )
            .bind::<diesel::sql_types::Text, _>(state)
            .bind::<diesel::sql_types::Text, _>(variants)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(
                winner.map(str::to_owned),
            )
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(
                exclusion_group.map(str::to_owned),
            )
            .get_result::<ExperimentRow>(&mut conn)
            .await
            .map_err(|e| AdminError::Database(e.to_string()))?;

            Ok(ExperimentRow::into_json(row))
        })
    }

    fn delete(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        id: i64,
    ) -> AdminFuture<'_, ()> {
        use diesel_async::RunQueryDsl;

        let pool = pool.clone();
        Box::pin(async move {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| AdminError::Database(e.to_string()))?;

            diesel::sql_query(
                "WITH deleted AS ( \
                     DELETE FROM autumn_experiments WHERE id = $1 RETURNING name \
                 ), \
                 _del_assignments AS ( \
                     DELETE FROM autumn_experiment_assignments \
                     WHERE experiment IN (SELECT name FROM deleted) \
                 ), \
                 _del_overrides AS ( \
                     DELETE FROM autumn_experiment_overrides \
                     WHERE experiment IN (SELECT name FROM deleted) \
                 ), \
                 _audit AS ( \
                     INSERT INTO autumn_experiment_changes (experiment, mutation, actor) \
                     SELECT name, 'deleted', NULL FROM deleted \
                 ) \
                 SELECT COUNT(*) AS count FROM deleted",
            )
            .bind::<diesel::sql_types::BigInt, _>(id)
            .get_result::<CountRow>(&mut conn)
            .await
            .map_err(|e| AdminError::Database(e.to_string()))?;

            Ok(())
        })
    }

    fn get_history<'a>(
        &'a self,
        pool: &'a diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        record_id: i64,
        page: u64,
        per_page: u64,
    ) -> AdminFuture<'a, AdminHistoryPage> {
        use diesel::prelude::*;
        use diesel_async::RunQueryDsl;

        let pool = pool.clone();
        Box::pin(async move {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| AdminError::Database(e.to_string()))?;

            let name: Option<String> =
                diesel::sql_query("SELECT name FROM autumn_experiments WHERE id = $1")
                    .bind::<diesel::sql_types::BigInt, _>(record_id)
                    .get_result::<NameRow>(&mut conn)
                    .await
                    .optional()
                    .unwrap_or(None)
                    .map(|r| r.name);

            let Some(name) = name else {
                return Ok(AdminHistoryPage {
                    entries: vec![],
                    total: 0,
                    page,
                    per_page,
                });
            };

            let count: i64 = diesel::sql_query(
                "SELECT COUNT(*) FROM autumn_experiment_changes WHERE experiment = $1",
            )
            .bind::<diesel::sql_types::Text, _>(&name)
            .get_result::<CountRow>(&mut conn)
            .await
            .map_or(0, |r| r.count);

            let offset = (page.saturating_sub(1)) * per_page;
            let entries: Vec<crate::AdminHistoryEntry> = diesel::sql_query(
                "SELECT id, mutation AS op, actor, changed_at \
                 FROM autumn_experiment_changes \
                 WHERE experiment = $1 \
                 ORDER BY changed_at DESC \
                 LIMIT $2 OFFSET $3",
            )
            .bind::<diesel::sql_types::Text, _>(&name)
            .bind::<diesel::sql_types::BigInt, _>(i64::try_from(per_page).unwrap_or(i64::MAX))
            .bind::<diesel::sql_types::BigInt, _>(i64::try_from(offset).unwrap_or(0))
            .load::<HistoryRow>(&mut conn)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| crate::AdminHistoryEntry {
                id: r.id,
                actor: r.actor.unwrap_or_else(|| "cli".to_owned()),
                op: r.op,
                request_id: None,
                changes: vec![],
                recorded_at: r.changed_at,
            })
            .collect();

            Ok(AdminHistoryPage {
                entries,
                total: u64::try_from(count).unwrap_or(0),
                page,
                per_page,
            })
        })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract `variants` from admin form data, handling both pre-parsed JSON arrays
/// (normalized by the admin route) and raw JSON strings submitted by the form.
fn extract_variants_str(data: &Value) -> String {
    match data.get("variants") {
        Some(Value::String(s)) => s.clone(),
        Some(v) if !v.is_null() => {
            serde_json::to_string(v).unwrap_or_else(|_| "[]".to_owned())
        }
        _ => "[]".to_owned(),
    }
}

/// Validate `raw` as a JSON array of `{"name": string, "weight": integer}` objects.
fn validate_variants_json(raw: &str) -> Result<String, AdminError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok("[]".to_owned());
    }
    match serde_json::from_str::<Vec<serde_json::Value>>(trimmed) {
        Ok(arr) => {
            for (i, v) in arr.iter().enumerate() {
                if v.get("name").and_then(|n| n.as_str()).is_none() {
                    return Err(AdminError::Validation(format!(
                        "variants[{i}].name must be a string"
                    )));
                }
                if v.get("weight")
                    .and_then(Value::as_u64)
                    .is_none()
                {
                    return Err(AdminError::Validation(format!(
                        "variants[{i}].weight must be a non-negative integer"
                    )));
                }
            }
            Ok(serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_owned()))
        }
        Err(_) => Err(AdminError::Validation(
            "'variants' must be a valid JSON array".into(),
        )),
    }
}

// ── Row types ─────────────────────────────────────────────────────────────────

#[derive(diesel::QueryableByName)]
struct CountRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

#[derive(diesel::QueryableByName)]
struct NameRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    name: String,
}

/// Row returned from list queries (no `exclusion_group` for brevity in list view).
#[derive(diesel::QueryableByName)]
struct ExperimentRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    id: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    name: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    description: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Text)]
    state: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    variants: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    winner: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Timestamptz)]
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl ExperimentRow {
    fn into_json(self) -> Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "description": self.description,
            "state": self.state,
            "variants": self.variants,
            "winner": self.winner,
            "updated_at": self.updated_at.to_rfc3339(),
        })
    }
}

/// Row returned from detail (get) queries — includes `exclusion_group`.
#[derive(diesel::QueryableByName)]
struct ExperimentDetailRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    id: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    name: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    description: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Text)]
    state: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    variants: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    winner: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    exclusion_group: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Timestamptz)]
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl ExperimentDetailRow {
    fn into_json(self) -> Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "description": self.description,
            "state": self.state,
            "variants": self.variants,
            "winner": self.winner,
            "exclusion_group": self.exclusion_group,
            "updated_at": self.updated_at.to_rfc3339(),
        })
    }
}

#[derive(diesel::QueryableByName)]
struct HistoryRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    id: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    op: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    actor: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Timestamptz)]
    changed_at: chrono::DateTime<chrono::Utc>,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experiment_admin_model_slug() {
        let model = ExperimentAdminModel;
        assert_eq!(model.slug(), "experiments");
    }

    #[test]
    fn experiment_admin_model_display_names() {
        let model = ExperimentAdminModel;
        assert_eq!(model.display_name(), "Experiment");
        assert_eq!(model.display_name_plural(), "Experiments");
    }

    #[test]
    fn experiment_admin_model_has_history() {
        let model = ExperimentAdminModel;
        assert!(model.has_history(), "experiment admin must expose history");
    }

    #[test]
    fn experiment_admin_model_has_expected_fields() {
        let model = ExperimentAdminModel;
        let fields = model.fields();
        let names: Vec<&str> = fields.iter().map(|f| f.name).collect();
        assert!(names.contains(&"name"), "must have 'name' field");
        assert!(names.contains(&"state"), "must have 'state' field");
        assert!(names.contains(&"variants"), "must have 'variants' field");
        assert!(names.contains(&"winner"), "must have 'winner' field");
        assert!(
            names.contains(&"exclusion_group"),
            "must have 'exclusion_group' field"
        );
    }

    #[test]
    fn experiment_admin_model_state_field_has_all_lifecycle_states() {
        let model = ExperimentAdminModel;
        let state_field = model
            .fields()
            .into_iter()
            .find(|f| f.name == "state")
            .expect("state field must exist");
        let AdminFieldKind::Select(options) = state_field.kind else {
            panic!("state field must be Select");
        };
        let values: Vec<&str> = options.iter().map(|o| o.value.as_str()).collect();
        assert!(values.contains(&"draft"));
        assert!(values.contains(&"running"));
        assert!(values.contains(&"concluded"));
        assert!(values.contains(&"archived"));
    }

    #[test]
    fn record_display_uses_experiment_name() {
        let model = ExperimentAdminModel;
        let record = serde_json::json!({"name": "checkout_v2", "state": "running"});
        assert_eq!(model.record_display(&record), "Experiment: checkout_v2");
    }

    #[test]
    fn record_display_fallback_when_no_name() {
        let model = ExperimentAdminModel;
        let record = serde_json::json!({});
        assert_eq!(model.record_display(&record), "Experiment");
    }

    #[test]
    fn validate_variants_json_accepts_valid_array() {
        let json =
            validate_variants_json(r#"[{"name":"control","weight":50},{"name":"treatment","weight":50}]"#)
                .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
    }

    #[test]
    fn validate_variants_json_accepts_empty_string() {
        assert_eq!(validate_variants_json("").unwrap(), "[]");
    }

    #[test]
    fn validate_variants_json_rejects_missing_name() {
        let err = validate_variants_json(r#"[{"weight":50}]"#).unwrap_err();
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn validate_variants_json_rejects_missing_weight() {
        let err = validate_variants_json(r#"[{"name":"control"}]"#).unwrap_err();
        assert!(err.to_string().contains("weight"));
    }

    #[test]
    fn validate_variants_json_rejects_invalid_json() {
        let err = validate_variants_json("{not json}").unwrap_err();
        assert!(err.to_string().contains("JSON"));
    }
}
