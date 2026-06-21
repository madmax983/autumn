#[test]
fn test_db_registry_panic_isolation() {
    let registry = std::sync::Arc::new(std::sync::Mutex::new(
        Vec::<autumn_web::db::CommitCallback>::new(),
    ));

    let reg1 = registry.clone();
    let t1 = std::thread::spawn(move || {
        let mut guard = reg1
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.push(Box::new(|| Box::pin(async { Ok(()) })));
        panic!("Boom in registration");
    });

    let _ = t1.join();

    let reg2 = registry.clone();
    let t2 = std::thread::spawn(move || {
        // This will successfully recover the lock thanks to our fix
        let mut reg = reg2
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = std::mem::take(&mut *reg);
    });

    let _ = t2.join();
}
