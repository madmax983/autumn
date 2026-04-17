//! Internal test utilities.

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::thread;

    #[test]
    fn test_env_isolation_no_data_races() {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let mut threads = vec![];

        for i in 0..10 {
            let errors_clone = errors.clone();
            threads.push(thread::spawn(move || {
                let value = format!("value_{}", i);
                let expected = value.clone();

                temp_env::with_vars([("AUTUMN_ISOLATION_TEST", Some(&value))], || {
                    let actual = std::env::var("AUTUMN_ISOLATION_TEST").unwrap_or_default();
                    if actual != expected {
                        errors_clone.lock().unwrap().push(format!("Thread {} expected {} but got {}", i, expected, actual));
                    }

                    // Add a small delay to increase chance of interleaving
                    std::thread::sleep(std::time::Duration::from_millis(5));

                    let actual_after = std::env::var("AUTUMN_ISOLATION_TEST").unwrap_or_default();
                    if actual_after != expected {
                        errors_clone.lock().unwrap().push(format!("Thread {} expected {} but got {} after sleep", i, expected, actual_after));
                    }
                });
            }));
        }

        for t in threads {
            t.join().unwrap();
        }

        let final_errors = errors.lock().unwrap();
        assert!(final_errors.is_empty(), "Data races detected: {:?}", *final_errors);
    }
}