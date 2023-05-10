use mongodb::bson::Timestamp;

mod connector;

pub(crate) use connector::MongoDbOplogConnector;
use readyset_client::replication::ReplicationOffset;

// Altight, here's the situation: ResumeToken works great for communicating
// just with Mongo (for resuming a change stream), but it's opaque and a 
// PITA to work with outside of Mongo itself (it is _opaque_ :shrug:).
// We can't compare nor order ResumeTokens, or use them in any useful manner 
// (outside of just sending it back to Mongo).
// This means it's damn hard (if not impossible) to try to use it as the basis
// of a ReplicationOffset in the rest of ReasySet (I don't have enough experience to say, yet).
// Thus, ResumeToken is probably not gonna work for us inside of ReadySet.
//
// However, you can use/get the bson::Timestamp (i.e. clusterTime) of the operation, 
// and I think that will be sufficient enough for everything in ReadySet.
// Timestamp derives Eq, Ord, PartialEq, PartialOrd.
// bson::Timestamp looks to be fairly similar to the postgres Lsn type,
// which basically wraps an i64.

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd, Clone, Copy)]
pub struct OplogPosition {
    pub timestamp: Timestamp,
}

impl OplogPosition {
    fn to_u128(&self) -> u128 {
        let upper = (self.timestamp.time as u64) << 32;
        let lower = self.timestamp.increment as u64;

        (upper | lower) as u128
    }

    // I can't define a a From<u128> item for bson::Timestamp, 
    // so making it a static function here
    fn from_u128(value: u128) -> Timestamp {
        let time = (value >> 32) as u32;
        let increment = value as u32;

        Timestamp { time: time, increment: increment }
    }
}

impl From<&OplogPosition> for ReplicationOffset {
    fn from(value: &OplogPosition) -> Self {
        ReplicationOffset { 
            offset: value.to_u128(), 
            replication_log_name: String::new() 
        }
    }
}

impl From<OplogPosition> for ReplicationOffset {
    fn from(value: OplogPosition) -> Self {
        (&value).into()
    }
}

impl From<&ReplicationOffset> for OplogPosition {
    fn from(value: &ReplicationOffset) -> Self {
        OplogPosition { 
            timestamp: Self::from_u128(value.offset),
        }
    }
}

impl From<ReplicationOffset> for OplogPosition {
    fn from(value: ReplicationOffset) -> Self {
        OplogPosition { 
            timestamp: Self::from_u128(value.offset),
        }
    }
}

impl From<u128> for OplogPosition {
    fn from(value: u128) -> Self {
        OplogPosition { 
            timestamp: Self::from_u128(value) 
        }
    }
}

impl From<Timestamp> for OplogPosition {
    fn from(value: Timestamp) -> Self {
        OplogPosition { 
            timestamp: value }

    }
}
