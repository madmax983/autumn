use autumn_web::channels::Channels;

#[test]
fn test_oom_capacity() {
    let channels = Channels::new(usize::MAX);
    let _tx = channels.sender("test");
}
