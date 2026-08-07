use std::{future::Future, time::Duration};

/// Result of one retained worker iteration and whether shutdown won the race.
#[derive(Debug)]
pub struct IterationOutcome<T> {
    inner: Option<T>,
    shutdown_requested: bool,
}

impl<T> IterationOutcome<T> {
    #[must_use]
    pub const fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    #[must_use]
    pub fn into_inner(self) -> Option<T> {
        self.inner
    }
}

/// Stops the outer lease loop when shutdown arrives but retains and awaits the
/// exact in-flight iteration so its jobs cannot be cancelled or detached.
pub async fn await_iteration_or_drain<I, S, T>(iteration: I, shutdown: S) -> IterationOutcome<T>
where
    I: Future<Output = T>,
    S: Future<Output = ()>,
{
    tokio::pin!(shutdown);
    tokio::select! {
        biased;
        () = &mut shutdown => return IterationOutcome {
            inner: None,
            shutdown_requested: true,
        },
        () = tokio::task::yield_now() => {}
    }

    tokio::pin!(iteration);
    tokio::select! {
        biased;
        inner = &mut iteration => IterationOutcome {
            inner: Some(inner),
            shutdown_requested: false,
        },
        () = &mut shutdown => IterationOutcome {
            inner: Some(iteration.await),
            shutdown_requested: true,
        },
    }
}

/// Runs operational sampling independently of the worker's in-flight batch.
#[must_use]
pub fn spawn_periodic_reporter<F, R>(period: Duration, mut report: F) -> tokio::task::JoinHandle<()>
where
    F: FnMut() -> R + Send + 'static,
    R: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            report().await;
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::await_iteration_or_drain;

    #[tokio::test]
    async fn shutdown_ready_before_an_iteration_prevents_new_work() {
        let started = Arc::new(AtomicBool::new(false));
        let iteration_started = Arc::clone(&started);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        assert!(shutdown_tx.send(()).is_ok());

        let outcome = await_iteration_or_drain(
            async move {
                iteration_started.store(true, Ordering::SeqCst);
            },
            async {
                let _ = shutdown_rx.await;
            },
        )
        .await;

        assert!(outcome.shutdown_requested());
        assert!(outcome.into_inner().is_none());
        assert!(!started.load(Ordering::SeqCst));
    }
}
