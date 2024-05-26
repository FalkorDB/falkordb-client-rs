/*
 * Copyright FalkorDB Ltd. 2023 - present
 * Licensed under the Server Side Public License v1 (SSPLv1).
 */

#[cfg(not(feature = "redis"))]
compile_error!("The `redis` feature must be enabled.");

mod client;
mod connection;
mod connection_info;
mod error;
mod graph;
mod graph_schema;
mod parser;
mod response;
mod value;

#[cfg(feature = "redis")]
mod redis_ext;

pub use client::{blocking::FalkorSyncClient, builder::FalkorClientBuilder};
pub use connection_info::FalkorConnectionInfo;
pub use error::FalkorDBError;
pub use graph::blocking::SyncGraph;
pub use graph_schema::{blocking::SyncGraphSchema, SchemaType};
pub use parser::FalkorParsable;
pub use response::{
    constraint::{Constraint, ConstraintStatus, ConstraintType},
    execution_plan::ExecutionPlan,
    index::{FalkorIndex, IndexFieldType, IndexStatus},
    query_result::QueryResult,
    slowlog_entry::SlowlogEntry,
    ResponseVariant,
};
pub use value::{
    config::ConfigValue,
    graph_entities::{Edge, EntityType, Node},
    path::Path,
    point::Point,
    FalkorValue,
};

#[cfg(feature = "tokio")]
pub use {
    client::asynchronous::FalkorAsyncClient, connection::asynchronous::FalkorAsyncConnection,
    graph::asynchronous::AsyncGraph, graph_schema::asynchronous::AsyncGraphSchema,
    parser::FalkorAsyncParseable,
};

#[cfg(test)]
pub(crate) mod test_utils {
    use crate::{FalkorClientBuilder, FalkorSyncClient, SyncGraph};

    pub(crate) struct TestGraphHandle {
        pub(crate) inner: SyncGraph,
    }

    impl Drop for TestGraphHandle {
        fn drop(&mut self) {
            self.inner.delete().ok();
        }
    }

    pub(crate) fn create_test_client() -> FalkorSyncClient {
        FalkorClientBuilder::new()
            .build()
            .expect("Could not create client")
    }

    pub(crate) fn open_test_graph(graph_name: &str) -> TestGraphHandle {
        let client = create_test_client();

        client.open_graph(graph_name).delete().ok();

        TestGraphHandle {
            inner: client
                .copy_graph("imdb", graph_name)
                .expect("Could not copy graph for test"),
        }
    }
}
