#[derive(Clone)]
struct Channels;

fn main() {
    let channels = Channels;
    let _cloned = channels.clone();
    drop(_cloned);
}
