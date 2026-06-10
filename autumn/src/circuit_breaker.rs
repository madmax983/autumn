#![allow(
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::new_without_default,
    clippy::missing_const_for_fn,
    clippy::items_after_statements,
    clippy::cast_precision_loss,
    clippy::collapsible_if
)]

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

impl CircuitState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "CLOSED",
            Self::Open => "OPEN",
            Self::HalfOpen => "HALF_OPEN",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CircuitBreakerPolicy {
    pub failure_ratio_threshold: f64,
    pub sample_window: Duration,
    pub minimum_sample_count: u64,
    pub open_duration: Duration,
    pub half_open_trial_count: u64,
}

impl Default for CircuitBreakerPolicy {
    fn default() -> Self {
        Self {
            failure_ratio_threshold: 0.5,
            sample_window: Duration::from_secs(10),
            minimum_sample_count: 10,
            open_duration: Duration::from_secs(60),
            half_open_trial_count: 3,
        }
    }
}

#[derive(Debug, Error)]
pub enum CircuitBreakerError<E> {
    #[error("circuit breaker is open")]
    Open,
    #[error("execution failed: {0}")]
    Execution(E),
}

#[derive(Clone)]
pub struct CircuitBreaker {
    name: String,
    config: CircuitBreakerPolicy,
    inner: Arc<Mutex<CircuitBreakerInner>>,
}

struct CircuitBreakerInner {
    state: CircuitState,
    history: Vec<(Instant, bool)>,
    open_until: Option<Instant>,
    half_open_successes: u64,
    half_open_failures: u64,
}

impl CircuitBreakerInner {
    fn clean_history(&mut self, window: Duration, now: Instant) {
        let cutoff = now.checked_sub(window).unwrap_or(now);
        self.history.retain(|(t, _)| *t >= cutoff);
    }

    fn failure_ratio(&self) -> f64 {
        if self.history.is_empty() {
            return 0.0;
        }
        let failures = self.history.iter().filter(|(_, ok)| !*ok).count();
        failures as f64 / self.history.len() as f64
    }

    fn transition_to(&mut self, name: &str, new_state: CircuitState, failure_ratio: f64) {
        let old_state = self.state;
        self.state = new_state;
        tracing::info!(
            circuit.name = name,
            circuit.state = new_state.as_str(),
            circuit.failure_ratio = failure_ratio,
            "circuit breaker state transition from {:?} to {:?}",
            old_state,
            new_state
        );
    }
}

