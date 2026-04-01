use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

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
    subscribers: RefCell<HashMap<usize, Box<dyn Fn(&T)>>>,
}

impl<T> Signal<T> {
    /// Create a new signal with the initial value.
    #[must_use]
    pub fn new(initial: T) -> Self {
        Self {
            inner: Rc::new(SignalInner {
                value: RefCell::new(initial),
                next_id: Cell::new(0),
                subscribers: RefCell::new(HashMap::new()),
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
            .insert(id, Box::new(callback));

        Subscription {
            inner: Rc::downgrade(&self.inner),
            id,
        }
    }

    fn notify_subscribers(&self) {
        let value = self.inner.value.borrow();
        let subscribers = self.inner.subscribers.borrow();
        for callback in subscribers.values() {
            callback(&value);
        }
    }
}

impl<T: Clone> Signal<T> {
    /// Read the current value.
    #[must_use]
    pub fn get(&self) -> T {
        self.inner.value.borrow().clone()
    }

    /// Set a new value and notify subscribers.
    pub fn set(&self, next: T) {
        *self.inner.value.borrow_mut() = next;
        self.notify_subscribers();
    }

    /// Update the current value in place and notify subscribers.
    pub fn update(&self, updater: impl FnOnce(&mut T)) {
        {
            let mut value = self.inner.value.borrow_mut();
            updater(&mut value);
        }
        self.notify_subscribers();
    }
}

/// Guard type that keeps a signal subscription active.
///
/// Dropping this value unsubscribes from future updates.
pub struct Subscription<T> {
    inner: Weak<SignalInner<T>>,
    id: usize,
}

impl<T> Drop for Subscription<T> {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner.subscribers.borrow_mut().remove(&self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Signal;
    use std::cell::RefCell;
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
}
