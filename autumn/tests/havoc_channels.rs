use autumn_web::channels::Channels;
use proptest::prelude::*;

proptest! {
    #[test]
    fn channels_capacity_bounds(capacity in 0usize..2) {
        let channels = Channels::new(capacity);
        let _tx = channels.sender("test");
    }
}
