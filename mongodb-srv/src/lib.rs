use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

// equivalent to `MySqlShim` and `psql_srv::Backend` traits
#[async_trait]
pub trait MongoDbProxy {
    /// Called when the client issues a request to prepare `query` for later execution.
    ///
    /// The provided [`StatementMetaWriter`](struct.StatementMetaWriter.html) should be used to
    /// notify the client of the statement id assigned to the prepared statement, as well as to
    /// give metadata about the types of parameters and returned columns.
    async fn on_prepare(
        &mut self,
        query: &str,
        // info: StatementMetaWriter<'_, W>,
        // schema_cache: &mut HashMap<u32, CachedSchema>,
    ) -> io::Result<()>;

    /// Provides the server's version information along with ReadySet indications
    fn version(&self) -> String;

    /// Called when the client executes a previously prepared statement.
    ///
    /// Any parameters included with the client's command is given in `params`.
    /// A response to the query should be given using the provided
    /// [`QueryResultWriter`](struct.QueryResultWriter.html).
    async fn on_execute(
        &mut self,
        id: u32,
        // params: ParamParser<'_>,
        results: QueryResultWriter<'_, W>,
        schema_cache: &mut HashMap<u32, CachedSchema>,
    ) -> io::Result<()>;

    /// Called when the client wishes to deallocate resources associated with a previously prepared
    /// statement.
    async fn on_close(&mut self, stmt: u32);

    /// Called when the client issues a query for immediate execution.
    ///
    /// Results should be returned using the given
    /// [`QueryResultWriter`](struct.QueryResultWriter.html).
    async fn on_query(&mut self, query: &str, results: QueryResultWriter<'_, W>) -> io::Result<()>;

    /// Called when client switches database.
    async fn on_init(&mut self, _: &str, _: Option<InitWriter<'_, W>>) -> io::Result<()>;

    /// Retrieve the password for the user with the given username, if any.
    ///
    /// If the user doesn't exist, return [`None`].
    fn password_for_username(&self, username: &str) -> Option<Vec<u8>>;

    /// Return false if password checking should be skipped entirely
    fn require_authentication(&self) -> bool {
        true
    }
}

/// A server that speaks the MongoDB protocol, and can delegate client commands to a backend
/// that implements `MongoDbProxy`.
pub struct MongoDbEmissary<B, R: AsyncRead + Unpin, W: AsyncWrite + Unpin> {
    shim: B,
    // reader: packet::PacketReader<R>,
    // writer: packet::PacketWriter<W>,
    /// Whether to log statements received from a client
    enable_statement_logging: bool,
}
