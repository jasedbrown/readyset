use mongodb::change_stream::event::ResumeToken;

mod connector;

pub(crate) use connector::MongoDbOplogConnector;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct OplogPosition {
    pub resume_token: ResumeToken,
}

