//! Event store — append and load workflow event histories.
//!
//! The event store is the persistence backbone of durable execution.
//! Every side effect in a workflow is recorded as an event; on replay the
//! engine loads the full history and feeds recorded results back instead
//! of re-executing activities.

use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use crate::error::{HarvestError, HarvestResult};
use crate::event::WorkflowEvent;
use crate::models::NewHarvestEvent;
use crate::schema::harvest_events;
use crate::types::ExecutionId;

// ── Writer helpers ───────────────────────────────────────────────────

/// Convert a slice of `WorkflowEvent`s into insertable rows, starting
/// event IDs at 0.
pub fn events_to_insert_rows(
    exec_id: ExecutionId,
    events: &[WorkflowEvent],
) -> Vec<NewHarvestEvent<'_>> {
    events_to_insert_rows_from(exec_id, 0, events)
}

/// Convert a slice of `WorkflowEvent`s into insertable rows, starting
/// event IDs at `start_event_id`.
pub fn events_to_insert_rows_from(
    exec_id: ExecutionId,
    start_event_id: i32,
    events: &[WorkflowEvent],
) -> Vec<NewHarvestEvent<'_>> {
    let uuid = exec_id.as_uuid();
    events
        .iter()
        .enumerate()
        .map(|(i, evt)| {
            let event_id = start_event_id + i32::try_from(i).expect("event index fits i32");
            NewHarvestEvent {
                workflow_exec_id: uuid,
                event_id,
                event_type: evt.type_name(),
                event_data: serde_json::to_value(evt)
                    .expect("WorkflowEvent is always serializable"),
            }
        })
        .collect()
}

/// Append events to the event store for a workflow execution.
///
/// Uses `events_to_insert_rows_from` starting at `next_event_id` so the
/// caller controls the continuation point (typically from `EventHistory`).
pub async fn append_events(
    conn: &mut diesel_async::AsyncPgConnection,
    exec_id: ExecutionId,
    next_event_id: i32,
    events: &[WorkflowEvent],
) -> HarvestResult<()> {
    if events.is_empty() {
        return Ok(());
    }

    let rows = events_to_insert_rows_from(exec_id, next_event_id, events);

    diesel::insert_into(harvest_events::table)
        .values(&rows)
        .execute(conn)
        .await
        .map_err(|e| HarvestError::Database(e.to_string()))?;

    Ok(())
}

// ── Reader ───────────────────────────────────────────────────────────

/// Loaded event history for a single workflow execution.
///
/// Contains the deserialized events and a continuation counter so the
/// caller knows which `event_id` to use when appending new events.
#[derive(Debug)]
pub struct EventHistory {
    /// The workflow execution this history belongs to.
    pub exec_id: ExecutionId,
    /// Deserialized events in order of `event_id ASC`.
    pub events: Vec<WorkflowEvent>,
    /// The next `event_id` to use when appending (last + 1, or 0 if empty).
    pub next_event_id: i32,
}

