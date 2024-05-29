/*
 * Copyright FalkorDB Ltd. 2023 - present
 * Licensed under the Server Side Public License v1 (SSPLv1).
 */

use crate::{
    client::asynchronous::FalkorAsyncClientInner,
    graph::utils::{construct_query, generate_procedure_call},
    parser::utils::{parse_header, parse_result_set},
    Constraint, ConstraintType, EntityType, ExecutionPlan, FalkorDBError, FalkorIndex,
    FalkorParsable, FalkorResponse, FalkorValue, GraphSchema, IndexType, ResultSet, SlowlogEntry,
};
use anyhow::Result;
use std::{collections::HashMap, fmt::Display, sync::Arc};
use tokio::sync::Mutex;

/// The main graph API, this allows the user to perform graph operations while exposing as little details as possible.
///
/// # Thread Safety
/// This struct is fully thread safe, it can be cloned and passed within threads without constraints,
/// Its API uses only immutable references
#[derive(Clone)]
pub struct AsyncGraph {
    pub(crate) client: Arc<FalkorAsyncClientInner>,
    pub(crate) graph_name: String,
    /// Provides user with access to the current graph schema,
    /// which contains a safe cache of id to labels/properties/relationship maps
    pub graph_schema: GraphSchema,
}

impl AsyncGraph {
    /// Returns the name of the graph for which this API performs operations.
    ///
    /// # Returns
    /// The graph name as a string slice, without cloning.
    pub fn graph_name(&self) -> &str {
        self.graph_name.as_str()
    }

    async fn send_command(
        &self,
        command: &str,
        subcommand: Option<&str>,
        params: Option<&[String]>,
    ) -> Result<FalkorValue> {
        let mut conn = self.client.borrow_connection().await?;
        conn.send_command(Some(self.graph_name.as_str()), command, subcommand, params)
            .await
    }

    /// Deletes the graph stored in the database, and drop all the schema caches.
    /// NOTE: This still maintains the graph API, operations are still viable.
    pub async fn delete(&mut self) -> Result<()> {
        self.send_command("GRAPH.DELETE", None, None).await?;
        self.graph_schema.clear();
        Ok(())
    }

    /// Retrieves the slowlog data, which contains info about the N slowest queries.
    ///
    /// # Returns
    /// A [`Vec`] of [`SlowlogEntry`], providing information about each query.
    pub async fn slowlog(&self) -> Result<Vec<SlowlogEntry>> {
        let res = self
            .send_command("GRAPH.SLOWLOG", None, None)
            .await?
            .into_vec()?;

        if res.is_empty() {
            return Ok(vec![]);
        }

        Ok(res.into_iter().flat_map(SlowlogEntry::try_from).collect())
    }

    /// Resets the slowlog, all query time data will be cleared.
    pub async fn slowlog_reset(&self) -> Result<FalkorValue> {
        self.send_command("GRAPH.SLOWLOG", None, Some(&["RESET".to_string()]))
            .await
    }

    /// Returns an [`ExecutionPlan`] object for the selected query,
    /// showing how long each step took to perform.
    /// This function variant allows adding extra parameters after the query
    ///
    /// # Arguments
    /// * `query_string`: The query to profile
    /// * `params`: a map of parameters and values, note that all keys should be of the same type, and all values should be of the same type.
    ///
    /// # Returns
    /// An [`ExecutionPlan`], which can provide info about each step, or a plaintext explanation of the whole thing for printing.
    pub async fn profile_with_params<Q: ToString, T: ToString, Z: ToString>(
        &self,
        query_string: Q,
        params: Option<&HashMap<T, Z>>,
    ) -> Result<ExecutionPlan> {
        let query = construct_query(query_string, params);

        ExecutionPlan::try_from(
            self.send_command("GRAPH.PROFILE", None, Some(&[query]))
                .await?,
        )
        .map_err(Into::into)
    }

    /// Returns an [`ExecutionPlan`] object for the selected query,
    /// showing how long each step took to perform.
    ///
    /// # Arguments
    /// * `query_string`: The query to profile
    ///
    /// # Returns
    /// An [`ExecutionPlan`], which can provide info about each step, or a plaintext explanation of the whole thing for printing.
    pub async fn profile<Q: ToString>(
        &self,
        query_string: Q,
    ) -> Result<ExecutionPlan> {
        self.profile_with_params::<Q, &str, &str>(query_string, None)
            .await
    }

