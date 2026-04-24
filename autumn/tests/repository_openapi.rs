//! Integration tests for `#[repository]` CRUD routes' `OpenAPI` metadata.
//!
//! The repository macro auto-generates five HTTP handlers (list, get,
//! create, update, delete) when `api = "/path"` is supplied. This test
//! pins their `ApiDoc` shape so the generated spec accurately reflects
//! the JSON request/response bodies that Autumn mounts.

#![cfg(all(feature = "db", feature = "openapi"))]

use autumn_web::openapi::SchemaKind;

mod schema {
    autumn_web::reexports::diesel::table! {
        widgets (id) {
            id -> Int8,
            name -> Text,
        }
    }
}

use schema::widgets;

#[autumn_web::model]
pub struct Widget {
    #[id]
    pub id: i64,
    pub name: String,
}

#[autumn_web::repository(Widget, api = "/api/widgets")]
pub trait WidgetRepository {}

#[test]
fn list_route_returns_array_of_widget() {
    let route = __autumn_route_info_widget_api_list();
    assert_eq!(route.api_doc.method, "GET");
    assert_eq!(route.api_doc.path, "/api/widgets");
    assert_eq!(route.api_doc.success_status, 200);
    let resp = route
        .api_doc
        .response
        .as_ref()
        .expect("list must document its JSON response");
    let inner = match resp.kind {
        SchemaKind::Array(e) => e,
        other => panic!("list should emit Array, got {other:?}"),
    };
    assert_eq!(inner.name, "Widget");
    assert_eq!(inner.kind, SchemaKind::Ref);
}

#[test]
fn get_route_returns_single_widget_ref() {
    let route = __autumn_route_info_widget_api_get();
    assert_eq!(route.api_doc.method, "GET");
    assert_eq!(route.api_doc.path_params, &["id"]);
    let resp = route.api_doc.response.as_ref().expect("get response");
    assert_eq!(resp.name, "Widget");
    assert_eq!(resp.kind, SchemaKind::Ref);
    assert!(route.api_doc.request_body.is_none());
}

#[test]
fn create_route_takes_new_widget_returns_widget() {
    let route = __autumn_route_info_widget_api_create();
    assert_eq!(route.api_doc.method, "POST");
    assert_eq!(route.api_doc.success_status, 201);
    let body = route
        .api_doc
        .request_body
        .as_ref()
        .expect("create must document a request body");
    assert_eq!(body.name, "NewWidget");
    assert_eq!(body.kind, SchemaKind::Ref);
    let resp = route.api_doc.response.as_ref().expect("create response");
    assert_eq!(resp.name, "Widget");
}

#[test]
fn update_route_takes_update_widget_and_id() {
    let route = __autumn_route_info_widget_api_update();
    assert_eq!(route.api_doc.method, "PUT");
    assert_eq!(route.api_doc.path_params, &["id"]);
    let body = route
        .api_doc
        .request_body
        .as_ref()
        .expect("update must document a request body");
    assert_eq!(body.name, "UpdateWidget");
    let resp = route.api_doc.response.as_ref().expect("update response");
    assert_eq!(resp.name, "Widget");
}

#[test]
fn delete_route_has_no_body_and_uses_204() {
    let route = __autumn_route_info_widget_api_delete();
    assert_eq!(route.api_doc.method, "DELETE");
    assert_eq!(route.api_doc.path_params, &["id"]);
    assert_eq!(route.api_doc.success_status, 204);
    assert!(route.api_doc.request_body.is_none());
    assert!(route.api_doc.response.is_none());
}
