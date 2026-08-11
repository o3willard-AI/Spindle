//! spindle-api: Filter grammar for REST API list endpoints.
//!
//! Defines the shared filter model and query-string parser used by all
//! GET endpoints (nodes, runs, resource-events, compliance, etc.).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

// ── Valid fields per resource type ──────────────────────────────────────

/// List of valid field names for each resource type.
/// Used to validate filters and return helpful 400 messages.
pub const VALID_NODE_FIELDS: &[&str] = &[
    "id", "name", "platform", "chef_environment", "policy_group",
    "policy_name", "run_list", "last_seen", "first_seen", "status",
];

pub const VALID_RUN_FIELDS: &[&str] = &[
    "id", "node_id", "status", "start_time", "end_time", "cookbook",
    "duration_ms", "platform",
];

pub const VALID_RESOURCE_EVENT_FIELDS: &[&str] = &[
    "id", "run_id", "node_id", "resource_type", "resource_name",
    "action", "status", "duration_ms", "cookbook_name", "cookbook_version",
    "platform",
];

pub const VALID_COMPLIANCE_REPORT_FIELDS: &[&str] = &[
    "id", "node_id", "profile_name", "status", "start_time", "end_time",
    "platform",
];

/// Valid fields for waiver entity filtering (M2-07).
pub const VALID_WAIVER_FIELDS: &[&str] = &[
    "id", "control_id", "scope", "justification", "approver",
    "start_date", "expiry_date",
];

/// Valid fields for cookbook entity filtering (M2-08).
pub const VALID_COOKBOOK_FIELDS: &[&str] = &[
    "name", "version", "node_id", "first_seen", "last_seen", "node_count",
];

// ── Filter operator ─────────────────────────────────────────────────────

/// Comparison operators for filter clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
    Like,
    Between,
    IsNull,
}

impl FilterOp {
    /// Parse a filter operator from a query-string fragment.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "eq" => Some(Self::Eq),
            "neq" => Some(Self::Neq),
            "gt" => Some(Self::Gt),
            "gte" => Some(Self::Gte),
            "lt" => Some(Self::Lt),
            "lte" => Some(Self::Lte),
            "in" => Some(Self::In),
            "like" => Some(Self::Like),
            "between" => Some(Self::Between),
            "is_null" => Some(Self::IsNull),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Neq => "neq",
            Self::Gt => "gt",
            Self::Gte => "gte",
            Self::Lt => "lt",
            Self::Lte => "lte",
            Self::In => "in",
            Self::Like => "like",
            Self::Between => "between",
            Self::IsNull => "is_null",
        }
    }
}

// ── Filter value ────────────────────────────────────────────────────────

/// The value side of a filter clause. Single values are `Value`,
/// multi-value operators (`in`, `between`) carry `Vec<Value>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub enum FilterValue {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Timestamp(DateTime<Utc>),
    /// For `in` / `between` operators
    List(Vec<String>),
}

impl std::fmt::Display for FilterValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Str(s) => write!(f, "{s}"),
            Self::Int(n) => write!(f, "{n}"),
            Self::Float(n) => write!(f, "{n}"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Timestamp(dt) => write!(f, "{}", dt.to_rfc3339()),
            Self::List(items) => write!(f, "{}", items.join(",")),
        }
    }
}

/// A single filter clause: `field operator value`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct Filter {
    pub field: String,
    pub operator: FilterOp,
    pub value: Option<FilterValue>,
}

/// Parse a filter value string into the appropriate `FilterValue` variant.
pub fn parse_filter_value(s: &str, expected_list: bool) -> Result<FilterValue, String> {
    if expected_list {
        let parts: Vec<String> = s
            .split(',')
            .map(|p| urlencoding::decode(p).unwrap_or_default().into_owned())
            .collect();
        if parts.is_empty() {
            return Ok(FilterValue::List(vec![]));
        }
        return Ok(FilterValue::List(parts));
    }
    let decoded = urlencoding::decode(s).unwrap_or_default().into_owned();
    if decoded.eq_ignore_ascii_case("true") {
        return Ok(FilterValue::Bool(true));
    }
    if decoded.eq_ignore_ascii_case("false") {
        return Ok(FilterValue::Bool(false));
    }
    if let Ok(n) = decoded.parse::<i64>() {
        return Ok(FilterValue::Int(n));
    }
    if let Ok(n) = decoded.parse::<f64>() {
        return Ok(FilterValue::Float(n));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(&decoded) {
        return Ok(FilterValue::Timestamp(dt.with_timezone(&Utc)));
    }
    Ok(FilterValue::Str(decoded))
}

// ── Time range ──────────────────────────────────────────────────────────

/// Optional time-range filter (RFC 3339 datetimes).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct TimeRange {
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
}