    /// Returns an [`ExecutionPlan`] object for the selected query,
    /// showing the internals steps the database will go through to perform the query.
    /// This function variant allows adding extra parameters after the query
    ///
    /// # Arguments
    /// * `query_string`: The query to explain
    /// * `params`: a map of parameters and values, note that all keys should be of the same type, and all values should be of the same type.
    ///
    /// # Returns
    /// An [`ExecutionPlan`], which can provide info about each step, or a plaintext explanation of the whole thing for printing.
    pub async fn explain_with_params<Q: ToString, T: ToString, Z: ToString>(
        &self,
        query_string: Q,
        params: Option<&HashMap<T, Z>>,
    ) -> Result<ExecutionPlan> {
        let query = construct_query(query_string, params);
        ExecutionPlan::try_from(
            self.send_command("GRAPH.EXPLAIN", None, Some(&[query]))
                .await?,
        )
        .map_err(Into::into)
    }

    /// Returns an [`ExecutionPlan`] object for the selected query,
    /// showing the internals steps the database will go through to perform the query.
    ///
    /// # Arguments
    /// * `query_string`: The query to explain
    ///
    /// # Returns
    /// An [`ExecutionPlan`], which can provide info about each step, or a plaintext explanation of the whole thing for printing.
    pub async fn explain<Q: ToString>(
        &self,
        query_string: Q,
    ) -> Result<ExecutionPlan> {
        self.explain_with_params::<Q, &str, &str>(query_string, None)
            .await
    }

    async fn query_inner_with_timeout<Q: ToString, T: ToString, Z: ToString>(
        &mut self,
        command: &str,
        query_string: Q,
        params: Option<&HashMap<T, Z>>,
        timeout: i64,
    ) -> Result<FalkorResponse<ResultSet>> {
        let query = construct_query(query_string, params);
        let mut conn = self.client.borrow_connection().await?;

        let [header, data, stats]: [FalkorValue; 3] = conn
            .send_command(
                Some(self.graph_name.as_str()),
                command,
                None,
                Some(&[
                    query.as_str(),
                    "--compact",
                    format!("timeout {timeout}").as_str(),
                ]),
            )
            .await?
            .into_vec()?
            .try_into()
            .map_err(|_| FalkorDBError::ParsingArrayToStructElementCount)?;

        let header_keys = parse_header(header)?;
        let conn = Arc::new(Mutex::new(conn));
        FalkorResponse::from_response_with_headers(
            parse_result_set(data, &mut self.graph_schema, conn)?,
            header_keys,
            stats,
        )
        .map_err(Into::into)
    }

    async fn query_inner<Q: ToString, T: ToString, Z: ToString>(
        &mut self,
        command: &str,
        query_string: Q,
        params: Option<&HashMap<T, Z>>,
    ) -> Result<FalkorResponse<ResultSet>> {
        let query = construct_query(query_string, params);
        let mut conn = self.client.borrow_connection().await?;

        let res = conn
            .send_command(
                Some(self.graph_name.as_str()),
                command,
                None,
                Some(&[query, "--compact".to_string()]),
            )
            .await?
            .into_vec()?;

        match res.len() {
            1 => {
                let stats = res
                    .into_iter()
                    .next()
                    .ok_or(FalkorDBError::ParsingArrayToStructElementCount)?;

                FalkorResponse::from_response(None, vec![], stats)
            }
            2 => {
                let [header, stats]: [FalkorValue; 2] = res
                    .try_into()
                    .map_err(|_| FalkorDBError::ParsingArrayToStructElementCount)?;

                FalkorResponse::from_response(Some(header), vec![], stats)
            }
            3 => {
                let [header, data, stats]: [FalkorValue; 3] = res
                    .try_into()
                    .map_err(|_| FalkorDBError::ParsingArrayToStructElementCount)?;

                let header_keys = parse_header(header)?;
                let conn = Arc::new(Mutex::new(conn));
                FalkorResponse::from_response_with_headers(
                    parse_result_set(data, &mut self.graph_schema, conn, &header_keys)?,
                    header_keys,
                    stats,
                )
            }
            _ => Err(FalkorDBError::ParsingArrayToStructElementCount),
        }
        .map_err(Into::into)
    }

