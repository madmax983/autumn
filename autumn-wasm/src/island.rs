#[derive(Clone, Copy)]
pub struct IslandRegistration {
    pub name: &'static str,
    pub mount_id: &'static str,
    pub mount: fn(root: crate::Element, props_json: String),
}

impl IslandRegistration {
    #[must_use]
    pub const fn new(
        name: &'static str,
        mount_id: &'static str,
        mount: fn(root: crate::Element, props_json: String),
    ) -> Self {
        Self {
            name,
            mount_id,
            mount,
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static LAST_PROPS: Mutex<Option<String>> = Mutex::new(None);

    fn capture_props(_: crate::Element, props_json: String) {
        *LAST_PROPS.lock().expect("capture lock") = Some(props_json);
    }

    #[test]
    fn constructor_preserves_registration_fields() {
        *LAST_PROPS.lock().expect("capture lock") = None;

        let registration = IslandRegistration::new("counter", "counter-root", capture_props);

        assert_eq!(registration.name, "counter");
        assert_eq!(registration.mount_id, "counter-root");

        (registration.mount)((), "{\"count\":1}".to_owned());

        assert_eq!(
            *LAST_PROPS.lock().expect("capture lock"),
            Some("{\"count\":1}".to_owned())
        );
    }
}
