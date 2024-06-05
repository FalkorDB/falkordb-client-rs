/*
 * Copyright FalkorDB Ltd. 2023 - present
 * Licensed under the Server Side Public License v1 (SSPLv1).
 */

use crate::{FalkorDBError, FalkorParsable, FalkorResult, FalkorValue, GraphSchema, SchemaType};
use std::collections::{HashMap, HashSet};

/// Whether this element is a node or edge in the graph
#[derive(Copy, Clone, Debug, Eq, PartialEq, strum::EnumString, strum::Display)]
#[strum(serialize_all = "UPPERCASE")]
pub enum EntityType {
    /// A node in the graph
    Node,
    /// An edge in the graph, meaning a relationship between two nodes
    #[strum(serialize = "RELATIONSHIP")]
    Edge,
}

/// A node in the graph, containing a unique id, various labels describing it, and its own property.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    /// The internal entity ID
    pub entity_id: i64,
    /// A [`Vec`] of the labels this node answers to
    pub labels: Vec<String>,
    /// A [`HashMap`] of the properties in key-val form
    pub properties: HashMap<String, FalkorValue>,
}

impl FalkorParsable for Node {
    fn from_falkor_value(
        value: FalkorValue,
        graph_schema: &mut GraphSchema,
    ) -> FalkorResult<Self> {
        let [entity_id, labels, properties]: [FalkorValue; 3] =
            value.into_vec()?.try_into().map_err(|_| {
                FalkorDBError::ParsingArrayToStructElementCount(
                    "Expected exactly 3 elements in node object".to_string(),
                )
            })?;
        let labels = labels.into_vec()?;

        let mut ids_hashset = HashSet::with_capacity(labels.len());
        for label in labels.iter() {
            ids_hashset.insert(
                label
                    .to_i64()
                    .ok_or(FalkorDBError::ParsingCompactIdUnknown)?,
            );
        }
        Ok(Node {
            entity_id: entity_id.to_i64().ok_or(FalkorDBError::ParsingI64)?,
            labels: graph_schema.parse_id_vec(labels, SchemaType::Labels)?,
            properties: graph_schema.parse_properties_map(properties)?,
        })
    }
}

/// An edge in the graph, representing a relationship between two [`Node`]s.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edge {
    /// The internal entity ID
    pub entity_id: i64,
    /// What type is this relationship
    pub relationship_type: String,
    /// The entity ID of the origin node
    pub src_node_id: i64,
    /// The entity ID of the destination node
    pub dst_node_id: i64,
    /// A [`HashMap`] of the properties in key-val form
    pub properties: HashMap<String, FalkorValue>,
}

impl FalkorParsable for Edge {
    fn from_falkor_value(
        value: FalkorValue,
        graph_schema: &mut GraphSchema,
    ) -> FalkorResult<Self> {
        let [entity_id, relations, src_node_id, dst_node_id, properties]: [FalkorValue; 5] =
            value.into_vec()?.try_into().map_err(|_| {
                FalkorDBError::ParsingArrayToStructElementCount(
                    "Expected exactly 5 elements in edge object".to_string(),
                )
            })?;

        let relation = relations.to_i64().ok_or(FalkorDBError::ParsingI64)?;
        let relationship = graph_schema
            .relationships()
            .get(&relation)
            .ok_or(FalkorDBError::MissingSchemaId(SchemaType::Relationships))?;

        Ok(Edge {
            entity_id: entity_id.to_i64().ok_or(FalkorDBError::ParsingI64)?,
            relationship_type: relationship.to_string(),
            src_node_id: src_node_id.to_i64().ok_or(FalkorDBError::ParsingI64)?,
            dst_node_id: dst_node_id.to_i64().ok_or(FalkorDBError::ParsingI64)?,
            properties: graph_schema.parse_properties_map(properties)?,
        })
    }
}
