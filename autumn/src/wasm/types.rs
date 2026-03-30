use serde::{Deserialize, Serialize};

/// Metadata for a browser island registered with the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IslandMeta {
    pub name: &'static str,
    pub mount_id: &'static str,
    pub props_type: &'static str,
}

/// Metadata for a generated server action endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionMeta {
    pub name: &'static str,
    pub path: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn island_meta_serializes_expected_fields() {
        let meta = IslandMeta {
            name: "counter",
            mount_id: "counter-root",
            props_type: "CounterProps",
        };

        let json = serde_json::to_value(meta).expect("serialize island meta");

        assert_eq!(json["name"], "counter");
        assert_eq!(json["mount_id"], "counter-root");
        assert_eq!(json["props_type"], "CounterProps");
    }

    #[test]
    fn action_meta_deserializes_from_json() {
        let meta: ActionMeta =
            serde_json::from_str(r#"{"name":"increment","path":"/actions/increment"}"#)
                .expect("deserialize action meta");

        assert_eq!(
            meta,
            ActionMeta {
                name: "increment",
                path: "/actions/increment",
            }
        );
    }
}
