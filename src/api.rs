mod base;
mod rest_wrapper_v1;
mod websocket_v1;

pub use rest_wrapper_v1::{rest_api_docs, rest_api_routes};
pub use websocket_v1::websocket_api;
