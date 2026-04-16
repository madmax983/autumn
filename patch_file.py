import sys

def run():
    with open('autumn-harvest/autumn-web-harvest/src/ext.rs', 'r') as f:
        content = f.read()

    # Replacements
    search1 = """    /// Mount the Harvest management API under `path`.
    #[must_use]
    fn harvest_api(self, path: &str) -> Self;
}

impl HarvestExt for AppBuilder {"""
    replace1 = """    /// Mount the Harvest management API under `path`.
    #[must_use]
    fn harvest_api(self, path: &str) -> Self;

    /// Mount the Harvest management API under `path`, protected by the given middleware layer.
    #[must_use]
    fn harvest_api_with_auth<M>(self, path: &str, middleware: M) -> Self
    where
        M: tower::Layer<axum::Router<autumn_web::AppState>> + Clone + Send + Sync + 'static,
        M::Service: tower::Service<axum::extract::Request> + Clone + Send + 'static,
        <M::Service as tower::Service<axum::extract::Request>>::Response: axum::response::IntoResponse + 'static,
        <M::Service as tower::Service<axum::extract::Request>>::Error: Into<std::convert::Infallible> + 'static,
        <M::Service as tower::Service<axum::extract::Request>>::Future: Send + 'static;
}

impl HarvestExt for AppBuilder {"""
    content = content.replace(search1, replace1)

    search2 = """struct HarvestRegistration {
    builder: HarvestBuilder,
    api_path: Option<String>,
}

#[derive(Default)]
struct HarvestIntegrationShared {"""
    replace2 = """type ApiMiddlewareFn = Box<dyn FnOnce(axum::Router<autumn_web::AppState>) -> axum::Router<autumn_web::AppState> + Send + Sync>;

struct HarvestRegistration {
    builder: HarvestBuilder,
    api_path: Option<String>,
    api_middleware: Option<ApiMiddlewareFn>,
}

impl Default for HarvestRegistration {
    fn default() -> Self {
        Self {
            builder: HarvestBuilder::default(),
            api_path: None,
            api_middleware: None,
        }
    }
}

#[derive(Default)]
struct HarvestIntegrationShared {"""
    content = content.replace(search2, replace2)

    search3 = """    fn harvest_api(self, path: &str) -> Self {
        let path = path.to_owned();
        configure_harvest(self, move |registration| {
            registration.api_path = Some(path);
        })
    }
}

fn configure_harvest<F>(builder: AppBuilder, update: F) -> AppBuilder"""
    replace3 = """    fn harvest_api(self, path: &str) -> Self {
        let path = path.to_owned();
        configure_harvest(self, move |registration| {
            registration.api_path = Some(path);
        })
    }

    fn harvest_api_with_auth<M>(self, path: &str, middleware: M) -> Self
    where
        M: tower::Layer<axum::Router<autumn_web::AppState>> + Clone + Send + Sync + 'static,
        M::Service: tower::Service<axum::extract::Request> + Clone + Send + 'static,
        <M::Service as tower::Service<axum::extract::Request>>::Response: axum::response::IntoResponse + 'static,
        <M::Service as tower::Service<axum::extract::Request>>::Error: Into<std::convert::Infallible> + 'static,
        <M::Service as tower::Service<axum::extract::Request>>::Future: Send + 'static,
    {
        let path = path.to_owned();
        configure_harvest(self, move |registration| {
            registration.api_path = Some(path);
            registration.api_middleware = Some(Box::new(move |router| {
                router.layer(middleware)
            }));
        })
    }
}

fn configure_harvest<F>(builder: AppBuilder, update: F) -> AppBuilder"""
    content = content.replace(search3, replace3)

    search4 = """    let mut api_mount = None;
    let builder = builder.update_extension::<HarvestIntegration, _, _>(
        HarvestIntegration::default,
        |integration| {
            {
                let mut shared = integration.shared.lock().expect("harvest lock poisoned");
                update(&mut shared.registration);
                if !integration.api_route_registered {
                    if let Some(path) = shared.registration.api_path.clone() {
                        integration.api_route_registered = true;
                        api_mount = Some((path, integration.api_state.clone()));
                    }
                }
            }

            if !integration.hooks_registered {"""
    replace4 = """    let mut api_mount = None;
    let mut api_middleware = None;
    let builder = builder.update_extension::<HarvestIntegration, _, _>(
        HarvestIntegration::default,
        |integration| {
            {
                let mut shared = integration.shared.lock().expect("harvest lock poisoned");
                update(&mut shared.registration);
                if !integration.api_route_registered {
                    if let Some(path) = shared.registration.api_path.clone() {
                        integration.api_route_registered = true;
                        api_mount = Some((path, integration.api_state.clone()));
                        api_middleware = shared.registration.api_middleware.take();
                    }
                }
            }

            if !integration.hooks_registered {"""
    content = content.replace(search4, replace4)

    search5 = """    if !register_hooks {
        return if let Some((path, api_state)) = api_mount {
            builder.nest(&path, harvest_api_router(api_state))
        } else {
            builder
        };
    }"""
    replace5 = """    if !register_hooks {
        return if let Some((path, api_state)) = api_mount {
            let mut router = harvest_api_router(api_state);
            if let Some(mw) = api_middleware {
                router = mw(router);
            }
            builder.nest(&path, router)
        } else {
            builder
        };
    }"""
    content = content.replace(search5, replace5)

    search6 = """        });

    if let Some((path, api_state)) = api_mount {
        builder.nest(&path, harvest_api_router(api_state))
    } else {
        builder
    }
}"""
    replace6 = """        });

    if let Some((path, api_state)) = api_mount {
        let mut router = harvest_api_router(api_state);
        if let Some(mw) = api_middleware {
            router = mw(router);
        }
        builder.nest(&path, router)
    } else {
        builder
    }
}"""
    content = content.replace(search6, replace6)

    with open('autumn-harvest/autumn-web-harvest/src/ext.rs', 'w') as f:
        f.write(content)

if __name__ == '__main__':
    run()
