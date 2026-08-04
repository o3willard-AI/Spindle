use spindle_error::*;

#[test]
fn test_error_variants_have_correct_codes() {
    assert_eq!(Error::Ingest("test".into()).code(), 400);
    assert_eq!(Error::Store("test".into()).code(), 500);
    assert_eq!(Error::Pipeline("test".into()).code(), 500);
    assert_eq!(Error::Validation("test".into()).code(), 400);
    assert_eq!(Error::NotFound("test".into()).code(), 404);
    assert_eq!(Error::Internal("test".into()).code(), 500);
    assert_eq!(Error::Authentication("test".into()).code(), 401);
    assert_eq!(Error::Authorization("test".into()).code(), 403);
    assert_eq!(Error::RateLimit("test".into()).code(), 429);
}

#[test]
fn test_error_display() {
    let err = Error::Validation("invalid input".into());
    assert_eq!(format!("{}", err), "validation error: invalid input");
}

#[test]
fn test_error_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Error>();
    assert_send_sync::<ApiError>();
}

#[test]
fn test_api_error_trait_implementation() {
    struct TestError;
    impl ApiErrorTrait for TestError {
        fn code(&self) -> u16 {
            400
        }

        fn message(&self) -> &str {
            "test error"
        }
    }

    let err = TestError;
    assert_eq!(err.code(), 400);
    assert_eq!(err.message(), "test error");
}

#[test]
fn test_error_conversion() {
    let err = Error::NotFound("resource missing".into());
    let api_err: ApiError = err.into();
    assert_eq!(api_err.code, 404);
    assert_eq!(api_err.message, "resource missing");
}
