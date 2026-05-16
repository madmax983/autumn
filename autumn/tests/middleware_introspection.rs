use std::any::TypeId;
use std::borrow::Cow;

use autumn_web::app;
use autumn_web::app::AppBuilder;
use autumn_web::app::Plugin;

#[derive(Clone)]
struct AuthLayer;

impl<S> tower::Layer<S> for AuthLayer {
    type Service = S;

    fn layer(&self, inner: S) -> Self::Service {
        inner
    }
}

#[derive(Clone)]
struct RateLimitLayer;

impl<S> tower::Layer<S> for RateLimitLayer {
    type Service = S;

    fn layer(&self, inner: S) -> Self::Service {
        inner
    }
}

struct RequireAuthPlugin;

impl Plugin for RequireAuthPlugin {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("require-auth-plugin")
    }

    fn build(self, app: AppBuilder) -> AppBuilder {
        assert!(
            app.has_layer::<AuthLayer>(),
            "AuthLayer must be registered before RequireAuthPlugin"
        );
        app
    }
}

#[test]
fn has_layer_detects_presence_and_absence() {
    let with_auth = app().layer(AuthLayer);
    assert!(with_auth.has_layer::<AuthLayer>());
    assert!(!with_auth.has_layer::<RateLimitLayer>());

    let without_auth = app();
    assert!(!without_auth.has_layer::<AuthLayer>());
}

#[test]
fn get_layer_types_returns_registration_order() {
    let builder = app().layer(AuthLayer).layer(RateLimitLayer);

    assert_eq!(
        builder.get_layer_types(),
        vec![TypeId::of::<AuthLayer>(), TypeId::of::<RateLimitLayer>()]
    );
}

#[test]
fn plugin_can_preflight_check_for_required_layer() {
    let builder = app().layer(AuthLayer);
    let _ = builder.plugin(RequireAuthPlugin);
}

#[test]
#[should_panic(expected = "AuthLayer must be registered before RequireAuthPlugin")]
fn plugin_preflight_panics_when_required_layer_is_missing() {
    let _ = app().plugin(RequireAuthPlugin);
}
