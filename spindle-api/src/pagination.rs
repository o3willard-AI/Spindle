//! spindle-api: Cursor-based pagination for REST API endpoints.
//!
//! Implements opaque base64-encoded cursor pagination with deterministic
//! ordering. Designed for keyset/pagination-by-sort-key pattern to avoid
//! N+1 performance degradation on deep pages.

use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Cursor encoding ─────────────────────────────────────────────────────

/// Opaque cursor encoding `(sort_field_value, id, direction)`.
///
/// Wire format (base64-encoded JSON):
///   {"v":"<sort_field_value>","i":"<uuid>","d":"asc|desc"}
///
/// This encoding is intentionally not meaningful to clients — it can
/// change between releases without breaking cursor compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CursorPayload {
    v: String, // sort field value
    i: String, // UUID as string
    d: String, // "asc" or "desc"
}

/// Encode pagination cursor from (sort_field_value, id, direction).
pub fn encode_cursor(
    sort_field_value: &str,
    id: Uuid,
    direction: &str,
) -> String {
    let payload = CursorPayload {
        v: sort_field_value.to_string(),
        i: id.to_string(),
        d: direction.to_string(),
    };
    let json = serde_json::to_string(&payload).expect("CursorPayload must serialize");
    STANDARD.encode(json.as_bytes())
}

/// Decode pagination cursor. Returns `None` if the cursor is malformed.
pub fn decode_cursor(cursor: &str) -> Option<(String, Uuid, String)> {
    let bytes = STANDARD.decode(cursor).ok()?;
    let payload: CursorPayload = serde_json::from_slice(&bytes).ok()?;

    // Validate direction
    if payload.d != "asc" && payload.d != "desc" {
        return None;
    }

    // Validate UUID
    let id = Uuid::parse_str(&payload.i).ok()?;

    Some((payload.v, id, payload.d))
}

// ── Pagination parameters ───────────────────────────────────────────────

/// Maximum number of rows returned per page.
const MAX_LIMIT: usize = 1000;
/// Default number of rows per page.
const DEFAULT_LIMIT: usize = 50;

/// Pagination parameters extracted from the query string.
#[derive(Debug, Clone)]
pub struct PaginationParams {
    /// Requested page size (capped at `MAX_LIMIT`).
    pub limit: usize,
    /// Opaque cursor from previous response.
    pub cursor: Option<String>,
    /// Sort field and direction for deterministic ordering.
    pub sort_field: String,
    pub sort_direction: String, // "asc" or "desc"
}

impl Default for PaginationParams {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            cursor: None,
            sort_field: "id".to_string(),
            sort_direction: "asc".to_string(),
        }
    }
}

/// Parse pagination parameters from a query string.
pub fn parse_pagination(
    query: &str,
    default_sort_field: &str,
) -> Result<PaginationParams, String> {
    let mut limit = DEFAULT_LIMIT;
    let mut cursor: Option<String> = None;
    let mut sort_field = default_sort_field.to_string();
    let mut sort_direction = "asc".to_string();

    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }

        let (key, value) = if let Some(pos) = pair.find('=') {
            (&pair[..pos], &pair[pos + 1..])
        } else {
            (pair, "")
        };

        let key = urlencoding::decode(key).unwrap_or_default().into_owned();
        let value = urlencoding::decode(value).unwrap_or_default().into_owned();

        match key.as_str() {
            "limit" => {
                match value.parse::<usize>() {
                    Ok(l) => {
                        if l == 0 {
                            return Err("limit must be >= 1".to_string());
                        }
                        limit = l.min(MAX_LIMIT);
                    }
                    Err(_) => return Err(format!("Invalid limit: {value}")),
                }
            }
            "cursor" => {
                if !value.is_empty() {
                    cursor = Some(value);
                }
            }
            "sort" => {
                if let Some(pos) = value.find(':') {
                    let field = &value[..pos];
                    let dir = &value[pos + 1..];
                    if !field.is_empty() && (dir == "asc" || dir == "desc") {
                        sort_field = field.to_string();
                        sort_direction = dir.to_string();
                    }
                } else if !value.is_empty() {
                    sort_field = value;
                }
            }
            _ => {
                // Unknown query param — ignore silently
            }
        }
    }

    Ok(PaginationParams {
        limit,
        cursor,
        sort_field,
        sort_direction,
    })
}