impl TimeRange {
    /// Parse RFC 3339 strings into a `TimeRange`.
    pub fn parse(start: Option<String>, end: Option<String>) -> Result<Self, String> {
        let start_time = match start {
            Some(ref s) => Some(
                DateTime::parse_from_rfc3339(s)
                    .map_err(|e| format!("Invalid start_time: {e}"))?
                    .with_timezone(&Utc),
            ),
            None => None,
        };
        let end_time = match end {
            Some(ref s) => Some(
                DateTime::parse_from_rfc3339(s)
                    .map_err(|e| format!("Invalid end_time: {e}"))?
                    .with_timezone(&Utc),
            ),
            None => None,
        };
        Ok(Self {
            start_time,
            end_time,
        })
    }
}

// ── Sort ────────────────────────────────────────────────────────────────

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "asc" | "ascending" => Some(Self::Asc),
            "desc" | "descending" => Some(Self::Desc),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }

    pub fn sql(&self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

/// A sort clause: `field direction`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct Sort {
    pub field: String,
    pub direction: SortDirection,
}

/// Parse a sort string in the form `field:direction` (e.g. `start_time:desc`).
/// If no colon, direction defaults to Asc.
pub fn parse_sort(s: &str) -> Result<Sort, String> {
    let (field, dir_str) = if let Some(pos) = s.find(':') {
        (&s[..pos], &s[pos + 1..])
    } else {
        (s, "asc")
    };
    let direction = SortDirection::from_str(dir_str)
        .ok_or_else(|| format!("Invalid sort direction: {dir_str}. Use 'asc' or 'desc'."))?;
    if field.is_empty() {
        return Err("Sort field must not be empty".to_string());
    }
    Ok(Sort {
        field: field.to_string(),
        direction,
    })
}

// ── Error ───────────────────────────────────────────────────────────────

/// Errors returned by the filter parser.
#[derive(Debug, Error)]
pub enum FilterError {
    #[error("Unknown field '{0}': valid fields are {1}")]
    UnknownField(String, String),

    #[error("Invalid filter syntax: {0}")]
    InvalidSyntax(String),

    #[error("Invalid value for field '{field}': {reason}")]
    InvalidValue { field: String, reason: String },
}

// ── Query filter (aggregated) ───────────────────────────────────────────

/// All filter/sort/time-range constraints for a single API endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QueryFilter {
    pub filters: Vec<Filter>,
    pub time_range: TimeRange,
    pub sort: Option<Sort>,
}

/// Validate that all filter fields are in the allowed list.
pub fn validate_filter_fields(
    filters: &[Filter],
    time_range: &TimeRange,
    allowed: &[&str],
) -> Result<(), FilterError> {
    let _ = time_range; // time range fields are always valid
    let allowed_set: HashSet<&str> = allowed.iter().copied().collect();
    for f in filters {
        if !allowed_set.contains(f.field.as_str()) {
            let valid_list = allowed.join(", ");
            return Err(FilterError::UnknownField(f.field.clone(), valid_list));
        }
    }
    Ok(())
}

// ── Query string parser ─────────────────────────────────────────────────

