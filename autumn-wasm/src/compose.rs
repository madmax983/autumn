use crate::signal::Signal;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Registry that allows independently mounted islands to share typed signals.
#[derive(Clone, Default)]
pub struct Composition {
    shared: Rc<RefCell<HashMap<String, Box<dyn Any>>>>,
}

impl Composition {
    /// Create an empty composition registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get an existing named signal or initialize it lazily.
    ///
    /// This is useful when two islands need to coordinate state without a
    /// parent/child relationship.
    #[must_use]
    pub fn signal<T: Clone + 'static>(
        &self,
        key: impl Into<String>,
        init: impl FnOnce() -> T,
    ) -> Signal<T> {
        let key = key.into();
        if let Some(existing) = self
            .shared
            .borrow()
            .get(&key)
            .and_then(|boxed| boxed.downcast_ref::<Signal<T>>())
        {
            return existing.clone();
        }

        let signal = Signal::new(init());
        self.shared
            .borrow_mut()
            .insert(key, Box::new(signal.clone()) as Box<dyn Any>);
        signal
    }

    /// Try to read a previously-initialized named signal.
    #[must_use]
    pub fn get<T: Clone + 'static>(&self, key: &str) -> Option<Signal<T>> {
        self.shared
            .borrow()
            .get(key)
            .and_then(|boxed| boxed.downcast_ref::<Signal<T>>())
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::Composition;

    #[test]
    fn named_signal_returns_same_instance_for_same_type() {
        let composition = Composition::new();
        let left = composition.signal("count", || 1_i32);
        let right = composition.signal("count", || 99_i32);

        left.update(|value| *value += 1);

        assert_eq!(left.get(), 2);
        assert_eq!(right.get(), 2);
    }

    #[test]
    fn get_returns_none_when_type_does_not_match() {
        let composition = Composition::new();
        let _count = composition.signal("shared", || 1_i32);

        assert!(composition.get::<String>("shared").is_none());
    }
}
