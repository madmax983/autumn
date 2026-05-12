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
fn repository_api_path_helpers_percent_encode_ids() {
    assert_eq!(__autumn_path_widget_api_get("a/b"), "/api/widgets/a%2Fb");
    assert_eq!(
        __autumn_path_widget_api_update("hello world/é"),
        "/api/widgets/hello%20world%2F%C3%A9"
    );
    assert_eq!(
        __autumn_path_widget_api_delete("a?b#c"),
        "/api/widgets/a%3Fb%23c"
    );
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

#[test]
fn model_impl_open_api_schema_returns_object_type() {
    use autumn_web::openapi::OpenApiSchema;
    let schema = Widget::schema();
    assert_eq!(schema["type"], "object");
}

#[test]
fn model_schema_includes_all_fields_as_properties() {
    use autumn_web::openapi::OpenApiSchema;
    let schema = Widget::schema();
    let props = schema["properties"]
        .as_object()
        .expect("properties must be an object");
    assert!(props.contains_key("id"), "should have id property");
    assert!(props.contains_key("name"), "should have name property");
}

#[test]
fn model_schema_maps_i64_to_integer() {
    use autumn_web::openapi::OpenApiSchema;
    let schema = Widget::schema();
    assert_eq!(schema["properties"]["id"]["type"], "integer");
}

#[test]
fn model_schema_maps_string_to_string_type() {
    use autumn_web::openapi::OpenApiSchema;
    let schema = Widget::schema();
    assert_eq!(schema["properties"]["name"]["type"], "string");
}

#[test]
fn model_schema_lists_non_optional_fields_as_required() {
    use autumn_web::openapi::OpenApiSchema;
    let schema = Widget::schema();
    let required = schema["required"]
        .as_array()
        .expect("required must be an array");
    let req_names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(req_names.contains(&"id"));
    assert!(req_names.contains(&"name"));
}

#[test]
fn new_model_schema_excludes_id_field() {
    use autumn_web::openapi::OpenApiSchema;
    let schema = NewWidget::schema();
    let props = schema["properties"].as_object().expect("properties");
    assert!(!props.contains_key("id"), "NewWidget should not have id");
    assert!(props.contains_key("name"));
}

#[test]
fn update_model_schema_has_no_required_fields() {
    use autumn_web::openapi::OpenApiSchema;
    let schema = UpdateWidget::schema();
    assert!(
        schema["required"].is_null(),
        "UpdateWidget should have no required fields, got: {:?}",
        schema["required"]
    );
}

// Model with Vec<T> and Option<T> fields to exercise array/nullable schema emission.
mod schema2 {
    autumn_web::reexports::diesel::table! {
        tagged_widgets (id) {
            id -> Int8,
            tags -> Array<Text>,
            description -> Nullable<Text>,
        }
    }
}

use schema2::tagged_widgets;

#[autumn_web::model(table = "tagged_widgets")]
pub struct TaggedWidget {
    #[id]
    pub id: i64,
    pub tags: Vec<String>,
    pub description: Option<String>,
}

#[test]
fn vec_field_emits_array_schema() {
    use autumn_web::openapi::OpenApiSchema;
    let schema = TaggedWidget::schema();
    let tags = &schema["properties"]["tags"];
    assert_eq!(tags["type"], "array", "Vec<String> should emit type:array");
    assert_eq!(
        tags["items"]["type"], "string",
        "Vec<String> items should be string"
    );
}

#[test]
fn option_field_emits_nullable_schema() {
    use autumn_web::openapi::OpenApiSchema;
    let schema = TaggedWidget::schema();
    let desc = &schema["properties"]["description"];
    let one_of = desc["oneOf"]
        .as_array()
        .expect("Option<T> should emit oneOf");
    assert_eq!(one_of.len(), 2);
    assert!(
        one_of.iter().any(|v| v["type"] == "null"),
        "oneOf should include null type"
    );
    assert!(
        one_of.iter().any(|v| v["type"] == "string"),
        "oneOf should include string type"
    );
}
