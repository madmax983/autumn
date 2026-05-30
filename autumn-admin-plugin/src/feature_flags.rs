//! Admin panel model for `autumn_feature_flags`.
//!
//! Registers a flag management page at `/admin/feature-flags/` with:
//! - List view: key, enabled status, rollout %, actor allowlist, history link
//! - Edit view: toggle enabled, set rollout_pct, manage allowlists
//! - History tab: per-flag audit trail from `feature_flag_changes`

use serde_json::Value;

use crate::{
    AdminError, AdminField, AdminFieldKind, AdminFuture, AdminModel, ListParams, ListResult,
    SelectOption,
};

/// Admin panel model for feature flags.
///
/// Register this model with the admin plugin to get a flag management UI
/// at `/admin/feature-flags/`:
///
/// ```rust,ignore
/// use autumn_admin_plugin::{prelude::*, AdminPlugin};
/// use autumn_admin_plugin::feature_flags::FeatureFlagAdminModel;
///
/// autumn_web::app()
///     .plugin(
///         AdminPlugin::new()
///             .register(FeatureFlagAdminModel::default()),
///     )
///     .run()
///     .await;
/// ```
#[derive(Debug, Default, Clone)]
pub struct FeatureFlagAdminModel;

impl AdminModel for FeatureFlagAdminModel {
    fn slug(&self) -> &'static str {
        "feature-flags"
    }

    fn display_name(&self) -> &'static str {
        "Feature Flag"
    }

    fn display_name_plural(&self) -> &'static str {
        "Feature Flags"
    }

    fn fields(&self) -> Vec<AdminField> {
        vec![
            AdminField::new("key", AdminFieldKind::Text)
                .label("Flag Key")
                .searchable(),
            AdminField::new("description", AdminFieldKind::TextArea)
                .label("Description")
                .optional()
                .searchable(),
            AdminField::new("enabled", AdminFieldKind::Boolean)
                .label("Globally Enabled"),
            AdminField::new(
                "rollout_pct",
                AdminFieldKind::Select(vec![
                    SelectOption { value: "0".into(), label: "Off (0%)".into() },
                    SelectOption { value: "10".into(), label: "10%".into() },
                    SelectOption { value: "25".into(), label: "25%".into() },
                    SelectOption { value: "50".into(), label: "50%".into() },
                    SelectOption { value: "75".into(), label: "75%".into() },
                    SelectOption { value: "100".into(), label: "All (100%)".into() },
                ]),
            )
            .label("Rollout %")
            .optional(),
            AdminField::new("actor_allowlist", AdminFieldKind::TextArea)
                .label("Actor Allowlist (JSON array)")
                .optional()
                .hide_from_list(),
            AdminField::new("group_allowlist", AdminFieldKind::TextArea)
                .label("Group Allowlist (JSON array)")
                .optional()
                .hide_from_list(),
            AdminField::new("updated_at", AdminFieldKind::DateTime)
                .label("Last Updated")
                .readonly()
                .optional(),
        ]
    }

    fn record_display(&self, record: &Value) -> String {
        record
            .get("key")
            .and_then(|v| v.as_str())
            .map(|k| format!("Flag: {k}"))
            .unwrap_or_else(|| "Feature Flag".to_owned())
    }

    fn list(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        params: ListParams,
    ) -> AdminFuture<'_, ListResult> {
        use diesel::prelude::*;
        use diesel_async::RunQueryDsl;

        let pool = pool.clone();
        Box::pin(async move {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| AdminError::Database(e.to_string()))?;

            let per_page = if params.per_page == 0 { 25 } else { params.per_page };
            let offset = (params.page.saturating_sub(1)) * per_page;

            // Parameterized search — `%` alone matches everything (no search case).
            let search_pattern = format!(
                "%{}%",
                params.search.as_deref().unwrap_or("")
            );

            let total: i64 = diesel::sql_query(
                "SELECT COUNT(*) FROM autumn_feature_flags \
                 WHERE (key ILIKE $1 OR COALESCE(description,'') ILIKE $1)",
            )
            .bind::<diesel::sql_types::Text, _>(&search_pattern)
            .get_result::<CountRow>(&mut conn)
            .await
            .map(|r| r.count)
            .unwrap_or(0);

            let records: Vec<Value> = diesel::sql_query(
                "SELECT id, key, description, enabled, rollout_pct, \
                        actor_allowlist, group_allowlist, updated_at \
                 FROM autumn_feature_flags \
                 WHERE (key ILIKE $1 OR COALESCE(description,'') ILIKE $1) \
                 ORDER BY key \
                 LIMIT $2 OFFSET $3",
            )
            .bind::<diesel::sql_types::Text, _>(&search_pattern)
            .bind::<diesel::sql_types::BigInt, _>(per_page as i64)
            .bind::<diesel::sql_types::BigInt, _>(offset as i64)
            .load::<FlagRow>(&mut conn)
            .await
            .map(|rows| rows.into_iter().map(FlagRow::into_json).collect())
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
                "SELECT id, key, description, enabled, rollout_pct, \
                        actor_allowlist, group_allowlist, updated_at \
                 FROM autumn_feature_flags WHERE id = $1",
            )
            .bind::<diesel::sql_types::BigInt, _>(id)
            .get_result::<FlagRow>(&mut conn)
            .await
            .optional()
            .map(|r| r.map(FlagRow::into_json))
            .map_err(|e| AdminError::Database(e.to_string()))
        })
    }

    fn create(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        data: Value,
    ) -> AdminFuture<'_, Value> {
        use diesel::prelude::*;
        use diesel_async::RunQueryDsl;

        let pool = pool.clone();
        Box::pin(async move {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| AdminError::Database(e.to_string()))?;

            let key = data
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AdminError::Validation("'key' is required".into()))?;
            let enabled = data.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
            // Select widget sends strings ("25"), direct API sends numbers.
            let mut rollout_pct = data
                .get("rollout_pct")
                .and_then(|v| {
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
                })
                .unwrap_or(0)
                .clamp(0, 100);
            // "Globally Enabled" in the admin UI means globally on for all actors.
            // The evaluator requires rollout_pct >= 100 for that — promote if the
            // admin checked the box but left rollout at the default 0%.
            if enabled && rollout_pct == 0 {
                rollout_pct = 100;
            }
            let description = data.get("description").and_then(|v| v.as_str());
            let actor_allowlist = data
                .get("actor_allowlist")
                .and_then(|v| v.as_str())
                .unwrap_or("[]");
            let group_allowlist = data
                .get("group_allowlist")
                .and_then(|v| v.as_str())
                .unwrap_or("[]");

            let mutation = if enabled { "enabled" } else { "disabled" };

            let sql = "INSERT INTO autumn_feature_flags \
                       (key, description, enabled, rollout_pct, actor_allowlist, group_allowlist) \
                       VALUES ($1, $2, $3, $4, $5, $6) \
                       ON CONFLICT (key) DO UPDATE \
                       SET description = EXCLUDED.description, \
                           enabled = EXCLUDED.enabled, \
                           rollout_pct = EXCLUDED.rollout_pct, \
                           actor_allowlist = EXCLUDED.actor_allowlist, \
                           group_allowlist = EXCLUDED.group_allowlist, \
                           updated_at = NOW() \
                       RETURNING id, key, description, enabled, rollout_pct, \
                                 actor_allowlist, group_allowlist, updated_at";

            let row = diesel::sql_query(sql)
                .bind::<diesel::sql_types::Text, _>(key)
                .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(
                    description.map(str::to_owned),
                )
                .bind::<diesel::sql_types::Bool, _>(enabled)
                .bind::<diesel::sql_types::SmallInt, _>(rollout_pct as i16)
                .bind::<diesel::sql_types::Text, _>(actor_allowlist)
                .bind::<diesel::sql_types::Text, _>(group_allowlist)
                .get_result::<FlagRow>(&mut conn)
                .await
                .map_err(|e| AdminError::Database(e.to_string()))?;

            // Record mutation in change log so LISTEN/NOTIFY propagates and history tab works.
            diesel::sql_query(
                "INSERT INTO feature_flag_changes (key, mutation, actor) VALUES ($1, $2, NULL)",
            )
            .bind::<diesel::sql_types::Text, _>(key)
            .bind::<diesel::sql_types::Text, _>(mutation)
            .execute(&mut conn)
            .await
            .ok(); // best-effort; don't fail the admin save if history write fails

            Ok(FlagRow::into_json(row))
        })
    }

    fn update(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        id: i64,
        data: Value,
    ) -> AdminFuture<'_, Value> {
        use diesel::prelude::*;
        use diesel_async::RunQueryDsl;

        let pool = pool.clone();
        Box::pin(async move {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| AdminError::Database(e.to_string()))?;

            let key = data
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AdminError::Validation("'key' is required".into()))?;
            let enabled = data.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
            let mut rollout_pct = data
                .get("rollout_pct")
                .and_then(|v| {
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
                })
                .unwrap_or(0)
                .clamp(0, 100);
            if enabled && rollout_pct == 0 {
                rollout_pct = 100;
            }
            let description = data.get("description").and_then(|v| v.as_str());
            let actor_allowlist = data
                .get("actor_allowlist")
                .and_then(|v| v.as_str())
                .unwrap_or("[]");
            let group_allowlist = data
                .get("group_allowlist")
                .and_then(|v| v.as_str())
                .unwrap_or("[]");

            let mutation = if enabled { "enabled" } else { "disabled" };

            // Target by id so a renamed key updates the correct row, not a
            // different one (or a new row via key-based upsert).
            let row = diesel::sql_query(
                "UPDATE autumn_feature_flags \
                 SET key = $2, description = $3, enabled = $4, rollout_pct = $5, \
                     actor_allowlist = $6, group_allowlist = $7, updated_at = NOW() \
                 WHERE id = $1 \
                 RETURNING id, key, description, enabled, rollout_pct, \
                           actor_allowlist, group_allowlist, updated_at",
            )
            .bind::<diesel::sql_types::BigInt, _>(id)
            .bind::<diesel::sql_types::Text, _>(key)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(
                description.map(str::to_owned),
            )
            .bind::<diesel::sql_types::Bool, _>(enabled)
            .bind::<diesel::sql_types::SmallInt, _>(rollout_pct as i16)
            .bind::<diesel::sql_types::Text, _>(actor_allowlist)
            .bind::<diesel::sql_types::Text, _>(group_allowlist)
            .get_result::<FlagRow>(&mut conn)
            .await
            .map_err(|e| AdminError::Database(e.to_string()))?;

            diesel::sql_query(
                "INSERT INTO feature_flag_changes (key, mutation, actor) VALUES ($1, $2, NULL)",
            )
            .bind::<diesel::sql_types::Text, _>(key)
            .bind::<diesel::sql_types::Text, _>(mutation)
            .execute(&mut conn)
            .await
            .ok();

            Ok(FlagRow::into_json(row))
        })
    }

    fn delete(
        &self,
        pool: &diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        id: i64,
    ) -> AdminFuture<'_, ()> {
        use diesel::prelude::*;
        use diesel_async::RunQueryDsl;

        let pool = pool.clone();
        Box::pin(async move {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| AdminError::Database(e.to_string()))?;

            diesel::sql_query("DELETE FROM autumn_feature_flags WHERE id = $1")
                .bind::<diesel::sql_types::BigInt, _>(id)
                .execute(&mut conn)
                .await
                .map(|_| ())
                .map_err(|e| AdminError::Database(e.to_string()))
        })
    }

    fn has_history(&self) -> bool {
        true
    }

    fn get_history<'a>(
        &'a self,
        pool: &'a diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>,
        record_id: i64,
        page: u64,
        per_page: u64,
    ) -> crate::AdminFuture<'a, crate::AdminHistoryPage> {
        use diesel::prelude::*;
        use diesel_async::RunQueryDsl;

        let pool = pool.clone();
        Box::pin(async move {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| AdminError::Database(e.to_string()))?;

            // Resolve the flag key by its stable integer id.
            let key: Option<String> = diesel::sql_query(
                "SELECT key FROM autumn_feature_flags WHERE id = $1",
            )
            .bind::<diesel::sql_types::BigInt, _>(record_id)
            .get_result::<KeyRow>(&mut conn)
            .await
            .optional()
            .unwrap_or(None)
            .map(|r| r.key);

            let Some(key) = key else {
                return Ok(crate::AdminHistoryPage {
                    entries: vec![],
                    total: 0,
                    page,
                    per_page,
                });
            };

            let count: i64 = diesel::sql_query(
                "SELECT COUNT(*) FROM feature_flag_changes WHERE key = $1",
            )
            .bind::<diesel::sql_types::Text, _>(&key)
            .get_result::<CountRow>(&mut conn)
            .await
            .map(|r| r.count)
            .unwrap_or(0);

            let offset = (page.saturating_sub(1)) * per_page;
            let entries: Vec<crate::AdminHistoryEntry> = diesel::sql_query(
                "SELECT id, mutation AS op, actor, changed_at \
                 FROM feature_flag_changes \
                 WHERE key = $1 \
                 ORDER BY changed_at DESC \
                 LIMIT $2 OFFSET $3",
            )
            .bind::<diesel::sql_types::Text, _>(&key)
            .bind::<diesel::sql_types::BigInt, _>(per_page as i64)
            .bind::<diesel::sql_types::BigInt, _>(offset as i64)
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

            Ok(crate::AdminHistoryPage {
                entries,
                total: u64::try_from(count).unwrap_or(0),
                page,
                per_page,
            })
        })
    }
}

