mod base;
// mod rest_wrapper_v1;
mod rest_wrapper_v2;
mod websocket_v1;

// pub use rest_wrapper_v1::{rest_api_docs as rest_api_docs_v1, rest_api_routes as rest_api_routes_v1};
pub use rest_wrapper_v2::{rest_api_docs as rest_api_docs_v2, rest_api_routes as rest_api_routes_v2};
pub use websocket_v1::websocket_api;