/// Load the full event history for a workflow execution.
///
/// Reads all rows from `harvest_events` where `workflow_exec_id` matches,
/// ordered by `event_id ASC`, and deserializes each `event_data` JSON blob
/// back into a [`WorkflowEvent`].
pub async fn load_history(
    conn: &mut diesel_async::AsyncPgConnection,
    exec_id: ExecutionId,
) -> HarvestResult<EventHistory> {
    let rows: Vec<crate::models::HarvestEvent> = harvest_events::table
        .filter(harvest_events::workflow_exec_id.eq(exec_id.as_uuid()))
        .order(harvest_events::event_id.asc())
        .load(conn)
        .await
        .map_err(|e| HarvestError::Database(e.to_string()))?;

    let mut events = Vec::with_capacity(rows.len());
    let mut last_event_id: Option<i32> = None;

    for row in &rows {
        let evt: WorkflowEvent =
            serde_json::from_value(row.event_data.clone()).map_err(HarvestError::Serialization)?;
        last_event_id = Some(row.event_id);
        events.push(evt);
    }

    let next_event_id = last_event_id.map_or(0, |id| id + 1);

    Ok(EventHistory {
        exec_id,
        events,
        next_event_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ActivityExecId;
    use chrono::Utc;

    /// Round-trip test: serialize events through `events_to_insert_rows`,
    /// then deserialize the `event_data` JSON values back and verify they
    /// match the original event variants.
    #[test]
    fn history_from_rows_deserializes_events() {
        let exec_id = ExecutionId::new();
        let now = Utc::now();

        let original_events = vec![
            WorkflowEvent::WorkflowStarted {
                input: serde_json::json!({"order_id": 99}),
                timestamp: now,
            },
            WorkflowEvent::ActivityScheduled {
                activity_id: ActivityExecId::new(),
                name: "charge_card".into(),
                input: serde_json::json!({"amount": 42.50}),
                queue: "payments".into(),
            },
            WorkflowEvent::ActivityCompleted {
                activity_id: ActivityExecId::new(),
                output: serde_json::json!({"txn_id": "abc-123"}),
            },
            WorkflowEvent::WorkflowCompleted {
                output: serde_json::json!({"status": "done"}),
            },
        ];

        // Serialize through the insert-row helper (this is what the writer does).
        let rows = events_to_insert_rows(exec_id, &original_events);
        assert_eq!(rows.len(), 4);

        // Verify event_ids are sequential starting at 0.
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row.event_id, i32::try_from(i).unwrap());
            assert_eq!(row.workflow_exec_id, exec_id.as_uuid());
        }

        // Verify event_type strings match.
        assert_eq!(rows[0].event_type, "WorkflowStarted");
        assert_eq!(rows[1].event_type, "ActivityScheduled");
        assert_eq!(rows[2].event_type, "ActivityCompleted");
        assert_eq!(rows[3].event_type, "WorkflowCompleted");

        // Deserialize each event_data back into WorkflowEvent — the reader path.
        let deserialized: Vec<WorkflowEvent> = rows
            .iter()
            .map(|r| serde_json::from_value(r.event_data.clone()).unwrap())
            .collect();

        assert!(matches!(
            deserialized[0],
            WorkflowEvent::WorkflowStarted { .. }
        ));
        assert!(matches!(
            deserialized[1],
            WorkflowEvent::ActivityScheduled { .. }
        ));
        assert!(matches!(
            deserialized[2],
            WorkflowEvent::ActivityCompleted { .. }
        ));
        assert!(matches!(
            deserialized[3],
            WorkflowEvent::WorkflowCompleted { .. }
        ));

        // Verify data fidelity on the first event.
        if let WorkflowEvent::WorkflowStarted { input, .. } = &deserialized[0] {
            assert_eq!(input["order_id"], 99);
        } else {
            panic!("expected WorkflowStarted");
        }

        // Verify data fidelity on the activity event.
        if let WorkflowEvent::ActivityScheduled { name, queue, .. } = &deserialized[1] {
            assert_eq!(name, "charge_card");
            assert_eq!(queue, "payments");
        } else {
            panic!("expected ActivityScheduled");
        }

        // Verify next_event_id computation matches what load_history would produce.
        let last_event_id = rows.last().map(|r| r.event_id);
        let next_event_id = last_event_id.map_or(0, |id| id + 1);
        assert_eq!(next_event_id, 4);
    }

    #[test]
    fn events_to_insert_rows_from_offsets_correctly() {
        let exec_id = ExecutionId::new();
        let events = vec![WorkflowEvent::WorkflowCompleted {
            output: serde_json::json!("ok"),
        }];

        let rows = events_to_insert_rows_from(exec_id, 7, &events);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_id, 7);
    }

    #[test]
    fn empty_history_next_event_id_is_zero() {
        // Simulate what load_history computes for an empty result set.
        let last_event_id: Option<i32> = None;
        let next = last_event_id.map_or(0, |id| id + 1);
        assert_eq!(next, 0);
    }
}
