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
