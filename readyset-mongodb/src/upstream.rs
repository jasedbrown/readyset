use async_trait::async_trait;
use mongodb::{Client, options::ClientOptions};
use nom_sql::SqlIdentifier;
use readyset_adapter::{UpstreamConfig, UpstreamDatabase, UpstreamPrepare};
use readyset_adapter::fallback_cache::FallbackCache;
use readyset_adapter::upstream_database::UpstreamDestination;
use readyset_client_metrics::QueryDestination;
use readyset_data::DfValue;
use readyset_errors::{unsupported_err, ReadySetError};
use tracing::info;

use crate::Error;

#[derive(Debug)]
pub enum QueryResult {
    WriteResult {
        num_rows_affected: u64,
        last_inserted_id: u64,
        // TODO(jeb) annotate with real mongodb error codes
        status_flags: u32,
    },
    ReadResult {
        // stream: ReadResultStream<'a>,
        // columns: Arc<[Column]>,
    },
    Command {
        // TODO(jeb) annotate with real mongodb error codes
        status_flags: u32,
    },
}

impl UpstreamDestination for QueryResult {
    fn destination(&self) -> QueryDestination {
        QueryDestination::Upstream
    }
}

// A connector to the underlying mongodb instance/replset.
pub struct MongoDbUpstream {
    _client: Client,
    upstream_config: UpstreamConfig,
}

#[async_trait]
impl UpstreamDatabase for MongoDbUpstream {
    type QueryResult<'a> = QueryResult;

    // only used with FallbackCache, which is currently only impl'd for MySQL,
    // PG not using it. it's not essential for my needs, so punting ...
    type CachedReadResult = ();

    // this is metadata for prepared statements. As I'm faking the funk with mongo,
    // trying to get by without this :shrug:
    type StatementMeta = ();

    type Error = Error;

    const DEFAULT_DB_VERSION: &'static str = "6.0.5-readyset\0";

    // Create a new connection to the unlying MongoDB instance/replset. IN the case
    // of a replset, the driver will hanlde all the magic of connecting to th
    // primary and the secondaries.
    async fn connect(
        upstream_config: UpstreamConfig,
        _: Option<FallbackCache<Self::CachedReadResult>>,
    ) -> Result<Self, Self::Error> {
        let url = upstream_config
            .upstream_db_url
            .as_deref()
            .ok_or(ReadySetError::InvalidUpstreamDatabase)?;


        // options to driver
        let mut client_options = ClientOptions::parse(url).await?;
        client_options.app_name = Some("readyset".to_string());

        // TODO(jeb) add tls here ... punting for now

        // TODO(jeb) do the span logging!
        // let span = info_span!(
        //     "Connecting to MongoDB upstream",
        //     host = ,
        //     port = ,
        //     user = ,
        // );
        // span.in_scope(|| info!("Establishing connection"));

        // actually connect to Mongo here. fwiw, there's no async connect function 
        // on Client that also takes a client_options struct :shrug:

        let client = Client::with_options(client_options)?;
        info!("MongoDbUpstream::connect() - connected to Client, url: {}", &url);

        // span.in_scope(|| info!("Established connection to upstream"));

        // TODO(jeb) we could (should?) check for a minimum mongo version here ...

        Ok(Self{ _client: client, upstream_config: upstream_config })
    }

    async fn reset(&mut self) -> Result<(), Self::Error> {
        todo!("not yet")
    }

    fn sql_dialect() -> nom_sql::Dialect {
        return nom_sql::Dialect::MongoDB;
    }

    fn url(&self) -> &str {
        self.upstream_config
            .upstream_db_url
            .as_deref()
            .unwrap()
    }

    fn database(&self) -> Option<&str> {
        // maybe in UpstreamConfig? not completely relevant for mongo, perhaps?
        None
    }

    fn version(&self) -> String {
        // include upstream mongo version + readyset info
        // Note: this could be a bit squirrely when performing an in-place
        // upgrade across a replset. Maybe just grab from the current primary.
        panic!("not yet")
    }

    async fn prepare<'a, S>(
        &'a mut self, 
        _query: S,
    ) -> Result<UpstreamPrepare<Self>, Self::Error>
    where
        S: AsRef<str> + Send + Sync + 'a,
    {
        // Mongo doesn't support prepared statements ....
        // maybe just ignore and return success
        // panic!("not yet")
        // Err(Error::ReadySet(ReadySetError::Unsupported))
        Err(Error::ReadySet(unsupported_err!("not yet (?)")))
    }

    async fn execute<'a>(
        &'a mut self,
        _statement_id: u32,
        _params: &[DfValue],
    ) -> Result<Self::QueryResult<'a>, Self::Error> {
        panic!("not yet")
    }

    /// Execute a raw, un-prepared query
    async fn query<'a>(
        &'a mut self, 
        _query: &'a str,
    ) -> Result<Self::QueryResult<'a>, Self::Error> {
        panic!("not yet")
    }

    async fn handle_ryw_write<'a, S>(
        &'a mut self,
        _query: S,
    ) -> Result<(Self::QueryResult<'a>, String), Self::Error>
    where
        S: AsRef<str> + Send + Sync + 'a {
            panic!("not yet")
        }

    async fn start_tx<'a>(&'a mut self) -> Result<Self::QueryResult<'a>, Self::Error> {
        panic!("not yet")
    }

    async fn commit<'a>(&'a mut self) -> Result<Self::QueryResult<'a>, Self::Error> {
        panic!("not yet")
    }

    async fn rollback<'a>(&'a mut self) -> Result<Self::QueryResult<'a>, Self::Error> {
        panic!("not yet")
    }

    async fn schema_dump(&mut self) -> Result<Vec<u8>, anyhow::Error> {
        panic!("not yet")
    }

    async fn schema_search_path(&mut self) -> Result<Vec<SqlIdentifier>, Self::Error> {
        // naively doing what the mysql upstream does :shrug:
        Ok(self.database().into_iter().map(|s| s.into()).collect())
    }

}