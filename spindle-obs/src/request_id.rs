use uuid::Uuid;

/// Generate a new request identifier (UUIDv7 — timestamp-first, sortable).
pub fn generate_request_id() -> Uuid {
    Uuid::now_v7()
}
