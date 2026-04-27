use autumn_web::prelude::*;

#[allow(clippy::unused_async)]
#[get("/res_int")]
async fn res_int() -> Result<i32, autumn_web::error::AutumnError> {
    Ok(42)
}

#[allow(clippy::unused_async)]
#[get("/res_bool")]
async fn res_bool() -> Result<bool, autumn_web::error::AutumnError> {
    Ok(true)
}

#[test]
fn result_primitive_int_return_type_compiles_with_route_macro() {
    let route = __autumn_route_info_res_int();
    assert_eq!(route.path, "/res_int");
}

#[test]
fn result_primitive_bool_return_type_compiles_with_route_macro() {
    let route = __autumn_route_info_res_bool();
    assert_eq!(route.path, "/res_bool");
}
