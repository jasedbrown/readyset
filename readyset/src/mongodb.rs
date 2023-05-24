use async_trait::async_trait;
use readyset_mongodb::{MongoDbQueryHandler, MongoDbUpstream};
use tokio::net::TcpStream;
use tracing::info;

use crate::ConnectionHandler;

#[derive(Clone, Copy)]
pub struct MongoDbHandler {
    pub enable_statement_logging: bool,
}

#[async_trait]
impl ConnectionHandler for MongoDbHandler {
    type UpstreamDatabase = MongoDbUpstream;
    type Handler = MongoDbQueryHandler;

    async fn process_connection(
        &mut self,
        _stream: TcpStream,
        _backend: readyset_adapter::Backend<MongoDbUpstream, MongoDbQueryHandler>,
    ) {
        info!("JEB::MongoDbHandler::process_connection - 1")
        // TODO(jeb): impl me!
    }

    async fn immediate_error(self, _stream: TcpStream, _error_message: String) {
        info!("JEB::MongoDbHandler::immediate_error - 1")
        // TODO(jeb) impl me!
    }
}