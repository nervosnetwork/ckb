use super::*;

use ckb_jsonrpc_types::IndexerOrder;

pub(crate) const COUNT_COLUMN: &str = "count";

pub struct PaginationRequest {
    pub cursor: Option<u64>,
    pub order: IndexerOrder,
    pub limit: Option<u16>,
    pub skip: Option<u64>,
    pub return_count: bool,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct PaginationResponse<T> {
    pub response: Vec<T>,
    pub next_cursor: Option<u64>,
    pub count: Option<u64>,
}

pub(crate) fn _build_query_page_sql(
    mut query_builder: SqlBuilder,
    pagination: &PaginationRequest,
) -> Result<(String, String)> {
    let sql_sub_query = query_builder.subquery()?;

    if let Some(id) = pagination.cursor {
        let id = i64::try_from(id).unwrap_or(i64::MAX);
        match pagination.order {
            IndexerOrder::Asc => query_builder.and_where_gt("id", id),
            IndexerOrder::Desc => query_builder.and_where_lt("id", id),
        };
    }
    match pagination.order {
        IndexerOrder::Asc => query_builder.order_by("id", false),
        IndexerOrder::Desc => query_builder.order_by("id", true),
    };
    query_builder.limit(pagination.limit.unwrap_or(u16::MAX));
    if let Some(skip) = pagination.skip {
        let offset = i64::try_from(skip).unwrap_or(i64::MAX);
        query_builder.offset(offset);
    }

    let query = query_builder.sql()?.trim_end_matches(';').to_string();
    let sub_query_for_count = fetch_count_sql(&format!("{} res", sql_sub_query));

    Ok((query, sub_query_for_count))
}

pub fn build_next_cursor(
    limit: u16,
    last_id: u64,
    records_size: usize,
    total: Option<u64>,
) -> Option<u64> {
    let mut next_cursor = None;
    if records_size == limit as usize {
        if let Some(total) = total {
            if total > limit as u64 {
                next_cursor = Some(last_id)
            }
        } else {
            next_cursor = Some(last_id);
        }
    }
    next_cursor
}