    /// Run a query on the graph
    ///
    /// # Arguments
    /// * `query_string`: The query to run
    ///
    /// # Returns
    /// A [`QueryResult`] object, containing the headers, statistics and the result set for the query
    pub async fn query<Q: Display>(
        &mut self,
        query_string: Q,
    ) -> Result<FalkorResponse<ResultSet>> {
        self.query_inner::<Q, &str, &str>("GRAPH.QUERY", query_string, None)
            .await
    }

    /// Run a query on the graph, but abort it if it exceeds the timeout
    ///
    /// # Arguments
    /// * `query_string`: The query to run
    /// * `timeout`: Specify how long should the query run before aborting.
    ///
    /// # Returns
    /// A [`QueryResult`] object, containing the headers, statistics and the result set for the query
    pub async fn query_with_timeout<Q: Display>(
        &mut self,
        query_string: Q,
        timeout: i64,
    ) -> Result<FalkorResponse<ResultSet>> {
        self.query_inner_with_timeout::<Q, &str, &str>("GRAPH.QUERY", query_string, None, timeout)
            .await
    }

    /// Run a query on the graph
    /// This function variant allows adding extra parameters after the query
    ///
    /// # Arguments
    /// * `query_string`: The query to run
    /// * `params`: a map of parameters and values, note that all keys should be of the same type, and all values should be of the same type.
    ///
    /// # Returns
    /// A [`FalkorResponse<ResultSet>`] object, containing the headers, statistics and the result set for the query
    pub async fn query_with_params<Q: Display, T: Display, Z: Display>(
        &mut self,
        query_string: Q,
        params: &HashMap<T, Z>,
    ) -> Result<FalkorResponse<ResultSet>> {
        self.query_inner("GRAPH.QUERY", query_string, Some(params))
            .await
    }

    /// Run a query on the graph but abort it if it exceeds the timeout
    /// This function variant allows adding extra parameters after the query
    ///
    /// # Arguments
    /// * `query_string`: The query to run
    /// * `timeout`: Specify how long should the query run before aborting.
    /// * `params`: a map of parameters and values, note that all keys should be of the same type, and all values should be of the same type.
    ///
    /// # Returns
    /// A [`QueryResult`] object, containing the headers, statistics and the result set for the query
    pub async fn query_with_params_and_timeout<Q: Display, T: Display, Z: Display>(
        &mut self,
        query_string: Q,
        timeout: i64,
        params: &HashMap<T, Z>,
    ) -> Result<FalkorResponse<ResultSet>> {
        self.query_inner_with_timeout("GRAPH.QUERY", query_string, Some(params), timeout)
            .await
    }

    /// Run a query on the graph
    /// Read-only queries are more limited with the operations they are allowed to perform.
    ///
    /// # Arguments
    /// * `query_string`: The query to run
    ///
    /// # Returns
    /// A [`FalkorResponse<ResultSet>`] object, containing the headers, statistics and the result set for the query
    pub async fn query_readonly<Q: Display>(
        &mut self,
        query_string: Q,
    ) -> Result<FalkorResponse<ResultSet>> {
        self.query_inner::<Q, &str, &str>("GRAPH.QUERY_RO", query_string, None)
            .await
    }

    /// Run a query on the graph, but abort it if it exceeds the timeout
    /// Read-only queries are more limited with the operations they are allowed to perform.
    ///
    /// # Arguments
    /// * `query_string`: The query to run
    /// * `timeout`: Specify how long should the query run before aborting.
    ///
    /// # Returns
    /// A [`FalkorResponse<ResultSet>`] object, containing the headers, statistics and the result set for the query
    pub async fn query_readonly_with_timeout<Q: Display>(
        &mut self,
        query_string: Q,
        timeout: i64,
    ) -> Result<FalkorResponse<ResultSet>> {
        self.query_inner_with_timeout::<Q, &str, &str>(
            "GRAPH.QUERY_RO",
            query_string,
            None,
            timeout,
        )
        .await
    }

