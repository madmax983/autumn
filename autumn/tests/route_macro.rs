use autumn::get;

#[get("/hello")]
async fn hello() -> &'static str {
    "Hello, Autumn!"
}

#[get("/")]
async fn index() -> &'static str {
    "root"
}

#[get("/with/nested/path")]
async fn nested() -> &'static str {
    "nested"
}

#[test]
fn hello_route_info_has_correct_method() {
    let route = __autumn_route_info_hello();
    assert_eq!(route.method, http::Method::GET);
}

#[test]
fn hello_route_info_has_correct_path() {
    let route = __autumn_route_info_hello();
    assert_eq!(route.path, "/hello");
}

#[test]
fn hello_route_info_has_correct_name() {
    let route = __autumn_route_info_hello();
    assert_eq!(route.name, "hello");
}

#[test]
fn index_route_info_has_correct_fields() {
    let route = __autumn_route_info_index();
    assert_eq!(route.method, http::Method::GET);
    assert_eq!(route.path, "/");
    assert_eq!(route.name, "index");
}

#[test]
fn nested_route_info_has_correct_path() {
    let route = __autumn_route_info_nested();
    assert_eq!(route.path, "/with/nested/path");
}
