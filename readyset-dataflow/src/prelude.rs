//! This module defines crate-local *and* public type exports.
//!
//! It is expected that files within the dataflow crate have use prelude::* at the top, and the
//! same applies to external users of the dataflow crate. Therefore, pay attention to whether `pub`
//! or `crate` is used.

use std::cell;

// core types
pub(crate) use crate::processing::{
    Ingredient, Lookup, Miss, ProcessingResult, RawProcessingResult, ReplayContext,
};
pub(crate) type Edge = ();

// dataflow types
pub(crate) use dataflow_state::{LookupResult, MemoryState, PersistentState, RecordResult, State};
pub(crate) use readyset_client::PacketPayload;

// domain local state
pub use crate::node_map::NodeMap;
pub(crate) use crate::payload::ReplayPathSegment;
pub(crate) type StateMap = NodeMap<MaterializedNodeState>;
pub type DomainNodes = NodeMap<cell::RefCell<Node>>;

// public exports
pub use common::*;
pub use petgraph::graph::NodeIndex;
pub use readyset_client::internal::*;

pub use crate::node::Node;
pub use crate::ops::NodeOperator;
pub use crate::payload::Packet;
pub use crate::Sharding;
pub type Graph = petgraph::Graph<Node, Edge>;
use dataflow_state::MaterializedNodeState;
pub use dataflow_state::{DurabilityMode, PersistenceParameters};
pub use readyset_errors::*;
pub use vec1::vec1;

pub use crate::processing::{ColumnRef, ColumnSource};
pub use crate::EvictionKind;

/// Channel coordinator type specialized for domains
pub type ChannelCoordinator =
    readyset_client::channel::ChannelCoordinator<ReplicaAddress, Box<Packet>>;
pub trait Executor {
    fn send(&mut self, dest: ReplicaAddress, m: Box<Packet>);
}
