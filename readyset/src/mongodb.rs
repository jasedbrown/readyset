use async_trait::async_trait;
use tokio::net::TcpStream;

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
        stream: TcpStream,
        backend: readyset_adapter::Backend<MongoDbUpstream, MongoDbQueryHandler>,
    ) {
        // TODO(jeb): impl me!
    }

    async fn immediate_error(self, stream: TcpStream, error_message: String) {
        // TODO(jeb) impl me!
    }
}