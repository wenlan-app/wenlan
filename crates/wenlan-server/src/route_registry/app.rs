// SPDX-License-Identifier: Apache-2.0
use axum::body::Body;
use axum::http::Request;
use axum::response::Response;
use axum::routing::IntoMakeService;
use axum::Router;
use std::convert::Infallible;
use std::task::{Context, Poll};
use tower::Service;

#[derive(Clone)]
/// Finalized HTTP service with no route-composition surface.
///
/// ```compile_fail,E0599
/// use std::sync::Arc;
/// use tokio::sync::RwLock;
/// use wenlan_server::{router::build_router, state::ServerState};
/// let state = Arc::new(RwLock::new(ServerState::default()));
/// let app = build_router(state);
/// let method: axum::routing::MethodRouter<()> = axum::routing::get(|| async {});
/// app.route("/forbidden", method);
/// ```
///
/// ```compile_fail,E0599
/// use std::sync::Arc;
/// use tokio::sync::RwLock;
/// use wenlan_server::{router::build_router, state::ServerState};
/// let state = Arc::new(RwLock::new(ServerState::default()));
/// let app = build_router(state);
/// app.nest("/forbidden", axum::Router::<()>::new());
/// ```
///
/// ```compile_fail,E0599
/// use std::sync::Arc;
/// use tokio::sync::RwLock;
/// use wenlan_server::{router::build_router, state::ServerState};
/// let state = Arc::new(RwLock::new(ServerState::default()));
/// let app = build_router(state);
/// app.merge(axum::Router::<()>::new());
/// ```
///
/// ```compile_fail,E0599
/// use std::convert::Infallible;
/// use std::sync::Arc;
/// use tokio::sync::RwLock;
/// use wenlan_server::{router::build_router, state::ServerState};
/// let state = Arc::new(RwLock::new(ServerState::default()));
/// let app = build_router(state);
/// let service = tower::service_fn(|_: axum::http::Request<axum::body::Body>| async {
///     Ok::<_, Infallible>(axum::http::Response::new(axum::body::Body::empty()))
/// });
/// app.route_service("/forbidden", service);
/// ```
pub struct AppRouter {
    inner: Router,
}

impl AppRouter {
    pub(super) const fn new(inner: Router) -> Self {
        Self { inner }
    }

    #[cfg(test)]
    pub(crate) fn from_test_router(inner: Router) -> Self {
        Self::new(inner)
    }

    pub fn into_make_service(self) -> IntoMakeService<Router> {
        self.inner.into_make_service()
    }
}

impl Service<Request<Body>> for AppRouter {
    type Response = Response;
    type Error = Infallible;
    type Future = <Router as Service<Request<Body>>>::Future;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        <Router as Service<Request<Body>>>::poll_ready(&mut self.inner, context)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        self.inner.call(request)
    }
}
