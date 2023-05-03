use async_trait::async_trait;
use readyset_adapter::UpstreamDatabase;

// A connector to the underlying mongodb instance/replset.
pub struct MongoDbUpstream {
    // driver instance ?? or just conn??

}

#[async_trait]
impl UpstreamDatabase for MongoDbUpstream {

}