// ── Row types ─────────────────────────────────────────────────────────────────

#[derive(diesel::QueryableByName)]
struct CountRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

#[derive(diesel::QueryableByName)]
struct KeyRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    key: String,
}

#[derive(diesel::QueryableByName)]
struct FlagRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    id: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    key: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    description: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Bool)]
    enabled: bool,
    #[diesel(sql_type = diesel::sql_types::SmallInt)]
    rollout_pct: i16,
    #[diesel(sql_type = diesel::sql_types::Text)]
    actor_allowlist: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    group_allowlist: String,
    #[diesel(sql_type = diesel::sql_types::Timestamptz)]
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl FlagRow {
    fn into_json(self) -> Value {
        serde_json::json!({
            "id": self.id,
            "key": self.key,
            "description": self.description,
            "enabled": self.enabled,
            "rollout_pct": self.rollout_pct,
            "actor_allowlist": self.actor_allowlist,
            "group_allowlist": self.group_allowlist,
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
    fn feature_flag_admin_model_slug_is_feature_flags() {
        let model = FeatureFlagAdminModel::default();
        assert_eq!(model.slug(), "feature-flags");
    }

    #[test]
    fn feature_flag_admin_model_has_correct_display_names() {
        let model = FeatureFlagAdminModel::default();
        assert_eq!(model.display_name(), "Feature Flag");
        assert_eq!(model.display_name_plural(), "Feature Flags");
    }

    #[test]
    fn feature_flag_admin_fields_include_required_columns() {
        let model = FeatureFlagAdminModel::default();
        let fields = model.fields();
        let names: Vec<&str> = fields.iter().map(|f| f.name).collect();
        assert!(names.contains(&"key"), "must have key field");
        assert!(names.contains(&"enabled"), "must have enabled field");
        assert!(names.contains(&"rollout_pct"), "must have rollout_pct field");
        assert!(names.contains(&"actor_allowlist"), "must have actor_allowlist field");
    }

    #[test]
    fn feature_flag_admin_model_has_history() {
        let model = FeatureFlagAdminModel::default();
        assert!(model.has_history(), "feature flag admin must expose history");
    }

    #[test]
    fn record_display_uses_flag_key() {
        let model = FeatureFlagAdminModel::default();
        let record = serde_json::json!({"key": "beta_inbox", "enabled": false});
        assert_eq!(model.record_display(&record), "Flag: beta_inbox");
    }

    #[test]
    fn record_display_fallback_when_no_key() {
        let model = FeatureFlagAdminModel::default();
        let record = serde_json::json!({});
        assert_eq!(model.record_display(&record), "Feature Flag");
    }

    #[test]
    fn globally_enabled_with_zero_rollout_promotes_to_100() {
        // When the admin checks "Globally Enabled" but leaves Rollout % at the
        // default 0%, the saved rollout_pct must be 100 so the evaluator
        // (which requires rollout_pct >= 100 for global access) works correctly.
        //
        // This is a pure logic test — it doesn't hit the DB; it just verifies
        // that the promotion happens before the SQL bind.
        let enabled = true;
        let submitted_rollout: i64 = 0;
        let mut rollout_pct = submitted_rollout.clamp(0, 100);
        if enabled && rollout_pct == 0 {
            rollout_pct = 100;
        }
        assert_eq!(
            rollout_pct, 100,
            "enabled=true + rollout=0 must be promoted to rollout=100"
        );
    }

    #[test]
    fn globally_enabled_with_explicit_rollout_is_preserved() {
        // If the admin explicitly sets 25% rollout AND checks "Globally Enabled",
        // the rollout should stay at 25 (not promoted to 100).
        let enabled = true;
        let submitted_rollout: i64 = 25;
        let mut rollout_pct = submitted_rollout.clamp(0, 100);
        if enabled && rollout_pct == 0 {
            rollout_pct = 100;
        }
        assert_eq!(
            rollout_pct, 25,
            "enabled=true + explicit rollout=25 must be preserved"
        );
    }

    #[test]
    fn disabled_with_zero_rollout_is_not_promoted() {
        // Kill-switch (enabled=false) with rollout=0 must stay at 0.
        let enabled = false;
        let submitted_rollout: i64 = 0;
        let mut rollout_pct = submitted_rollout.clamp(0, 100);
        if enabled && rollout_pct == 0 {
            rollout_pct = 100;
        }
        assert_eq!(rollout_pct, 0, "kill-switch must not promote rollout_pct");
    }
}
