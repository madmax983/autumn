#![cfg(feature = "ws")]

use autumn_web::channels::Channels;
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_channels_capacity_fuzzing(capacity in any::<usize>()) {
        let channels = Channels::new(capacity);

        // This should never panic on any capacity (see explicit zero test)
        let _tx = channels.sender("test_channel");
        let _rx = channels.subscribe("test_channel");
    }
}

#[test]
fn test_channels_zero_capacity_regression() {
    let channels = Channels::new(0);

    // This should never panic even if capacity is 0 (which was the bug)
    let _tx = channels.sender("test_channel");
    let _rx = channels.subscribe("test_channel");
}