// ── Pagination result ───────────────────────────────────────────────────

/// Paginated response envelope returned by all list endpoints.
///
/// - `total_count` — scoped count (all matching rows, not just this page)
/// - `has_more` — true if there are more pages after this one
/// - `next_cursor` — opaque cursor for the next page; `None` on last page
///
/// Items are omitted here intentionally — each endpoint attaches its own
/// item type via a wrapper struct.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaginationResult {
    pub total_count: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

impl PaginationResult {
    /// Build a `PaginationResult` from the actual row count and params.
    ///
    /// `actual_rows` is the number of rows returned in this page.
    /// If `actual_rows` exceeds `limit`, `has_more` is true and
    /// `next_cursor` is encoded.
    pub fn from_query(
        limit: usize,
        actual_rows: usize,
        total_count: usize,
        next_cursor: Option<String>,
    ) -> Self {
        let has_more = next_cursor.is_some() && actual_rows >= limit;
        Self {
            total_count,
            has_more,
            next_cursor,
        }
    }

    /// Build a pagination result when the total is not known (no COUNT query).
    /// `has_more` is true only if we hit the limit.
    pub fn from_rows(limit: usize, actual_rows: usize) -> Self {
        let has_more = actual_rows >= limit;
        Self {
            total_count: actual_rows,
            has_more,
            next_cursor: if has_more {
                // Cursor will be set by the caller
                None
            } else {
                None
            },
        }
    }

    /// Build a `PaginationResult` with explicit next cursor.
    pub fn with_next_cursor(mut self, next_cursor: Option<String>) -> Self {
        self.has_more = next_cursor.is_some();
        self.next_cursor = next_cursor;
        self
    }
}

// ── Deterministic ordering helper ───────────────────────────────────────

/// Generate an ORDER BY clause with tiebreaker for deterministic results.
///
/// Format: `ORDER BY <sort_field> <direction>, id <direction>`
/// The `id` tiebreaker ensures deterministic ordering when sort_field
/// values are duplicated.
pub fn deterministic_order_by(sort_field: &str, direction: &str) -> String {
    let dir = if direction == "desc" { "DESC" } else { "ASC" };
    format!("{sort_field} {dir}, id {dir}")
}

