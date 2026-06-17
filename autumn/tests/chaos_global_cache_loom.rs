use autumn_web::cache::{Cache, clear_global_cache, global_cache, set_global_cache};
use loom::thread;
use std::any::Any;
use std::sync::{Arc, Mutex};

struct DummyCache {
    _values: Mutex<std::collections::HashMap<String, Arc<dyn Any + Send + Sync>>>,
}

impl Cache for DummyCache {
    fn get_value(&self, _key: &str) -> Option<Arc<dyn Any + Send + Sync>> {
        None
    }
    fn insert_value(&self, _key: &str, _value: Arc<dyn Any + Send + Sync>) {}
    fn invalidate(&self, _key: &str) {}
    fn clear(&self) {}
}

#[test]
fn global_cache_concurrent_mutations() {
    loom::model(|| {
        clear_global_cache();

        let t1 = thread::spawn(|| {
            let cache = Arc::new(DummyCache {
                _values: Mutex::new(std::collections::HashMap::new()),
            });
            set_global_cache(cache);
        });

        let t2 = thread::spawn(|| {
            let _ = global_cache();
        });

        let t3 = thread::spawn(|| {
            clear_global_cache();
        });

        t1.join().unwrap();
        t2.join().unwrap();
        t3.join().unwrap();
    });
}