    /// Run a read-only query on the graph
    /// Read-only queries are more limited with the operations they are allowed to perform.
    /// This function variant allows adding extra parameters after the query
    ///
    /// # Arguments
    /// * `query_string`: The query to run
    /// * `timeout`: Specify how long should the query run before aborting.
    /// * `params`: a map of parameters and values, note that all keys should be of the same type, and all values should be of the same type.
    ///
    /// # Returns
    /// A [`FalkorResponse<ResultSet>`] object, containing the headers, statistics and the result set for the query
    pub async fn query_readonly_with_params<Q: ToString, T: ToString, Z: ToString>(
        &mut self,
        query_string: Q,
        params: &HashMap<T, Z>,
    ) -> Result<FalkorResponse<ResultSet>> {
        self.query_inner("GRAPH.QUERY_RO", query_string, Some(params))
            .await
    }

    /// Run a read-only query on the graph, but abort it if it exceeds the timeout
    /// Read-only queries are more limited with the operations they are allowed to perform.
    /// This function variant allows adding extra parameters after the query
    ///
    /// # Arguments
    /// * `query_string`: The query to run
    /// * `timeout`: Specify how long should the query run before aborting.
    /// * `params`: a map of parameters and values, note that all keys should be of the same type, and all values should be of the same type.
    ///
    /// # Returns
    /// A [`FalkorResponse<ResultSet>`] object, containing the headers, statistics and the result set for the query
    pub async fn query_readonly_with_params_and_timeout<Q: ToString, T: ToString, Z: ToString>(
        &mut self,
        query_string: Q,
        params: &HashMap<T, Z>,
        timeout: i64,
    ) -> Result<FalkorResponse<ResultSet>> {
        self.query_inner_with_timeout("GRAPH.QUERY_RO", query_string, Some(params), timeout)
            .await
    }

    /// Run a query which calls a procedure on the graph, read-only, or otherwise.
    /// Read-only queries are more limited with the operations they are allowed to perform.
    /// This function allows adding extra parameters after the query, and adding a YIELD block afterward
    ///
    /// # Arguments
    /// * `procedure`: The procedure to call
    /// * `args`: An optional slice of strings containing the parameters.
    /// * `yields`: The optional yield block arguments.
    /// * `read_only`: Whether this procedure is read-only.
    /// * `timeout`: If provided, the query will abort if overruns the timeout.
    ///
    /// # Returns
    /// A caller-provided type which implements [`FalkorParsableAsync`]
    pub async fn call_procedure<C: ToString, P: FalkorParsable>(
        &mut self,
        procedure: C,
        args: Option<&[&str]>,
        yields: Option<&[&str]>,
        read_only: bool,
    ) -> Result<P> {
        let (query_string, params) = generate_procedure_call(procedure, args, yields);
        let query = construct_query(query_string, params.as_ref());
        let mut conn = self.client.borrow_connection().await?;

        let res = conn
            .send_command(
                Some(self.graph_name.as_str()),
                if read_only {
                    "GRAPH.QUERY_RO"
                } else {
                    "GRAPH.QUERY"
                },
                None,
                Some(&[query, "--compact".to_string()]),
            )
            .await?;

        let conn = Arc::new(Mutex::new(conn));
        P::from_falkor_value(res, &mut self.graph_schema, conn)
    }

    /// Run a query which calls a procedure on the graph, read-only, or otherwise.
    /// Read-only queries are more limited with the operations they are allowed to perform.
    /// This function allows adding extra parameters after the query, and adding a YIELD block afterward
    /// This function will cause the query to abort if it exceeds a certain timeout
    ///
    /// # Arguments
    /// * `procedure`: The procedure to call
    /// * `args`: An optional slice of strings containing the parameters.
    /// * `yields`: The optional yield block arguments.
    /// * `read_only`: Whether this procedure is read-only.
    /// * `timeout`: If provided, the query will abort if overruns the timeout.
    ///
    /// # Returns
    /// A caller-provided type which implements [`FalkorParsableAsync`]
    pub async fn call_procedure_with_timeout<C: ToString, P: FalkorParsable>(
        &mut self,
        procedure: C,
        args: Option<&[&str]>,
        yields: Option<&[&str]>,
        read_only: bool,
        timeout: i64,
    ) -> Result<P> {
        let (query_string, params) = generate_procedure_call(procedure, args, yields);
        let query = construct_query(query_string, params.as_ref());
        let mut conn = self.client.borrow_connection().await?;

        let res = conn
            .send_command(
                Some(self.graph_name.as_str()),
                if read_only {
                    "GRAPH.QUERY_RO"
                } else {
                    "GRAPH.QUERY"
                },
                None,
                Some(&[
                    query.as_str(),
                    "--compact",
                    format!("timeout {timeout}").as_str(),
                ]),
            )
            .await?;

        let conn = Arc::new(Mutex::new(conn));
        P::from_falkor_value(res, &mut self.graph_schema, conn)
    }