/// Generate a cursor-bearing WHERE clause for keyset pagination.
///
/// For ASC: `WHERE (sort_field, id) > (last_value, last_id)`
/// For DESC: `WHERE (sort_field, id) < (last_value, last_id)`
///
/// Returns `(operator, placeholder_values)`.
pub fn cursor_where_clause(
    sort_field: &str,
    direction: &str,
    cursor_val: &str,
    cursor_id: &str,
) -> (&'static str, Vec<String>) {
    let cursor_tuple = format!("('{cursor_val}', '{cursor_id}')");
    let operator = if direction == "desc" { "<" } else { ">" };
    let values = vec![
        format!("{sort_field}, id {operator} {cursor_tuple}"),
    ];
    (operator, values)
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Cursor encoding/decoding ─────────────────────────────────────────

    #[test]
    fn test_encode_decode_cursor() {
        let id = Uuid::nil();
        let cursor = encode_cursor("ubuntu", id, "asc");
        let decoded = decode_cursor(&cursor);
        assert!(decoded.is_some());
        let (v, i, d) = decoded.unwrap();
        assert_eq!(v, "ubuntu");
        assert_eq!(i, id);
        assert_eq!(d, "asc");
    }

    #[test]
    fn test_cursor_direction_desc() {
        let id = Uuid::new_v4();
        let cursor = encode_cursor("2026-01-01", id, "desc");
        let (v, i, d) = decode_cursor(&cursor).unwrap();
        assert_eq!(v, "2026-01-01");
        assert_eq!(i, id);
        assert_eq!(d, "desc");
    }

    #[test]
    fn test_decode_cursor_invalid_base64() {
        assert!(decode_cursor("!!!not-base64!!!").is_none());
    }

    #[test]
    fn test_decode_cursor_malformed_json() {
        let bad_b64 = STANDARD.encode(b"not json");
        assert!(decode_cursor(&bad_b64).is_none());
    }

    #[test]
    fn test_decode_cursor_bad_uuid() {
        let payload = CursorPayload {
            v: "val".to_string(),
            i: "not-a-uuid".to_string(),
            d: "asc".to_string(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let b64 = STANDARD.encode(json.as_bytes());
        assert!(decode_cursor(&b64).is_none());
    }

    #[test]
    fn test_decode_cursor_bad_direction() {
        let payload = CursorPayload {
            v: "val".to_string(),
            i: "00000000-0000-0000-0000-000000000000".to_string(),
            d: "banana".to_string(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let b64 = STANDARD.encode(json.as_bytes());
        assert!(decode_cursor(&b64).is_none());
    }

    // ── Pagination params ───────────────────────────────────────────────

    #[test]
    fn test_parse_pagination_defaults() {
        let params = parse_pagination("", "id").unwrap();
        assert_eq!(params.limit, DEFAULT_LIMIT);
        assert!(params.cursor.is_none());
        assert_eq!(params.sort_field, "id");
        assert_eq!(params.sort_direction, "asc");
    }

    #[test]
    fn test_parse_pagination_limit() {
        let params = parse_pagination("limit=25", "id").unwrap();
        assert_eq!(params.limit, 25);
    }

    #[test]
    fn test_parse_pagination_limit_capped() {
        let params = parse_pagination("limit=9999", "id").unwrap();
        assert_eq!(params.limit, MAX_LIMIT);
    }

    #[test]
    fn test_parse_pagination_limit_zero() {
        let result = parse_pagination("limit=0", "id");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_pagination_cursor() {
        let id = Uuid::nil();
        let cursor = encode_cursor("ubuntu", id, "asc");
        let params = parse_pagination(&format!("cursor={}", cursor), "id").unwrap();
        assert_eq!(params.cursor, Some(cursor));
    }

    #[test]
    fn test_parse_pagination_sort() {
        let params = parse_pagination("sort=last_seen:desc", "id").unwrap();
        assert_eq!(params.sort_field, "last_seen");
        assert_eq!(params.sort_direction, "desc");
    }

    #[test]
    fn test_parse_pagination_sort_no_direction() {
        let params = parse_pagination("sort=last_seen", "id").unwrap();
        assert_eq!(params.sort_field, "last_seen");
        assert_eq!(params.sort_direction, "asc");
    }

    // ── Pagination result ───────────────────────────────────────────────

    #[test]
    fn test_pagination_result_has_more_true() {
        let limit = 50;
        let actual_rows = 50;
        let next_cursor = Some("abc123".to_string());
        let result = PaginationResult::from_query(limit, actual_rows, 200, next_cursor.clone());
        assert!(result.has_more);
        assert_eq!(result.next_cursor, next_cursor);
        assert_eq!(result.total_count, 200);
    }

    #[test]
    fn test_pagination_result_has_more_false_last_page() {
        let limit = 50;
        let actual_rows = 30;
        let next_cursor = None;
        let result = PaginationResult::from_query(limit, actual_rows, 100, next_cursor.clone());
        assert!(!result.has_more);
        assert_eq!(result.next_cursor, next_cursor);
    }

    #[test]
    fn test_pagination_result_has_more_false_exact_page() {
        let limit = 50;
        let actual_rows = 50;
        let next_cursor = None;
        let result = PaginationResult::from_query(limit, actual_rows, 100, next_cursor.clone());
        assert!(!result.has_more);
        assert_eq!(result.next_cursor, next_cursor);
    }

    // ── Deterministic ordering ──────────────────────────────────────────

    #[test]
    fn test_deterministic_order_by_asc() {
        let clause = deterministic_order_by("last_seen", "asc");
        assert_eq!(clause, "last_seen ASC, id ASC");
    }

    #[test]
    fn test_deterministic_order_by_desc() {
        let clause = deterministic_order_by("last_seen", "desc");
        assert_eq!(clause, "last_seen DESC, id DESC");
    }

    // ── Cursor WHERE clause ─────────────────────────────────────────────

    #[test]
    fn test_cursor_where_clause_asc() {
        let (op, vals) = cursor_where_clause("last_seen", "asc", "2026-01-01", "abc-def");
        assert_eq!(op, ">");
        assert_eq!(
            vals[0],
            "last_seen, id > ('2026-01-01', 'abc-def')".to_string()
        );
    }

    #[test]
    fn test_cursor_where_clause_desc() {
        let (op, _vals) = cursor_where_clause("last_seen", "desc", "2026-01-01", "abc-def");
        assert_eq!(op, "<");
    }
}