use serde::{Deserialize, Serialize};

/// Metadata for a browser island registered with the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IslandMeta {
    pub name: &'static str,
    pub mount_id: &'static str,
    pub props_type: &'static str,
}

/// Metadata for a generated server action endpoint.
#[derive(Clone, Copy)]
pub struct ActionMeta {
    pub name: &'static str,
    pub path: &'static str,
    pub route: fn() -> crate::route::Route,
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
    fn action_meta_keeps_route_companion() {
        fn fake_route() -> crate::route::Route {
            crate::route::Route {
                method: http::Method::POST,
                path: "/_autumn/actions/increment",
                handler: axum::routing::post(|| async { "ok" }),
                name: "increment",
            }
        }

        let meta = ActionMeta {
            name: "increment",
            path: "/_autumn/actions/increment",
            route: fake_route,
        };

        let route = (meta.route)();
        assert_eq!(route.path, "/_autumn/actions/increment");
        assert_eq!(route.name, "increment");
    }
}