impl CircuitBreaker {
    pub fn new(name: impl Into<String>, config: CircuitBreakerPolicy) -> Self {
        Self {
            name: name.into(),
            config,
            inner: Arc::new(Mutex::new(CircuitBreakerInner {
                state: CircuitState::Closed,
                history: Vec::new(),
                open_until: None,
                half_open_successes: 0,
                half_open_failures: 0,
            })),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn state(&self) -> CircuitState {
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();
        if inner.state == CircuitState::Open {
            if let Some(until) = inner.open_until {
                if now >= until {
                    inner.transition_to(&self.name, CircuitState::HalfOpen, 1.0);
                    inner.half_open_successes = 0;
                    inner.half_open_failures = 0;
                    inner.open_until = None;
                }
            }
        }
        inner.state
    }

    pub fn config(&self) -> &CircuitBreakerPolicy {
        &self.config
    }

    pub fn failure_ratio(&self) -> f64 {
        let mut inner = self.inner.lock().unwrap();
        inner.clean_history(self.config.sample_window, Instant::now());
        inner.failure_ratio()
    }

    pub(crate) fn before_call(&self) -> Result<(), CircuitBreakerError<()>> {
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();

        if inner.state == CircuitState::Open {
            if let Some(until) = inner.open_until {
                if now >= until {
                    inner.transition_to(&self.name, CircuitState::HalfOpen, 1.0);
                    inner.half_open_successes = 0;
                    inner.half_open_failures = 0;
                    inner.open_until = None;
                }
            }
        }

        if inner.state == CircuitState::Open {
            Err(CircuitBreakerError::Open)
        } else {
            Ok(())
        }
    }

    pub(crate) fn after_call(&self, success: bool) {
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();
        inner.clean_history(self.config.sample_window, now);

        match inner.state {
            CircuitState::Closed => {
                inner.history.push((now, success));
                inner.clean_history(self.config.sample_window, now);

                if inner.history.len() as u64 >= self.config.minimum_sample_count {
                    let ratio = inner.failure_ratio();
                    if ratio >= self.config.failure_ratio_threshold {
                        inner.transition_to(&self.name, CircuitState::Open, ratio);
                        inner.open_until = Some(now + self.config.open_duration);
                    }
                }
            }
            CircuitState::HalfOpen => {
                if success {
                    inner.half_open_successes += 1;
                    if inner.half_open_successes >= self.config.half_open_trial_count {
                        inner.transition_to(&self.name, CircuitState::Closed, 0.0);
                        inner.history.clear();
                    }
                } else {
                    inner.half_open_failures += 1;
                    inner.transition_to(&self.name, CircuitState::Open, 1.0);
                    inner.open_until = Some(now + self.config.open_duration);
                }
            }
            CircuitState::Open => {}
        }
    }

    pub async fn run<F, T, E>(&self, fut: F) -> Result<T, CircuitBreakerError<E>>
    where
        F: Future<Output = Result<T, E>>,
    {
        self.before_call().map_err(|_| CircuitBreakerError::Open)?;

        let res = fut.await;

        match &res {
            Ok(_) => self.after_call(true),
            Err(_) => self.after_call(false),
        }

        res.map_err(CircuitBreakerError::Execution)
    }

    pub async fn run_with_fallback<F, T, E, FB>(&self, fut: F, fallback: FB) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
        FB: FnOnce(CircuitBreakerError<E>) -> Result<T, E>,
    {
        match self.run(fut).await {
            Ok(val) => Ok(val),
            Err(err) => fallback(err),
        }
    }
}

pub struct CircuitBreakerRegistry {
    breakers: Mutex<HashMap<String, CircuitBreaker>>,
}

impl CircuitBreakerRegistry {
    pub fn new() -> Self {
        Self {
            breakers: Mutex::new(HashMap::new()),
        }
    }

    pub fn get_or_create(&self, name: &str, config: CircuitBreakerPolicy) -> CircuitBreaker {
        let mut breakers = self.breakers.lock().unwrap();
        breakers
            .entry(name.to_owned())
            .or_insert_with(|| CircuitBreaker::new(name, config))
            .clone()
    }

    /// Returns a list of all currently registered circuit breakers.
    ///
    /// # Panics
    ///
    /// Panics if the internal registry lock is poisoned.
    pub fn all_breakers(&self) -> Vec<CircuitBreaker> {
        let breakers = self.breakers.lock().unwrap();
        breakers.values().cloned().collect()
    }

    /// Clears all registered circuit breakers from the registry.
    ///
    /// # Panics
    ///
    /// Panics if the internal registry lock is poisoned.
    pub fn clear(&self) {
        let mut breakers = self.breakers.lock().unwrap();
        breakers.clear();
    }
}

static REGISTRY: std::sync::OnceLock<CircuitBreakerRegistry> = std::sync::OnceLock::new();

pub fn global_registry() -> &'static CircuitBreakerRegistry {
    REGISTRY.get_or_init(CircuitBreakerRegistry::new)
}

#[cfg(test)]
pub static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone)]
pub struct CircuitBreakerLayer {
    breaker: CircuitBreaker,
}

impl CircuitBreakerLayer {
    #[must_use]
    pub const fn new(breaker: CircuitBreaker) -> Self {
        Self { breaker }
    }
}

impl<S> tower::Layer<S> for CircuitBreakerLayer {
    type Service = CircuitBreakerService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CircuitBreakerService {
            inner,
            breaker: self.breaker.clone(),
        }
    }
}

#[derive(Clone)]
pub struct CircuitBreakerService<S> {
    inner: S,
    breaker: CircuitBreaker,
}

pin_project_lite::pin_project! {
    #[project = CircuitBreakerServiceFutureProj]
    pub enum CircuitBreakerServiceFuture<F> {
        Executing {
            #[pin]
            fut: F,
            breaker: CircuitBreaker,
        },
        Open,
    }
}

