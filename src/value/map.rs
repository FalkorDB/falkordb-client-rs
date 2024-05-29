/*
 * Copyright FalkorDB Ltd. 2023 - present
 * Licensed under the Server Side Public License v1 (SSPLv1).
 */

use crate::{
    connection::blocking::BorrowedSyncConnection, value::utils::parse_type, FalkorDBError,
    FalkorParsable, FalkorResult, FalkorValue, GraphSchema, SchemaType,
};
use std::collections::HashMap;

// Intermediate type for map parsing
pub(crate) struct FKeyTypeVal {
    key: i64,
    type_marker: i64,
    val: FalkorValue,
}

impl TryFrom<FalkorValue> for FKeyTypeVal {
    type Error = FalkorDBError;

    fn try_from(value: FalkorValue) -> FalkorResult<Self> {
        let [key_raw, type_raw, val]: [FalkorValue; 3] = value
            .into_vec()?
            .try_into()
            .map_err(|_| FalkorDBError::ParsingArrayToStructElementCount)?;

        let key = key_raw.to_i64();
        let type_marker = type_raw.to_i64();

        match (key, type_marker) {
            (Some(key), Some(type_marker)) => Ok(FKeyTypeVal {
                key,
                type_marker,
                val,
            }),
            (Some(_), None) => Err(FalkorDBError::ParsingTypeMarkerTypeMismatch)?,
            (None, Some(_)) => Err(FalkorDBError::ParsingKeyIdTypeMismatch)?,
            _ => Err(FalkorDBError::ParsingKTVTypes)?,
        }
    }
}

fn ktv_vec_to_map(
    map_vec: Vec<FKeyTypeVal>,
    relevant_ids_map: HashMap<i64, String>,
    graph_schema: &mut GraphSchema,
    conn: &mut BorrowedSyncConnection,
) -> FalkorResult<HashMap<String, FalkorValue>> {
    let mut new_map = HashMap::with_capacity(map_vec.len());
    for fktv in map_vec {
        new_map.insert(
            relevant_ids_map
                .get(&fktv.key)
                .cloned()
                .ok_or(FalkorDBError::ParsingError)?,
            parse_type(fktv.type_marker, fktv.val, graph_schema, conn)?,
        );
    }

    Ok(new_map)
}

pub(crate) fn parse_map_with_schema(
    value: FalkorValue,
    graph_schema: &mut GraphSchema,
    conn: &mut BorrowedSyncConnection,
    schema_type: SchemaType,
) -> FalkorResult<HashMap<String, FalkorValue>> {
    let (id_hashset, map_vec) = value
        .into_vec()?
        .into_iter()
        .flat_map(FKeyTypeVal::try_from)
        .map(|fktv| (fktv.key, fktv))
        .unzip();

    if let Some(relevant_ids_map) = graph_schema.verify_id_set(&id_hashset, schema_type) {
        return ktv_vec_to_map(map_vec, relevant_ids_map, graph_schema, conn);
    }

    // If we reached here, schema validation failed and we need to refresh our schema
    match graph_schema.refresh(conn, schema_type, Some(&id_hashset))? {
        Some(relevant_ids_map) => ktv_vec_to_map(map_vec, relevant_ids_map, graph_schema, conn),
        None => Err(FalkorDBError::ParsingError)?,
    }
}

impl FalkorParsable for HashMap<String, FalkorValue> {
    fn from_falkor_value(
        value: FalkorValue,
        graph_schema: &mut GraphSchema,
        conn: &mut BorrowedSyncConnection,
    ) -> FalkorResult<Self> {
        let val_vec = value.into_vec()?;
        if val_vec.len() % 2 != 0 {
            Err(FalkorDBError::ParsingFMap)?;
        }

        Ok(val_vec
            .chunks_exact(2)
            .flat_map(|pair| {
                let [key, val]: [FalkorValue; 2] = pair
                    .to_vec()
                    .try_into()
                    .map_err(|_| FalkorDBError::ParsingFMap)?;

                let [type_marker, val]: [FalkorValue; 2] = val
                    .into_vec()?
                    .try_into()
                    .map_err(|_| FalkorDBError::ParsingFMap)?;

                FalkorResult::<_>::Ok((
                    key.into_string()?,
                    parse_type(
                        type_marker.to_i64().ok_or(FalkorDBError::ParsingKTVTypes)?,
                        val,
                        graph_schema,
                        conn,
                    )?,
                ))
            })
            .collect())
    }
}
