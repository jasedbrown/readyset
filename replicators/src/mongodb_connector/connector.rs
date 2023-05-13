use core::time::Duration;

use async_trait::async_trait;
use mongodb::bson::Bson;
use mongodb::Client;
use mongodb::bson::Document;
use mongodb::change_stream::ChangeStream;
use mongodb::change_stream::event::{ChangeStreamEvent, OperationType::*};
use mongodb::options::{ChangeStreamOptions, ReadConcern, SelectionCriteria, ReadPreference, ClientOptions};
use nom_sql::Relation;
use readyset_client::replication::ReplicationOffset;
use readyset_data::DfValue;
use readyset_errors::{ReadySetResult, internal_err};
use tracing::info;

use super::OplogPosition;
use crate::noria_adapter::{Connector, ReplicationAction};

/// A connector that connectst to a MongoDB replset (targeting the primary)
/// /// and tails the oplog from a given position.
pub(crate) struct MongoDbOplogConnector {
    /// Client instance that maintains the connections to the backing mongodb replset.
    client: mongodb::Client,

    // It would be the best to create this only in connect(), and be done.
    // Unfortunately, I hit several compiler fails that ChangeStream<..> 
    // does not implement the Send trait. As I couldn't figure out how to immediately resolve
    // that (I tried the Arc<Mutex<>> pattern, as well), but something about the await 
    // just didn't work. In order to plow ahead on the rest of this project, I'm naively
    // ignoring for now, and doing the shitty thing just to move on. I'll end up learning more along the way .....
    /// The stream via which we get all the mongodb oplog entries.
    // change_stream: Arc<Mutex<ChangeStream<ChangeStreamEvent<Document>>>>,

    change_stream_options: ChangeStreamOptions,

    /// If we just want to continue reading the binlog from a previous point
    next_position: OplogPosition,
    /// Whether to log statements received by the connector
    enable_statement_logging: bool,
}

impl MongoDbOplogConnector {
    pub(crate) async fn connect(
        client_options: ClientOptions,
        next_position: OplogPosition,
        enable_statement_logging: bool
    ) -> ReadySetResult<Self> {
        let client = Client::with_options(client_options)?;

        let change_stream_options = ChangeStreamOptions::builder()
            .batch_size(Some(16 as u32))                     // sure, 16 seems like a fine batch size :shrug:
            .start_at_operation_time(Some(next_position.timestamp.clone()))
            .max_await_time(Some(Duration::new(1, 0))) // one second? not sure if this drops the stream or just returns from blocking...
            .read_concern(Some(ReadConcern::MAJORITY))
            .selection_criteria(Some(SelectionCriteria::ReadPreference(ReadPreference::Primary)))
            .build();

        let connector = MongoDbOplogConnector {
            client: client,
            // change_stream: Arc::new(Mutex::new(change_stream)),
            change_stream_options,
            next_position: next_position,
            enable_statement_logging: enable_statement_logging,
        };
        Ok(connector)
    }

    async fn next_action_inner(
        &mut self, 
        until: Option<&ReplicationOffset>
    ) -> mongodb::error::Result<(ReplicationAction, &OplogPosition)> {

        // TODO(jeb) should only create the change stream once, in connect(). See notes in MongoDbOplogConnector struct.
        let mut change_stream: ChangeStream<ChangeStreamEvent<Document>> = self.client.watch(None, self.change_stream_options.clone()).await?;

        loop {
            if let Some(event) = change_stream.next_if_any().await? {
                let timestamp = event.cluster_time.ok_or_else(|| {
                    mongodb::error::Error::custom(Box::new(internal_err!("couldn't get the clusterTimestamp of the event")))
                })?;
                self.next_position = OplogPosition { timestamp };

                if self.enable_statement_logging {
                    info!(target: "replicator_statement", "{:?}", event);
                }

                // event types: https://www.mongodb.com/docs/manual/reference/change-events/
                match event.operation_type {
                    Insert => {
                        let namespace = event.ns.ok_or_else(|| {
                            mongodb::error::Error::custom(Box::new(internal_err!("couldn't get the namespace of the event")))
                        })?;
                        let _id_doc = event.document_key.ok_or_else(|| {
                            mongodb::error::Error::custom(Box::new(internal_err!("couldn't get the document id of the event")))
                        })?;
                        let doc: Document = event.full_document.ok_or_else(|| {
                            mongodb::error::Error::custom(Box::new(internal_err!("couldn't get the document of the event")))
                        })?;

                        // convert doc to noria rows....
                        let mut inserted_docs = Vec::new();
                        inserted_docs.push(readyset_client::TableOperation::Insert(
                            oplog_doc_to_noria_row(&doc)?,
                        ));

                        return Ok((
                            ReplicationAction::TableAction {
                                table: Relation {
                                    schema: Some(namespace.db.into()),
                                    name: namespace.coll.ok_or_else(|| {
                                        mongodb::error::Error::custom(Box::new(internal_err!("couldn't get the collection of the event")))
                                    })?.into(),
                                },
                                actions: inserted_docs,
                                txid: Some(0), // TODO(jeb) fix this
                            },
                            &self.next_position,
                        ));
                    },
                    Update => {

                    },
                    Replace => {

                    },
                    Delete => {

                    },
                    Drop => {

                    },
                    Rename => {

                    },
                    DropDatabase => {

                    },
                    Invalidate => {
                        // we need to blow up the change stream and start it over when we get this event type
                    },
                    Other(_s) => {
                        // lol, does the mongo driver not even have a clue here :shrug:
                    },
                    _ => {

                    }
                }

                // Ok((entry, &self.next_position))
            }

            // We didn't get an actionable event, but we still need to check that we haven't reached
            // the until limit
            if let Some(limit) = until {
                let limit = OplogPosition::try_from(limit).expect("Valid oplog limit");
                if self.next_position >= limit {
                    return Ok((ReplicationAction::LogPosition, &self.next_position));
                }
            }
        }
    }
}

fn oplog_doc_to_noria_row(
    doc: &Document,
) -> mongodb::error::Result<Vec<DfValue>> {

    let mut inserted_columns: Vec<DfValue> = Vec::new();
    
    // assumming no nested/sub documents, keep life simple
    for item in doc.iter() {
        let _key = item.0;
        let val = item.1;
        match val {
            // TODO(jeb) do some better error handling rather than expect()
            Bson::Double(v) => inserted_columns.push((*v).try_into().expect("bad f64")),
            Bson::String(v) => inserted_columns.push((*v).clone().try_into().expect("bad string")),
            Bson::Boolean(v) => inserted_columns.push((*v).try_into().expect("bad bool")),
            Bson::Int32(v) => inserted_columns.push((*v).try_into().expect("bad i32")),
            Bson::Int64(v) => inserted_columns.push((*v).try_into().expect("bad i64")),
            Bson::ObjectId(_v) => panic!("handle me"),
            Bson::DateTime(_v) => panic!("handle me"),
            _ => panic!("handle me"),
        };
    }
    Ok(inserted_columns)
}

#[async_trait]
impl Connector for MongoDbOplogConnector {
    async fn next_action(
        &mut self,
        _last_pos: &ReplicationOffset,
        until: Option<&ReplicationOffset>,
    ) -> ReadySetResult<(ReplicationAction, ReplicationOffset)> {
        let (action, pos) = self.next_action_inner(until).await?;

        Ok((action, pos.into()))
    }
}
