use core::time::Duration;

use async_trait::async_trait;
// use futures::TryStreamExt;
use mongodb::Client;
use mongodb::bson::Document;
use mongodb::change_stream::ChangeStream;
use mongodb::change_stream::event::{ChangeStreamEvent, OperationType::*};
use mongodb::options::{ChangeStreamOptions, ReadConcern, SelectionCriteria, ReadPreference, ClientOptions};
use readyset_client::replication::ReplicationOffset;
use readyset_errors::{ReadySetResult, internal_err};

use super::OplogPosition;
use crate::noria_adapter::{Connector, ReplicationAction};

/// A connector that connectst to a MongoDB replset (targetting the primary)
/// /// and tails the oplog from a given position.
pub(crate) struct MongoDbOplogConnector {
    /// Client instance that maintains the connections to the backing mongodb replset.
    client: mongodb::Client,
    /// The stream via which we get all the mongodb oplog entries.
    change_stream: ChangeStream<ChangeStreamEvent<Document>>,
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
            .build()
        ;
        let change_stream: ChangeStream<ChangeStreamEvent<Document>> = client.watch(None, change_stream_options).await?;

        let connector = MongoDbOplogConnector {
            client: client,
            change_stream: change_stream,
            next_position: next_position,
            enable_statement_logging: enable_statement_logging,
        };
        Ok(connector)
    }

    async fn next_action_inner(
        &mut self, 
        until: Option<&ReplicationOffset>
    ) -> mongodb::error::Result<(ReplicationAction, &OplogPosition)> {
        loop {
            if let Some(event) = self.change_stream.next_if_any().await? {
                // let resume_token = self.change_stream.resume_token().ok_or_else(|| {
                //     mongodb::error::Error::custom(Box::new(internal_err!("couldn't get the oplog position")))
                // })?;
                let timestamp = event.cluster_time.ok_or_else(|| {
                    mongodb::error::Error::custom(Box::new(internal_err!("couldn't get the clusterTimestamp of the event")))
                })?;
                self.next_position = OplogPosition { timestamp };

                match event.operation_type {
                    Insert => {

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

                    },
                    Other(s) => {

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


#[async_trait]
impl Connector for MongoDbOplogConnector {
    async fn next_action(
        &mut self,
        last_pos: &ReplicationOffset,
        until: Option<&ReplicationOffset>,
    ) -> ReadySetResult<(ReplicationAction, ReplicationOffset)> {
        let (action, pos) =- self.next_action_inner(until).await?;
        Ok((action, pos.try_into()?))
    }
}
