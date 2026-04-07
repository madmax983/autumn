use autumn_web::channels::Channels;

#[test]
fn test_zero_capacity() {
    let channels = Channels::new(0);
    let _tx = channels.sender("test");
}