impl<F, T, E> std::future::Future for CircuitBreakerServiceFuture<F>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    type Output = Result<T, CircuitBreakerError<E>>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match self.project() {
            CircuitBreakerServiceFutureProj::Executing { fut, breaker } => match fut.poll(cx) {
                std::task::Poll::Ready(Ok(val)) => {
                    breaker.after_call(true);
                    std::task::Poll::Ready(Ok(val))
                }
                std::task::Poll::Ready(Err(err)) => {
                    breaker.after_call(false);
                    std::task::Poll::Ready(Err(CircuitBreakerError::Execution(err)))
                }
                std::task::Poll::Pending => std::task::Poll::Pending,
            },
            CircuitBreakerServiceFutureProj::Open => {
                std::task::Poll::Ready(Err(CircuitBreakerError::Open))
            }
        }
    }
}

impl<S, Request> tower::Service<Request> for CircuitBreakerService<S>
where
    S: tower::Service<Request>,
{
    type Response = S::Response;
    type Error = CircuitBreakerError<S::Error>;
    type Future = CircuitBreakerServiceFuture<S::Future>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner
            .poll_ready(cx)
            .map_err(CircuitBreakerError::Execution)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        match self.breaker.before_call() {
            Ok(()) => {
                let fut = self.inner.call(req);
                CircuitBreakerServiceFuture::Executing {
                    fut,
                    breaker: self.breaker.clone(),
                }
            }
            Err(_) => CircuitBreakerServiceFuture::Open,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_circuit_breaker_transitions_to_open() {
        let policy = CircuitBreakerPolicy {
            failure_ratio_threshold: 0.5,
            sample_window: Duration::from_secs(10),
            minimum_sample_count: 5,
            open_duration: Duration::from_secs(60),
            half_open_trial_count: 2,
        };
        let breaker = CircuitBreaker::new("test", policy);
        assert_eq!(breaker.state(), CircuitState::Closed);

        // Run 5 failing calls
        for _ in 0..5 {
            let res: Result<(), _> = breaker.run(async { Err("error") }).await;
            assert!(matches!(res, Err(CircuitBreakerError::Execution("error"))));
        }

        // The failure ratio is 100%, and we have 5 samples, so it should trip.
        assert_eq!(breaker.state(), CircuitState::Open);

        // Subsequent calls should fail fast with CircuitBreakerError::Open
        let mut executed = false;
        let res: Result<(), CircuitBreakerError<&'static str>> = breaker
            .run(async {
                executed = true;
                Ok(())
            })
            .await;
        assert!(matches!(res, Err(CircuitBreakerError::Open)));
        assert!(!executed);
    }

    #[tokio::test]
    async fn test_circuit_breaker_tower_service() {
        use tower::{Layer, Service};
        let policy = CircuitBreakerPolicy {
            failure_ratio_threshold: 0.5,
            sample_window: Duration::from_secs(10),
            minimum_sample_count: 5,
            open_duration: Duration::from_secs(60),
            half_open_trial_count: 2,
        };
        let breaker = CircuitBreaker::new("tower_test", policy);

        struct DummyService;
        impl tower::Service<&'static str> for DummyService {
            type Response = &'static str;
            type Error = &'static str;
            type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

            fn poll_ready(
                &mut self,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Result<(), Self::Error>> {
                std::task::Poll::Ready(Ok(()))
            }

            fn call(&mut self, req: &'static str) -> Self::Future {
                if req == "fail" {
                    std::future::ready(Err("failed"))
                } else {
                    std::future::ready(Ok("ok"))
                }
            }
        }

        let mut svc = CircuitBreakerLayer::new(breaker.clone()).layer(DummyService);

        // Run 5 failing calls
        for _ in 0..5 {
            let res = svc.call("fail").await;
            assert!(matches!(res, Err(CircuitBreakerError::Execution("failed"))));
        }

        // Breaker should be Open
        assert_eq!(breaker.state(), CircuitState::Open);

        // Subsequent call should fail fast
        let res = svc.call("ok").await;
        assert!(matches!(res, Err(CircuitBreakerError::Open)));
    }
}
