use nom_sql::{SqlQuery, SetStatement};
use readyset_adapter::{QueryHandler, SetBehavior};
use readyset_errors::{ReadySetResult};
use readyset_adapter::backend::noria_connector::QueryResult;
pub struct MongoDbQueryHandler;

impl QueryHandler for MongoDbQueryHandler {
    fn requires_fallback(_query: &SqlQuery) -> bool{
        return true;
    }

    fn default_response(_query: &SqlQuery) -> ReadySetResult<QueryResult<'static>> {
        panic!("not supported")
    }

    fn handle_set_statement(_stmt: &SetStatement) -> SetBehavior {
        panic!("not supported")
    }
}