/// Parse a query string into a `QueryFilter`.
///
/// Supported formats:
/// - `?filter[field]=value` — uses `eq` operator
/// - `?filter[field:op]=value` — explicit operator
/// - `?sort=field:direction` — sort clause
/// - `?since=RFC3339` / `?until=RFC3339` — time range
///
/// Example:
///   ?filter[platform]=ubuntu&filter[cputime:gt]=100&sort=last_seen:desc&since=2026-01-01T00:00:00Z
pub fn parse_query_string(
    query: &str,
    allowed_fields: &[&str],
) -> Result<QueryFilter, FilterError> {
    let mut filters = Vec::new();
    let mut start_time: Option<String> = None;
    let mut end_time: Option<String> = None;
    let mut sort: Option<Sort> = None;

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
            "filter" => {
                if !value.is_empty() {
                    filters.push(Filter {
                        field: value.clone(),
                        operator: FilterOp::Eq,
                        value: None,
                    });
                }
            }
            _ if key.starts_with("filter[") && key.ends_with(']') => {
                let inner = &key[7..key.len() - 1]; // strip "filter[" and "]"
                let (field, op) = if let Some(pos) = inner.find(':') {
                    (
                        &inner[..pos],
                        FilterOp::from_str(&inner[pos + 1..]).unwrap_or(FilterOp::Eq),
                    )
                } else {
                    (inner, FilterOp::Eq)
                };

                let value_opt = if value.is_empty() {
                    None
                } else {
                    let expected_list = matches!(op, FilterOp::In | FilterOp::Between);
                    Some(parse_filter_value(&value, expected_list)
                        .map_err(|e| FilterError::InvalidValue { field: field.to_string(), reason: e })?)
                };

                filters.push(Filter {
                    field: field.to_string(),
                    operator: op,
                    value: value_opt,
                });
            }
            "sort" => {
                sort = Some(parse_sort(&value).map_err(FilterError::InvalidSyntax)?);
            }
            "since" | "start_time" => {
                start_time = Some(value);
            }
            "until" | "end_time" => {
                end_time = Some(value);
            }
            _ => {
                // Unknown query param — ignore silently (forward compatible)
            }
        }
    }

    let time_range = TimeRange::parse(start_time, end_time)
        .map_err(|e| FilterError::InvalidSyntax(e))?;

    validate_filter_fields(&filters, &time_range, allowed_fields)?;

    Ok(QueryFilter {
        filters,
        time_range,
        sort,
    })
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── FilterOp ─────────────────────────────────────────────────────────

    #[test]
    fn test_filter_op_round_trip() {
        for op in [
            FilterOp::Eq,
            FilterOp::Neq,
            FilterOp::Gt,
            FilterOp::Gte,
            FilterOp::Lt,
            FilterOp::Lte,
            FilterOp::In,
            FilterOp::Like,
            FilterOp::Between,
            FilterOp::IsNull,
        ] {
            let s = op.as_str();
            assert_eq!(FilterOp::from_str(s), Some(op));
        }
        assert_eq!(FilterOp::from_str("unknown"), None);
    }

    // ── FilterValue ──────────────────────────────────────────────────────

    #[test]
    fn test_parse_filter_value_str() {
        let v = parse_filter_value("ubuntu", false).unwrap();
        assert_eq!(v, FilterValue::Str("ubuntu".to_string()));
    }

    #[test]
    fn test_parse_filter_value_int() {
        let v = parse_filter_value("42", false).unwrap();
        assert_eq!(v, FilterValue::Int(42));
    }

    #[test]
    fn test_parse_filter_value_float() {
        let v = parse_filter_value("3.14", false).unwrap();
        let expected = "3.14".parse::<f64>().unwrap();
        assert_eq!(v, FilterValue::Float(expected));
    }

    #[test]
    fn test_parse_filter_value_bool() {
        let v = parse_filter_value("true", false).unwrap();
        assert_eq!(v, FilterValue::Bool(true));
        let v2 = parse_filter_value("false", false).unwrap();
        assert_eq!(v2, FilterValue::Bool(false));
    }

    #[test]
    fn test_parse_filter_value_timestamp() {
        let v = parse_filter_value("2026-01-15T10:30:00Z", false).unwrap();
        match v {
            FilterValue::Timestamp(dt) => {
                assert_eq!(dt.to_rfc3339(), "2026-01-15T10:30:00+00:00");
            }
            other => panic!("Expected Timestamp, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_filter_value_list() {
        let v = parse_filter_value("ubuntu,centos,debian", true).unwrap();
        assert_eq!(
            v,
            FilterValue::List(vec![
                "ubuntu".to_string(),
                "centos".to_string(),
                "debian".to_string(),
            ])
        );
    }

    // ── TimeRange ────────────────────────────────────────────────────────

    #[test]
    fn test_time_range_parse() {
        let tr = TimeRange::parse(
            Some("2026-01-01T00:00:00Z".to_string()),
            Some("2026-12-31T23:59:59Z".to_string()),
        )
        .unwrap();
        assert!(tr.start_time.is_some());
        assert!(tr.end_time.is_some());
    }

    #[test]
    fn test_time_range_invalid() {
        let result = TimeRange::parse(Some("not-a-date".to_string()), None);
        assert!(result.is_err());
    }

    // ── Sort ─────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_sort_with_direction() {
        let s = parse_sort("start_time:desc").unwrap();
        assert_eq!(s.field, "start_time");
        assert_eq!(s.direction, SortDirection::Desc);
    }

    #[test]
    fn test_parse_sort_default_direction() {
        let s = parse_sort("last_seen").unwrap();
        assert_eq!(s.field, "last_seen");
        assert_eq!(s.direction, SortDirection::Asc);
    }

    #[test]
    fn test_parse_sort_invalid_direction() {
        assert!(parse_sort("field:banana").is_err());
    }

    #[test]
    fn test_parse_sort_empty_field() {
        assert!(parse_sort(":desc").is_err());
    }

    #[test]
    fn test_sort_direction_sql() {
        assert_eq!(SortDirection::Asc.sql(), "ASC");
        assert_eq!(SortDirection::Desc.sql(), "DESC");
    }

    // ── Query string parser ─────────────────────────────────────────────

    #[test]
    fn test_parse_query_filter_field_eq() {
        let qf = parse_query_string("filter[platform]=ubuntu", VALID_NODE_FIELDS).unwrap();
        assert_eq!(qf.filters.len(), 1);
        assert_eq!(qf.filters[0].field, "platform");
        assert_eq!(qf.filters[0].operator, FilterOp::Eq);
        assert_eq!(
            qf.filters[0].value,
            Some(FilterValue::Str("ubuntu".to_string()))
        );
    }

    #[test]
    fn test_parse_query_filter_explicit_operator() {
        let qf = parse_query_string(
            "filter[name:gt]=zulu&filter[platform:in]=ubuntu,centos",
            VALID_NODE_FIELDS,
        )
        .unwrap();
        assert_eq!(qf.filters.len(), 2);
        assert_eq!(qf.filters[0].field, "name");
        assert_eq!(qf.filters[0].operator, FilterOp::Gt);
        assert_eq!(qf.filters[0].value, Some(FilterValue::Str("zulu".to_string())));
        assert_eq!(qf.filters.len(), 2);
        assert_eq!(qf.filters[0].field, "name");
        assert_eq!(qf.filters[0].operator, FilterOp::Gt);
        assert_eq!(qf.filters[0].value, Some(FilterValue::Str("zulu".to_string())));

        assert_eq!(qf.filters[1].field, "platform");
        assert_eq!(qf.filters[1].operator, FilterOp::In);
        assert_eq!(
            qf.filters[1].value,
            Some(FilterValue::List(vec!["ubuntu".to_string(), "centos".to_string(),]))
        );
    }

    #[test]
    fn test_parse_query_sort_and_time_range() {
        let qf = parse_query_string(
            "sort=start_time:desc&since=2026-01-01T00:00:00Z&until=2026-12-31T23:59:59Z",
            VALID_RUN_FIELDS,
        )
        .unwrap();
        assert_eq!(qf.sort.as_ref().unwrap().field, "start_time");
        assert_eq!(
            qf.sort.as_ref().unwrap().direction,
            SortDirection::Desc
        );
        assert!(qf.time_range.start_time.is_some());
        assert!(qf.time_range.end_time.is_some());
    }

    #[test]
    fn test_parse_query_unknown_field_400() {
        let result = parse_query_string("filter[nonexistent]=value", VALID_NODE_FIELDS);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"));
        assert!(err.contains("valid fields"));
    }

    #[test]
    fn test_parse_query_multiple_filters() {
        let qf = parse_query_string(
            "filter[platform]=ubuntu&sort=last_seen:desc&since=2026-06-01T00:00:00Z",
            VALID_NODE_FIELDS,
        )
        .unwrap();
        assert_eq!(qf.filters.len(), 1);
        assert_eq!(qf.filters[0].field, "platform");
        assert!(qf.sort.is_some());
        assert!(qf.time_range.start_time.is_some());
    }

    // ── validate_filter_fields ──────────────────────────────────────────

    #[test]
    fn test_validate_all_fields_valid() {
        let filters = vec![
            Filter {
                field: "platform".to_string(),
                operator: FilterOp::Eq,
                value: Some(FilterValue::Str("ubuntu".to_string())),
            },
            Filter {
                field: "status".to_string(),
                operator: FilterOp::Eq,
                value: None,
            },
        ];
        let result = validate_filter_fields(&filters, &TimeRange::default(), VALID_NODE_FIELDS);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_unknown_field_error_message() {
        let filters = vec![Filter {
            field: "garbage_field".to_string(),
            operator: FilterOp::Eq,
            value: None,
        }];
        let result = validate_filter_fields(&filters, &TimeRange::default(), VALID_NODE_FIELDS);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("garbage_field"));
        assert!(err.contains("platform"));
    }
}