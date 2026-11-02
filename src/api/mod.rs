pub mod client;
mod event_hub;
pub mod schema;
mod server;
mod status;
mod subscriptions;
mod wait;

pub use event_hub::EventHub;
pub use server::ServerHandle;
pub(crate) use server::{api_method_name, start_server_with_stop_control};
pub use status::{read_runtime_status_at, RuntimeStatus};

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::api::schema::Request;

pub const SOCKET_PATH_ENV_VAR: &str = "HERDR_SOCKET_PATH";

pub(crate) fn request_changes_ui(request: &Request) -> bool {
    request.method.operation().changes_ui || ledger_read_acknowledges(&request.method)
}

/// andreconde fork (sheprd): reading the ledger changes it only when the read acknowledges
/// answers; a plain `card list` or `card answers --peek` is read-only (SHE-100009).
fn ledger_read_acknowledges(method: &schema::Method) -> bool {
    use schema::Method;
    match method {
        Method::LedgerList(params) => params.ack,
        Method::LedgerAnswers(params) => !params.peek,
        _ => false,
    }
}

pub struct ApiRequestMessage {
    pub request: Request,
    pub respond_to: std::sync::mpsc::Sender<String>,
    pub response_write_complete: Option<std::sync::mpsc::Receiver<()>>,
}

pub type ApiRequestSender = mpsc::UnboundedSender<ApiRequestMessage>;

pub fn socket_path() -> PathBuf {
    crate::session::active_api_socket_path()
}