    /// Calls the DB.INDICES procedure on the graph, returning all the indexing methods currently used
    ///
    /// # Returns
    /// A [`Vec`] of [`FalkorIndex`]
    pub async fn list_indices(&mut self) -> Result<FalkorResponse<Vec<FalkorIndex>>> {
        let [header, indices, stats]: [FalkorValue; 3] = self
            .call_procedure::<&str, FalkorValue>("DB.INDEXES", None, None, false)
            .await?
            .into_vec()?
            .try_into()
            .map_err(|_| FalkorDBError::ParsingArrayToStructElementCount)?;

        let conn = Arc::new(Mutex::new(self.client.borrow_connection().await?));
        FalkorResponse::from_response(
            Some(header),
            indices
                .into_vec()?
                .into_iter()
                .flat_map(|index| {
                    let conn = Arc::clone(&conn);
                    FalkorIndex::from_falkor_value(index, &mut self.graph_schema, conn)
                })
                .collect(),
            stats,
        )
        .map_err(Into::into)
    }

    pub async fn create_index<L: ToString, P: ToString>(
        &mut self,
        index_field_type: IndexType,
        entity_type: EntityType,
        label: L,
        properties: &[P],
        options: Option<&HashMap<String, String>>,
    ) -> Result<FalkorResponse<ResultSet>> {
        // Create index from these properties
        let properties_string = properties
            .iter()
            .map(|element| format!("l.{}", element.to_string()))
            .collect::<Vec<_>>()
            .join(", ");

        let pattern = match entity_type {
            EntityType::Node => format!("(l:{})", label.to_string()),
            EntityType::Edge => format!("()-[l:{}]->()", label.to_string()),
        };

        let idx_type = match index_field_type {
            IndexType::Range => "",
            IndexType::Vector => "VECTOR ",
            IndexType::Fulltext => "FULLTEXT ",
        }
        .to_string();

        let options_string = options
            .map(|hashmap| {
                hashmap
                    .iter()
                    .map(|(key, val)| format!("'{key}':'{val}'"))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .map(|options_string| format!(" OPTIONS {{ {} }}", options_string))
            .unwrap_or_default();

        self.query(format!(
            "CREATE {idx_type}INDEX FOR {pattern} ON ({}){}",
            properties_string, options_string
        ))
        .await
    }

    /// Drop an existing index, by specifying its type, entity, label and specific properties
    ///
    /// # Arguments
    /// * `index_field_type`
    pub async fn drop_index<L: ToString, P: ToString>(
        &mut self,
        index_field_type: IndexType,
        entity_type: EntityType,
        label: L,
        properties: &[P],
    ) -> Result<FalkorResponse<ResultSet>> {
        let properties_string = properties
            .iter()
            .map(|element| format!("e.{}", element.to_string()))
            .collect::<Vec<_>>()
            .join(", ");

        let pattern = match entity_type {
            EntityType::Node => format!("(e:{})", label.to_string()),
            EntityType::Edge => format!("()-[e:{}]->()", label.to_string()),
        };

        let idx_type = match index_field_type {
            IndexType::Range => "",
            IndexType::Vector => "VECTOR",
            IndexType::Fulltext => "FULLTEXT",
        }
        .to_string();

        self.query(format!(
            "DROP {idx_type} INDEX for {pattern} ON ({})",
            properties_string
        ))
        .await
    }

    /// Calls the DB.CONSTRAINTS procedure on the graph, returning an array of the graph's constraints
    ///
    /// # Returns
    /// A tuple where the first element is a [`Vec`] of [`Constraint`]s, and the second element is a [`Vec`] of stats as [`String`]s
    pub async fn list_constraints(&mut self) -> Result<FalkorResponse<Vec<Constraint>>> {
        let mut conn = self.client.borrow_connection().await?;
        let [header, query_res, stats]: [FalkorValue; 3] = self
            .call_procedure::<&str, FalkorValue>("DB.CONSTRAINTS", None, None, false)
            .await?
            .into_vec()?
            .try_into()
            .map_err(|_| FalkorDBError::ParsingArrayToStructElementCount)?;

        let conn = Arc::new(Mutex::new(conn));
        FalkorResponse::from_response(
            Some(header),
            query_res
                .into_vec()?
                .into_iter()
                .flat_map(|item| Constraint::from_falkor_value(item, &mut self.graph_schema, conn))
                .collect(),
            stats,
        )
        .map_err(Into::into)
    }

    /// Creates a new constraint for this graph, making the provided properties mandatory
    ///
    /// # Arguments
    /// * `entity_type`: Whether to apply this constraint on nodes or relationships.
    /// * `label`: Entities with this label will have this constraint applied to them.
    /// * `properties`: A slice of the names of properties this constraint will apply to.
    pub async fn create_mandatory_constraint(
        &self,
        entity_type: EntityType,
        label: &str,
        properties: &[&str],
    ) -> Result<FalkorValue> {
        let mut params = Vec::with_capacity(5 + properties.len());
        params.extend([
            "MANDATORY".to_string(),
            entity_type.to_string(),
            label.to_string(),
            "PROPERTIES".to_string(),
            properties.len().to_string(),
        ]);
        params.extend(properties.iter().map(|property| property.to_string()));

        self.send_command("GRAPH.CONSTRAINT", Some("CREATE"), Some(params.as_slice()))
            .await
    }

    /// Creates a new constraint for this graph, making the provided properties unique
    ///
    /// # Arguments
    /// * `entity_type`: Whether to apply this constraint on nodes or relationships.
    /// * `label`: Entities with this label will have this constraint applied to them.
    /// * `properties`: A slice of the names of properties this constraint will apply to.
    pub async fn create_unique_constraint<P: ToString>(
        &mut self,
        entity_type: EntityType,
        label: String,
        properties: &[P],
    ) -> Result<FalkorValue> {
        self.create_index(
            IndexType::Range,
            entity_type,
            label.as_str(),
            properties,
            None,
        )
        .await?;

        let mut params: Vec<String> = Vec::with_capacity(5 + properties.len());
        params.extend([
            "UNIQUE".to_string(),
            entity_type.to_string(),
            label.to_string(),
            "PROPERTIES".to_string(),
            properties.len().to_string(),
        ]);
        params.extend(properties.iter().map(|property| property.to_string()));

        // create constraint using index
        self.send_command("GRAPH.CONSTRAINT", Some("CREATE"), Some(params.as_slice()))
            .await
    }

    /// Drop an existing constraint from the graph
    ///
    /// # Arguments
    /// * `constraint_type`: Which type of constraint to remove.
    /// * `entity_type`: Whether this constraint exists on nodes or relationships.
    /// * `label`: Remove the constraint from entities with this label.
    /// * `properties`: A slice of the names of properties to remove the constraint from.
    pub async fn drop_constraint<L: ToString, P: ToString>(
        &self,
        constraint_type: ConstraintType,
        entity_type: EntityType,
        label: L,
        properties: &[P],
    ) -> Result<FalkorValue> {
        let mut params = Vec::with_capacity(5 + properties.len());
        params.extend([
            constraint_type.to_string(),
            entity_type.to_string(),
            label.to_string(),
            "PROPERTIES".to_string(),
            properties.len().to_string(),
        ]);
        params.extend(properties.iter().map(|property| property.to_string()));

        self.send_command("GRAPH.CONSTRAINT", Some("DROP"), Some(params.as_slice()))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::open_test_graph_async, IndexType};

    #[tokio::test(flavor = "multi_thread")]
    async fn test_create_drop_index_async() {
        let mut graph = open_test_graph_async("test_create_drop_index_async").await;
        graph
            .inner
            .create_index(
                IndexType::Fulltext,
                EntityType::Node,
                "actor".to_string(),
                &["Hello"],
                None,
            )
            .await
            .expect("Could not create index");

        let indices = graph
            .inner
            .list_indices()
            .await
            .expect("Could not list indices");

        assert_eq!(indices.data.len(), 2);
        assert_eq!(
            indices.data[0].field_types["Hello"],
            vec![IndexType::Fulltext]
        );

        graph
            .inner
            .drop_index(
                IndexType::Fulltext,
                EntityType::Node,
                "actor".to_string(),
                &["Hello"],
            )
            .await
            .expect("Could not drop index");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_list_indices_async() {
        let mut graph = open_test_graph_async("test_list_indices_async").await;
        let indices = graph
            .inner
            .list_indices()
            .await
            .expect("Could not list indices");

        assert_eq!(indices.data.len(), 1);
        assert_eq!(indices.data[0].entity_type, EntityType::Node);
        assert_eq!(indices.data[0].index_label, "actor".to_string());
        assert_eq!(indices.data[0].field_types.len(), 2);
        assert_eq!(
            indices.data[0].field_types,
            HashMap::from([
                ("age".to_string(), vec![IndexType::Range]),
                ("name".to_string(), vec![IndexType::Fulltext])
            ])
        );

        graph.inner.delete().await.ok();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_create_drop_mandatory_constraint_async() {
        let graph = open_test_graph_async("test_mandatory_constraint_async").await;

        graph
            .inner
            .create_mandatory_constraint(EntityType::Edge, "act", &["hello", "goodbye"])
            .await
            .expect("Could not create constraint");

        graph
            .inner
            .drop_constraint(
                ConstraintType::Mandatory,
                EntityType::Edge,
                "act",
                &["hello", "goodbye"],
            )
            .await
            .expect("Could not drop constraint");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_create_drop_unique_constraint_async() {
        let mut graph = open_test_graph_async("test_unique_constraint_async").await;

        graph
            .inner
            .create_unique_constraint(
                EntityType::Node,
                "actor".to_string(),
                &["first_name", "last_name"],
            )
            .await
            .expect("Could not create constraint");

        graph
            .inner
            .drop_constraint(
                ConstraintType::Unique,
                EntityType::Node,
                "actor",
                &["first_name", "last_name"],
            )
            .await
            .expect("Could not drop constraint");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_list_constraints_async() {
        let mut graph = open_test_graph_async("test_list_constraint_async").await;

        graph
            .inner
            .create_unique_constraint(
                EntityType::Node,
                "actor".to_string(),
                &["first_name", "last_name"],
            )
            .await
            .expect("Could not create constraint");

        let constraints = graph
            .inner
            .list_constraints()
            .await
            .expect("Could not list constraints");
        assert_eq!(constraints.data.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_slowlog_async() {
        let mut graph = open_test_graph_async("test_slowlog_async").await;

        graph
            .inner
            .query("UNWIND range(0, 500) AS x RETURN x")
            .await
            .expect("Could not generate the fast query");
        graph
            .inner
            .query("UNWIND range(0, 100000) AS x RETURN x")
            .await
            .expect("Could not generate the slow query");

        let slowlog = graph
            .inner
            .slowlog()
            .await
            .expect("Could not get slowlog entries");

        assert_eq!(slowlog.len(), 2);
        assert_eq!(
            slowlog[0].arguments,
            "UNWIND range(0, 500) AS x RETURN x".to_string()
        );
        assert_eq!(
            slowlog[1].arguments,
            "UNWIND range(0, 100000) AS x RETURN x".to_string()
        );

        graph
            .inner
            .slowlog_reset()
            .await
            .expect("Could not reset slowlog memory");
        let slowlog_after_reset = graph
            .inner
            .slowlog()
            .await
            .expect("Could not get slowlog entries after reset");
        assert!(slowlog_after_reset.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_explain_async() {
        let graph = open_test_graph_async("test_explain_async").await;

        let execution_plan = graph.inner.explain("MATCH (a:actor) WITH a MATCH (b:actor) WHERE a.age = b.age AND a <> b RETURN a, collect(b) LIMIT 100").await.expect("Could not create execution plan");
        assert_eq!(execution_plan.steps().len(), 7);
        assert_eq!(
            execution_plan.text(),
            "\nResults\n    Limit\n        Aggregate\n            Filter\n                Node By Index Scan | (b:actor)\n                    Project\n                        Node By Label Scan | (a:actor)"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_profile_async() {
        let graph = open_test_graph_async("test_profile_async").await;

        let execution_plan = graph
            .inner
            .profile("UNWIND range(0, 1000) AS x RETURN x")
            .await
            .expect("Could not generate the query");

        let steps = execution_plan.steps().to_vec();
        assert_eq!(steps.len(), 3);

        let expected = vec!["Results", "Project", "Unwind"];
        for (step, expected) in steps.into_iter().zip(expected) {
            assert!(step.starts_with(expected));
            assert!(step.ends_with("ms"));
        }
    }
}
