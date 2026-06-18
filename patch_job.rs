<<<<<<< SEARCH
        JobExecutionOutcome::Failed(error) => {
            if job.attempt < max_attempts {
                // Running-window keys stay held across retries (the job is
                // still in flight until it settles). A pending-window key was
                // released when execution started, so re-acquire it now to
                // keep duplicates coalescing while the retry waits out its
                // backoff as a pending job again. If a duplicate was accepted
                // while this job ran it now owns the key; in that case drop
                // the retry (coalesce into the duplicate) rather than letting
                // both run unprotected.
                if let Some(unique) = &uniqueness
                    && unique.window == JobUniquenessWindow::Pending
                {
                    let key = job_unique_key(unique, &job.payload);
                    if !coordination.try_acquire_unique(&job.name, &key, &job.id, unique.window) {
                        state.job_registry.record_deduplicated(&job.name);
                        job_admin.record_deduplicated(&job.id);
                        finish_local_slot(coordination, concurrency_group.as_ref(), tx, state);
                        return;
                    }
                }
                state
                    .job_registry
                    .record_retry(&job.name, &error, job.attempt);
                job_admin.record_retrying(&job.id, &error);
                let sender = tx.clone();
                let registry = state.job_registry.clone();
                let job_admin = job_admin.clone();
                let id = job.id.clone();
                let name = job.name.clone();
                let payload = job.payload;
                #[cfg(feature = "telemetry-otlp")]
                let traceparent = job.traceparent;
                #[cfg(feature = "telemetry-otlp")]
                let tracestate = job.tracestate;
                let delay = backoff_ms.saturating_mul(2_u64.saturating_pow(job.attempt - 1));
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    registry.record_enqueue(&name);
                    job_admin.record_requeued(&id, job.attempt + 1);
                    let _ = sender
                        .send(QueuedJob {
                            id,
                            name,
                            payload,
                            attempt: job.attempt + 1,
                            max_attempts,
                            initial_backoff_ms: backoff_ms,
                            #[cfg(feature = "telemetry-otlp")]
                            traceparent,
                            #[cfg(feature = "telemetry-otlp")]
                            tracestate,
                        })
                        .await;
                });
            } else {
                state
                    .job_registry
                    .record_failure(&job.name, error.clone(), true);
                job_admin.record_failure(&job.id, error);
                release_local_unique_hold(
                    coordination,
                    uniqueness.as_ref(),
                    &job.name,
                    &job.payload,
                    &job.id,
                );
            }
        }
=======
        JobExecutionOutcome::Failed(error) => {
            let retry_handled = handle_failed_local_job(
                &job,
                &error,
                max_attempts,
                backoff_ms,
                state,
                job_admin,
                coordination,
                uniqueness.as_ref(),
                tx,
            );
            if retry_handled {
                finish_local_slot(coordination, concurrency_group.as_ref(), tx, state);
                return;
            }
        }
>>>>>>> REPLACE
<<<<<<< SEARCH
/// Release a unique hold after a job settles. No-op for TTL-window holds
=======
#[allow(clippy::too_many_arguments)]
fn handle_failed_local_job(
    job: &QueuedJob,
    error: &str,
    max_attempts: u32,
    backoff_ms: u64,
    state: &AppState,
    job_admin: &JobAdminMemoryBackend,
    coordination: &Arc<LocalJobCoordination>,
    uniqueness: Option<&JobUniqueness>,
    tx: &tokio::sync::mpsc::Sender<QueuedJob>,
) -> bool {
    if job.attempt < max_attempts {
        // Running-window keys stay held across retries (the job is
        // still in flight until it settles). A pending-window key was
        // released when execution started, so re-acquire it now to
        // keep duplicates coalescing while the retry waits out its
        // backoff as a pending job again. If a duplicate was accepted
        // while this job ran it now owns the key; in that case drop
        // the retry (coalesce into the duplicate) rather than letting
        // both run unprotected.
        if let Some(unique) = uniqueness
            && unique.window == JobUniquenessWindow::Pending
        {
            let key = job_unique_key(unique, &job.payload);
            if !coordination.try_acquire_unique(&job.name, &key, &job.id, unique.window) {
                state.job_registry.record_deduplicated(&job.name);
                job_admin.record_deduplicated(&job.id);
                return true; // Indicate that execution should return early
            }
        }
        state
            .job_registry
            .record_retry(&job.name, error, job.attempt);
        job_admin.record_retrying(&job.id, error);
        let sender = tx.clone();
        let registry = state.job_registry.clone();
        let job_admin = job_admin.clone();
        let id = job.id.clone();
        let name = job.name.clone();
        let payload = job.payload.clone();
        #[cfg(feature = "telemetry-otlp")]
        let traceparent = job.traceparent.clone();
        #[cfg(feature = "telemetry-otlp")]
        let tracestate = job.tracestate.clone();
        let attempt = job.attempt;
        let delay = backoff_ms.saturating_mul(2_u64.saturating_pow(attempt - 1));
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            registry.record_enqueue(&name);
            job_admin.record_requeued(&id, attempt + 1);
            let _ = sender
                .send(QueuedJob {
                    id,
                    name,
                    payload,
                    attempt: attempt + 1,
                    max_attempts,
                    initial_backoff_ms: backoff_ms,
                    #[cfg(feature = "telemetry-otlp")]
                    traceparent,
                    #[cfg(feature = "telemetry-otlp")]
                    tracestate,
                })
                .await;
        });
    } else {
        state
            .job_registry
            .record_failure(&job.name, error.to_owned(), true);
        job_admin.record_failure(&job.id, error.to_owned());
        release_local_unique_hold(
            coordination,
            uniqueness,
            &job.name,
            &job.payload,
            &job.id,
        );
    }
    false
}

/// Release a unique hold after a job settles. No-op for TTL-window holds
>>>>>>> REPLACE
