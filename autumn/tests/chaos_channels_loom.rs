#![cfg(feature = "ws")]
use autumn_web::channels::Channels;
use loom::sync::Arc;
use loom::thread;

#[test]
fn channels_concurrent_sender_subscribe_loom() {
    loom::model(|| {
        let channels = Channels::new(32);
        let channels = Arc::new(channels);

        let c1 = channels.clone();
        let t1 = thread::spawn(move || {
            let tx = c1.sender("test");
            let _ = tx.send("hello");
        });

        let c2 = channels;
        let t2 = thread::spawn(move || {
            let _rx = c2.subscribe("test");
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}
