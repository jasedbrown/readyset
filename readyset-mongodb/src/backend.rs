use async_trait::async_trait;
use 


pub struct Backend {
    /// Handle to the backing noria client
    pub noria: readyset_adapter::Backend<MongoDbUpstream, MongoDbQueryHandler>,
    /// Enables logging of statements received from the client. The `Backend` only logs Query,
    /// Prepare and Execute statements.
    pub enable_statement_logging: bool,
}

impl Deref for Backend {
    type Target = readyset_adapter::Backend<MongoDbUpstream, MongoDbQueryHandler>;

    fn deref(&self) -> &Self::Target {
        &self.noria
    }
}

impl DerefMut for Backend {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.noria
    }
}
