use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

type SubscriberId = usize;
type SubscriberCallback<T> = Rc<dyn Fn(&T)>;
type Subscribers<T> = Vec<(SubscriberId, SubscriberCallback<T>)>;

/// Reactive local state container for browser islands.
///
/// `Signal` is intentionally tiny and framework-agnostic. It allows
/// multiple islands to observe and update shared state without introducing a
/// rendering runtime.
#[derive(Clone)]
pub struct Signal<T> {
    inner: Rc<SignalInner<T>>,
}

struct SignalInner<T> {
    value: RefCell<T>,
    next_id: Cell<usize>,
    subscribers: RefCell<Subscribers<T>>,
    version: Cell<u64>,
    notifying: Cell<bool>,
    needs_flush: Cell<bool>,
}

struct NotifyingGuard<'a> {
    notifying: &'a Cell<bool>,
}

impl Drop for NotifyingGuard<'_> {
    fn drop(&mut self) {
        self.notifying.set(false);
    }
}

impl<T> Signal<T> {
    /// Create a new signal with the initial value.
    #[must_use]
    pub fn new(initial: T) -> Self {
        Self {
            inner: Rc::new(SignalInner {
                value: RefCell::new(initial),
                next_id: Cell::new(0),
                subscribers: RefCell::new(Vec::new()),
                version: Cell::new(0),
                notifying: Cell::new(false),
                needs_flush: Cell::new(false),
            }),
        }
    }

    /// Subscribe to value updates.
    ///
    /// The callback is called every time the signal value changes.
    #[must_use]
    pub fn subscribe(&self, callback: impl Fn(&T) + 'static) -> Subscription<T> {
        let id = self.inner.next_id.get();
        self.inner.next_id.set(id.saturating_add(1));
        self.inner
            .subscribers
            .borrow_mut()
            .push((id, Rc::new(callback)));

        Subscription {
            inner: Rc::downgrade(&self.inner),
            id,
        }
    }
}

impl<T: Clone> Signal<T> {
    fn mark_updated(&self) {
        let next = self.inner.version.get().wrapping_add(1);
        self.inner.version.set(next);
    }

    fn notify_subscribers(&self) {
        if self.inner.notifying.get() {
            self.inner.needs_flush.set(true);
            return;
        }

        self.inner.notifying.set(true);
        let _guard = NotifyingGuard {
            notifying: &self.inner.notifying,
        };
        loop {
            self.inner.needs_flush.set(false);
            let cycle_version = self.inner.version.get();
            let callbacks = self
                .inner
                .subscribers
                .borrow()
                .iter()
                .map(|(_, callback)| Rc::clone(callback))
                .collect::<Vec<_>>();
            let value = self.get();

            for callback in callbacks {
                if self.inner.version.get() != cycle_version {
                    break;
                }
                callback(&value);
            }

            let has_newer_value = self.inner.version.get() != cycle_version;
            if !has_newer_value && !self.inner.needs_flush.get() {
                break;
            }
        }
    }

    /// Read the current value.
    #[must_use]
    pub fn get(&self) -> T {
        self.inner.value.borrow().clone()
    }

    /// Set a new value and notify subscribers.
    pub fn set(&self, next: T) {
        *self.inner.value.borrow_mut() = next;
        self.mark_updated();
        self.notify_subscribers();
    }

    /// Update the current value in place and notify subscribers.
    pub fn update(&self, updater: impl FnOnce(&mut T)) {
        {
            let mut value = self.inner.value.borrow_mut();
            updater(&mut value);
        }
        self.mark_updated();
        self.notify_subscribers();
    }
}

/// Guard type that keeps a signal subscription active.
///
/// Dropping this value unsubscribes from future updates.
pub struct Subscription<T> {
    inner: Weak<SignalInner<T>>,
    id: SubscriberId,
}

impl<T> Drop for Subscription<T> {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner
                .subscribers
                .borrow_mut()
                .retain(|(id, _)| *id != self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Signal;
    use std::cell::RefCell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;

    #[test]
    fn signal_notifies_subscribers_on_set_and_update() {
        let signal = Signal::new(1_i32);
        let observed = Rc::new(RefCell::new(Vec::new()));
        let observed_clone = Rc::clone(&observed);

        let _sub = signal.subscribe(move |value| {
            observed_clone.borrow_mut().push(*value);
        });

        signal.set(2);
        signal.update(|value| *value += 3);

        assert_eq!(signal.get(), 5);
        assert_eq!(*observed.borrow(), vec![2, 5]);
    }

    #[test]
    fn dropping_subscription_stops_notifications() {
        let signal = Signal::new(10_i32);
        let observed = Rc::new(RefCell::new(Vec::new()));
        let observed_clone = Rc::clone(&observed);

        let sub = signal.subscribe(move |value| observed_clone.borrow_mut().push(*value));
        signal.set(11);
        drop(sub);
        signal.set(12);

        assert_eq!(*observed.borrow(), vec![11]);
    }

    #[test]
    fn callback_can_drop_another_subscription_without_panicking() {
        let signal = Signal::new(0_i32);
        let dropped = Rc::new(RefCell::new(false));
        let drop_slot = Rc::new(RefCell::new(None));

        let dropped_clone = Rc::clone(&dropped);
        let tracked = signal.subscribe(move |_| {
            *dropped_clone.borrow_mut() = true;
        });
        *drop_slot.borrow_mut() = Some(tracked);

        let drop_slot_clone = Rc::clone(&drop_slot);
        let _dropper = signal.subscribe(move |_| {
            drop_slot_clone.borrow_mut().take();
        });

        signal.set(1);
        assert!(*dropped.borrow());

        *dropped.borrow_mut() = false;
        signal.set(2);
        assert!(!*dropped.borrow());
    }

    #[test]
    fn reentrant_update_does_not_deliver_stale_value_to_later_subscribers() {
        let signal = Signal::new(0_i32);
        let seen = Rc::new(RefCell::new(Vec::new()));

        let signal_for_reentrant = signal.clone();
        let _first = signal.subscribe(move |value| {
            if *value == 1 {
                signal_for_reentrant.set(2);
            }
        });

        let seen_clone = Rc::clone(&seen);
        let _second = signal.subscribe(move |value| {
            seen_clone.borrow_mut().push(*value);
        });

        signal.set(1);

        assert_eq!(*seen.borrow(), vec![2]);
    }

    #[test]
    fn panic_in_callback_does_not_brick_future_notifications() {
        let signal = Signal::new(0_i32);
        let panicker = signal.subscribe(|_| panic!("boom"));

        let result = catch_unwind(AssertUnwindSafe(|| {
            signal.set(1);
        }));
        assert!(result.is_err());
        drop(panicker);

        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen_clone = Rc::clone(&seen);
        let _healthy = signal.subscribe(move |value| seen_clone.borrow_mut().push(*value));

        signal.set(2);
        assert_eq!(*seen.borrow(), vec![2]);
    }
}
