//! `WebPush::Pool` (reference/lib/web_push/pool.rb) with the invalid-subscription handler from
//! reference/config/initializers/web_push.rb: up to 50 deliveries at once and 10,000 waiting
//! (more are dropped, like `Concurrent::RejectedExecutionError`, and logged), and one worker that
//! destroys expired or unusable subscriptions in order.

use std::fmt::Display;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use ember_db::{Connection, PushPayload, PushSubscription};
use tokio::sync::Semaphore;

use super::{Notification, VapidConfig};
use crate::integrations::net::Network;

/// `Concurrent::ThreadPoolExecutor.new(max_threads: 50, max_queue: 10000)`
const MAX_THREADS: usize = 50;
const MAX_QUEUE: usize = 10_000;
/// Deliveries running or waiting.
const QUEUE_SLOTS: usize = MAX_THREADS + MAX_QUEUE;

type Handler = Box<dyn Fn(i64) -> Result<(), String> + Send>;

#[derive(Clone)]
pub struct Pool {
    inner: Arc<Inner>,
}

struct Inner {
    net: Network,
    vapid: VapidConfig,
    runtime: tokio::runtime::Handle,
    running: Semaphore,
    /// A delivery holds one of these until it finishes, or unwinds.
    slots: Arc<Semaphore>,
    /// Deliveries dropped since the queue was last accepting them.
    dropped: AtomicUsize,
    shut_down: AtomicBool,
    invalidations: Mutex<Option<mpsc::Sender<i64>>>,
    invalidator: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Pool {
    /// Call from inside the Tokio runtime deliveries should run on. `invalid_subscription_handler`
    /// is `Push::Subscription.find_by(id:)&.destroy`; it runs on the pool's own thread, so it may
    /// block (e.g. `Database::write_blocking`).
    pub fn new<F, E>(net: Network, vapid: VapidConfig, invalid_subscription_handler: F) -> Self
    where
        F: Fn(i64) -> Result<(), E> + Send + 'static,
        E: Display,
    {
        let handler: Handler = Box::new(move |id| invalid_subscription_handler(id).map_err(|e| e.to_string()));
        let (sender, receiver) = mpsc::channel::<i64>();
        let invalidator = std::thread::Builder::new()
            .name("web_push-invalidation".into())
            .spawn(move || {
                for id in receiver {
                    tracing::info!("Destroying push subscription: {id}");
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(id))) {
                        Ok(Ok(())) => {}
                        Ok(Err(message)) => {
                            tracing::error!("Error in WebPush::Pool.invalid_subscription_handler: {message}")
                        }
                        Err(_) => tracing::error!("Error in WebPush::Pool.invalid_subscription_handler: panic"),
                    }
                }
            })
            .expect("spawn the web push invalidation thread");

        Self {
            inner: Arc::new(Inner {
                net,
                vapid,
                runtime: tokio::runtime::Handle::current(),
                running: Semaphore::new(MAX_THREADS),
                slots: Arc::new(Semaphore::new(QUEUE_SLOTS)),
                dropped: AtomicUsize::new(0),
                shut_down: AtomicBool::new(false),
                invalidations: Mutex::new(Some(sender)),
                invalidator: Mutex::new(Some(invalidator)),
            }),
        }
    }

    /// `queue(payload, subscriptions)`: in id order (`find_each`), each subscription's
    /// notification is built here (counting its badge) and delivered on the pool.
    pub fn queue(&self, conn: &Connection, payload: &PushPayload, mut subscriptions: Vec<PushSubscription>) -> ember_db::Result<()> {
        subscriptions.sort_by_key(|s| s.id);
        for subscription in &subscriptions {
            self.deliver_later(Notification::build(conn, subscription, payload)?);
        }
        Ok(())
    }

    pub fn deliver_later(&self, notification: Notification) {
        let inner = &self.inner;
        if inner.shut_down.load(Ordering::SeqCst) {
            tracing::warn!("WebPush::Pool is shut down, dropping a notification");
            return;
        }
        let Ok(slot) = inner.slots.clone().try_acquire_owned() else {
            if inner.dropped.fetch_add(1, Ordering::SeqCst) == 0 {
                tracing::error!("WebPush::Pool is full, dropping notifications");
            }
            return;
        };
        let dropped = inner.dropped.swap(0, Ordering::SeqCst);
        if dropped > 0 {
            tracing::error!("WebPush::Pool dropped {dropped} notifications while it was full");
        }
        let pool = self.inner.clone();
        inner.runtime.spawn(async move {
            // Released when the delivery finishes, or when Tokio drops the task after a panic.
            let _slot = slot;
            if let Ok(_running) = pool.running.acquire().await {
                pool.deliver(&notification).await;
            }
        });
    }

    /// Waits (up to a second, like `wait_for_termination(1)`) for queued deliveries, then stops
    /// the invalidation worker once it has drained.
    pub async fn shutdown(&self) {
        let inner = &self.inner;
        inner.shut_down.store(true, Ordering::SeqCst);
        // Every slot free means nothing is queued or running.
        let _ = tokio::time::timeout(Duration::from_secs(1), inner.slots.acquire_many(QUEUE_SLOTS as u32)).await;
        inner.invalidations.lock().unwrap().take();
        let worker = inner.invalidator.lock().unwrap().take();
        if let Some(worker) = worker {
            let _ = tokio::task::spawn_blocking(move || worker.join()).await;
        }
    }

    pub fn vapid(&self) -> &VapidConfig {
        &self.inner.vapid
    }

    /// Queued or running deliveries.
    #[cfg(test)]
    pub fn pending(&self) -> usize {
        QUEUE_SLOTS - self.inner.slots.available_permits()
    }
}

impl Inner {
    async fn deliver(&self, notification: &Notification) {
        match notification.deliver(&self.net, &self.vapid).await {
            Ok(_) => {}
            Err(error) if error.invalidates_subscription() => self.invalidate_subscription_later(notification.subscription.id),
            Err(error) => tracing::error!("Error in WebPush::Pool.deliver: {} {}", error.class_name(), error),
        }
    }

    fn invalidate_subscription_later(&self, id: i64) {
        if let Some(sender) = self.invalidations.lock().unwrap().as_ref() {
            let _ = sender.send(id);
        }
    }
}
