//! Pagination extractors and response types.
//!
//! Provides offset-based ([`Paginate`] / [`Page`]) and cursor-based
//! ([`CursorParams`] / [`CursorPage`]) pagination. Both extractors read
//! [`PaginationConfig`] from application state (if provided) to apply
//! default and maximum page sizes.

const DEFAULT_PER_PAGE: u64 = 20;
const DEFAULT_MAX_PER_PAGE: u64 = 100;

/// Configurable defaults and limits for pagination.
///
/// Register with [`App::pagination()`](crate::app::App::pagination) to
/// override the hardcoded defaults (20 per page, 100 max). Both
/// [`Paginate`] and [`CursorParams`] read this from state automatically.
#[derive(Debug, Clone)]
pub struct PaginationConfig {
    /// Default items per page when not specified in the query string.
    pub default_per_page: u64,
    /// Maximum allowed items per page. Values above this are clamped.
    pub max_per_page: u64,
}

impl PaginationConfig {
    /// Create a new config with defaults (20 per page, 100 max).
    pub fn new() -> Self {
        Self {
            default_per_page: DEFAULT_PER_PAGE,
            max_per_page: DEFAULT_MAX_PER_PAGE,
        }
    }

    /// Set the default items per page.
    pub fn default_per_page(mut self, n: u64) -> Self {
        self.default_per_page = n;
        self
    }

    /// Set the maximum allowed items per page.
    pub fn max_per_page(mut self, n: u64) -> Self {
        self.max_per_page = n;
        self
    }
}

impl Default for PaginationConfig {
    fn default() -> Self {
        Self::new()
    }
}

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::extract::FromRequest;
use crate::request::Request;
use crate::response::{IntoResponse, Response};

/// Offset pagination extractor.
///
/// Parses `?page=N&per_page=N` from the query string. Implements
/// [`FromRequest`] — use it directly as a handler argument (not wrapped
/// in `Query<T>`).
///
/// Values are clamped: `page` is at least 1, `per_page` is clamped to
/// `[1, max_per_page]` using [`PaginationConfig`] from state (or
/// hardcoded defaults of 20/100).
///
/// # Examples
///
/// ```rust,ignore
/// async fn list_users(page: Paginate, db: State<Database>) -> Result<Json<Page<User>>> {
///     let users = db.query("SELECT * LIMIT $1 OFFSET $2", page.limit(), page.offset()).await?;
///     let total = db.count("users").await?;
///     Ok(Json(page.respond(users, total)))
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Paginate {
    /// Current page number (1-indexed, minimum 1).
    pub page: u64,
    /// Items per page (clamped to [1, max_per_page]).
    pub per_page: u64,
}

impl Paginate {
    /// SQL OFFSET value: `(page - 1) * per_page`.
    pub fn offset(&self) -> u64 {
        self.page.saturating_sub(1) * self.per_page
    }

    /// SQL LIMIT value (same as `per_page`).
    pub fn limit(&self) -> u64 {
        self.per_page
    }

    /// Build a [`Page`] response from data and total item count.
    pub fn respond<T: Serialize>(&self, data: Vec<T>, total: u64) -> Page<T> {
        let total_pages = if self.per_page == 0 {
            0
        } else {
            total.div_ceil(self.per_page)
        };
        Page {
            data,
            meta: PageMeta {
                page: self.page,
                per_page: self.per_page,
                total,
                total_pages,
            },
        }
    }
}

/// A page of results with metadata.
///
/// Returned by [`Paginate::respond()`]. Implements [`IntoResponse`] —
/// serializes to JSON with `Content-Type: application/json`.
#[derive(Debug, Serialize)]
pub struct Page<T: Serialize> {
    /// The items on this page.
    pub data: Vec<T>,
    /// Pagination metadata.
    pub meta: PageMeta,
}

/// Metadata for offset pagination.
#[derive(Debug, Serialize)]
pub struct PageMeta {
    /// Current page number.
    pub page: u64,
    /// Items per page.
    pub per_page: u64,
    /// Total number of items across all pages.
    pub total: u64,
    /// Total number of pages.
    pub total_pages: u64,
}

impl<T: Serialize> IntoResponse for Page<T> {
    fn into_response(self) -> Response {
        let body = serde_json::to_vec(&self).expect("Page<T> serialization cannot fail");
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::header::HeaderValue::from_static("application/json"),
        );
        Response::new(http::StatusCode::OK, headers, Bytes::from(body))
    }
}

#[derive(Deserialize)]
struct RawPaginate {
    page: Option<u64>,
    per_page: Option<u64>,
}

impl FromRequest for Paginate {
    fn from_request(req: &mut Request) -> Result<Self, Response> {
        let query_string = req.uri().query().unwrap_or("");
        let raw: RawPaginate = serde_urlencoded::from_str(query_string).map_err(|e| {
            (
                http::StatusCode::BAD_REQUEST,
                format!("Invalid pagination parameters: {e}"),
            )
                .into_response()
        })?;

        let (default_per_page, max_per_page) =
            if let Some(config) = req.extensions().get::<PaginationConfig>() {
                (config.default_per_page, config.max_per_page)
            } else {
                (DEFAULT_PER_PAGE, DEFAULT_MAX_PER_PAGE)
            };

        let page = raw.page.unwrap_or(1).max(1);
        let per_page = raw
            .per_page
            .unwrap_or(default_per_page)
            .clamp(1, max_per_page);

        Ok(Paginate { page, per_page })
    }
}

/// Cursor pagination extractor.
///
/// Parses `?after=CURSOR&limit=N` or `?before=CURSOR&limit=N` from the
/// query string. `after` and `before` are mutually exclusive — providing
/// both returns 400 Bad Request.
///
/// `limit` is clamped to `[1, max_per_page]` using [`PaginationConfig`]
/// from state (or hardcoded defaults).
///
/// # Examples
///
/// ```rust,ignore
/// async fn list_posts(cursor: CursorParams, db: State<Database>) -> Result<Json<CursorPage<Post>>> {
///     let posts = db.query_after(cursor.after.as_deref(), cursor.limit + 1).await?;
///     let next = if posts.len() > cursor.limit as usize {
///         posts.pop().map(|p| p.id.to_string())
///     } else { None };
///     Ok(Json(cursor.respond(posts, next)))
/// }
/// ```
#[derive(Debug, Clone)]
pub struct CursorParams {
    /// Cursor pointing to the item after which to start.
    pub after: Option<String>,
    /// Cursor pointing to the item before which to end.
    pub before: Option<String>,
    /// Number of items to return (clamped to [1, max_per_page]).
    pub limit: u64,
}

impl CursorParams {
    /// Build a [`CursorPage`] response.
    pub fn respond<T: Serialize>(
        &self,
        data: Vec<T>,
        next_cursor: Option<String>,
    ) -> CursorPage<T> {
        let has_more = next_cursor.is_some();
        CursorPage {
            data,
            meta: CursorMeta {
                next_cursor,
                has_more,
            },
        }
    }
}

/// A cursor-paginated page of results.
///
/// Returned by [`CursorParams::respond()`]. Implements [`IntoResponse`] —
/// serializes to JSON with `Content-Type: application/json`.
#[derive(Debug, Serialize)]
pub struct CursorPage<T: Serialize> {
    /// The items on this page.
    pub data: Vec<T>,
    /// Cursor pagination metadata.
    pub meta: CursorMeta,
}

/// Metadata for cursor pagination.
#[derive(Debug, Serialize)]
pub struct CursorMeta {
    /// Cursor for the next page, if more items exist.
    pub next_cursor: Option<String>,
    /// Whether more items exist after this page.
    pub has_more: bool,
}

impl<T: Serialize> IntoResponse for CursorPage<T> {
    fn into_response(self) -> Response {
        let body =
            serde_json::to_vec(&self).expect("CursorPage<T> serialization cannot fail");
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::header::HeaderValue::from_static("application/json"),
        );
        Response::new(http::StatusCode::OK, headers, Bytes::from(body))
    }
}

#[derive(Deserialize)]
struct RawCursorParams {
    after: Option<String>,
    before: Option<String>,
    limit: Option<u64>,
}

impl FromRequest for CursorParams {
    fn from_request(req: &mut Request) -> Result<Self, Response> {
        let query_string = req.uri().query().unwrap_or("");
        let raw: RawCursorParams =
            serde_urlencoded::from_str(query_string).map_err(|e| {
                (
                    http::StatusCode::BAD_REQUEST,
                    format!("Invalid cursor parameters: {e}"),
                )
                    .into_response()
            })?;

        if raw.after.is_some() && raw.before.is_some() {
            return Err((
                http::StatusCode::BAD_REQUEST,
                "Cannot specify both 'after' and 'before' cursors".to_string(),
            )
                .into_response());
        }

        let (default_per_page, max_per_page) =
            if let Some(config) = req.extensions().get::<PaginationConfig>() {
                (config.default_per_page, config.max_per_page)
            } else {
                (DEFAULT_PER_PAGE, DEFAULT_MAX_PER_PAGE)
            };

        let limit = raw.limit.unwrap_or(default_per_page).clamp(1, max_per_page);

        Ok(CursorParams {
            after: raw.after,
            before: raw.before,
            limit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_config_defaults() {
        let config = PaginationConfig::new();
        assert_eq!(config.default_per_page, 20);
        assert_eq!(config.max_per_page, 100);
    }

    #[test]
    fn pagination_config_builder() {
        let config = PaginationConfig::new()
            .default_per_page(25)
            .max_per_page(50);
        assert_eq!(config.default_per_page, 25);
        assert_eq!(config.max_per_page, 50);
    }

    #[test]
    fn paginate_offset_page_one() {
        let p = Paginate { page: 1, per_page: 20 };
        assert_eq!(p.offset(), 0);
        assert_eq!(p.limit(), 20);
    }

    #[test]
    fn paginate_offset_page_three() {
        let p = Paginate { page: 3, per_page: 10 };
        assert_eq!(p.offset(), 20);
        assert_eq!(p.limit(), 10);
    }

    #[test]
    fn paginate_offset_page_zero_saturates() {
        let p = Paginate { page: 0, per_page: 10 };
        assert_eq!(p.offset(), 0);
    }

    #[test]
    fn paginate_respond_builds_correct_meta() {
        let p = Paginate { page: 2, per_page: 10 };
        let page: Page<String> = p.respond(vec!["a".into(), "b".into()], 50);
        assert_eq!(page.meta.page, 2);
        assert_eq!(page.meta.per_page, 10);
        assert_eq!(page.meta.total, 50);
        assert_eq!(page.meta.total_pages, 5);
        assert_eq!(page.data.len(), 2);
    }

    #[test]
    fn paginate_respond_total_pages_rounds_up() {
        let p = Paginate { page: 1, per_page: 10 };
        let page: Page<u32> = p.respond(vec![], 51);
        assert_eq!(page.meta.total_pages, 6);
    }

    #[test]
    fn paginate_respond_zero_total() {
        let p = Paginate { page: 1, per_page: 10 };
        let page: Page<u32> = p.respond(vec![], 0);
        assert_eq!(page.meta.total_pages, 0);
    }

    #[test]
    fn cursor_respond_with_next_cursor() {
        let c = CursorParams { after: None, before: None, limit: 10 };
        let page: CursorPage<String> = c.respond(
            vec!["a".into()],
            Some("cursor_abc".into()),
        );
        assert_eq!(page.data.len(), 1);
        assert_eq!(page.meta.next_cursor, Some("cursor_abc".into()));
        assert!(page.meta.has_more);
    }

    #[test]
    fn cursor_respond_no_more_items() {
        let c = CursorParams { after: None, before: None, limit: 10 };
        let page: CursorPage<u32> = c.respond(vec![1, 2], None);
        assert_eq!(page.meta.next_cursor, None);
        assert!(!page.meta.has_more);
    }

    use crate::extract::FromRequest;
    use http::Method;

    #[test]
    fn paginate_extracts_from_query() {
        let mut req = crate::request::Request::test(Method::GET, "/users?page=2&per_page=15");
        let p = Paginate::from_request(&mut req).unwrap();
        assert_eq!(p.page, 2);
        assert_eq!(p.per_page, 15);
    }

    #[test]
    fn paginate_defaults_when_no_query() {
        let mut req = crate::request::Request::test(Method::GET, "/users");
        let p = Paginate::from_request(&mut req).unwrap();
        assert_eq!(p.page, 1);
        assert_eq!(p.per_page, DEFAULT_PER_PAGE);
    }

    #[test]
    fn paginate_page_zero_becomes_one() {
        let mut req = crate::request::Request::test(Method::GET, "/users?page=0");
        let p = Paginate::from_request(&mut req).unwrap();
        assert_eq!(p.page, 1);
    }

    #[test]
    fn paginate_per_page_clamped_to_max() {
        let mut req = crate::request::Request::test(Method::GET, "/users?per_page=200");
        let p = Paginate::from_request(&mut req).unwrap();
        assert_eq!(p.per_page, DEFAULT_MAX_PER_PAGE);
    }

    #[test]
    fn paginate_per_page_clamped_to_min() {
        let mut req = crate::request::Request::test(Method::GET, "/users?per_page=0");
        let p = Paginate::from_request(&mut req).unwrap();
        assert_eq!(p.per_page, 1);
    }

    #[test]
    fn paginate_respects_pagination_config() {
        let mut req = crate::request::Request::test(Method::GET, "/users");
        req.provide_test_state(PaginationConfig::new().default_per_page(25).max_per_page(50));
        let p = Paginate::from_request(&mut req).unwrap();
        assert_eq!(p.per_page, 25);
    }

    #[test]
    fn paginate_config_clamps_per_page() {
        let mut req = crate::request::Request::test(Method::GET, "/users?per_page=200");
        req.provide_test_state(PaginationConfig::new().max_per_page(50));
        let p = Paginate::from_request(&mut req).unwrap();
        assert_eq!(p.per_page, 50);
    }

    #[test]
    fn cursor_extracts_after() {
        let mut req = crate::request::Request::test(Method::GET, "/posts?after=abc&limit=10");
        let c = CursorParams::from_request(&mut req).unwrap();
        assert_eq!(c.after, Some("abc".into()));
        assert_eq!(c.before, None);
        assert_eq!(c.limit, 10);
    }

    #[test]
    fn cursor_extracts_before() {
        let mut req = crate::request::Request::test(Method::GET, "/posts?before=xyz&limit=5");
        let c = CursorParams::from_request(&mut req).unwrap();
        assert_eq!(c.before, Some("xyz".into()));
        assert_eq!(c.after, None);
    }

    #[test]
    fn cursor_defaults_limit() {
        let mut req = crate::request::Request::test(Method::GET, "/posts");
        let c = CursorParams::from_request(&mut req).unwrap();
        assert_eq!(c.limit, DEFAULT_PER_PAGE);
    }

    #[test]
    fn cursor_clamps_limit() {
        let mut req = crate::request::Request::test(Method::GET, "/posts?limit=200");
        let c = CursorParams::from_request(&mut req).unwrap();
        assert_eq!(c.limit, DEFAULT_MAX_PER_PAGE);
    }

    #[test]
    fn cursor_both_after_and_before_returns_400() {
        let mut req = crate::request::Request::test(
            Method::GET,
            "/posts?after=a&before=b",
        );
        let result = CursorParams::from_request(&mut req);
        assert!(result.is_err());
        let resp = result.unwrap_err();
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn cursor_respects_pagination_config() {
        let mut req = crate::request::Request::test(Method::GET, "/posts?limit=200");
        req.provide_test_state(PaginationConfig::new().max_per_page(50));
        let c = CursorParams::from_request(&mut req).unwrap();
        assert_eq!(c.limit, 50);
    }

    use crate::response::IntoResponse;

    #[test]
    fn page_into_response_is_json() {
        let page = Page {
            data: vec!["alice", "bob"],
            meta: PageMeta {
                page: 1,
                per_page: 10,
                total: 2,
                total_pages: 1,
            },
        };
        let resp = page.into_response();
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(resp.content_type(), Some("application/json"));
        let body: serde_json::Value =
            serde_json::from_slice(resp.body_bytes()).unwrap();
        assert_eq!(body["meta"]["total"], 2);
        assert_eq!(body["data"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn cursor_page_into_response_is_json() {
        let page = CursorPage {
            data: vec![1_u32, 2, 3],
            meta: CursorMeta {
                next_cursor: Some("cursor_xyz".into()),
                has_more: true,
            },
        };
        let resp = page.into_response();
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(resp.content_type(), Some("application/json"));
        let body: serde_json::Value =
            serde_json::from_slice(resp.body_bytes()).unwrap();
        assert_eq!(body["meta"]["next_cursor"], "cursor_xyz");
        assert!(body["meta"]["has_more"].as_bool().unwrap());
    }

    #[test]
    fn page_serializes_correct_structure() {
        let p = Paginate { page: 3, per_page: 5 };
        let page = p.respond(vec!["x"], 12_u64);
        let json: serde_json::Value =
            serde_json::from_slice(page.into_response().body_bytes()).unwrap();
        assert_eq!(json["meta"]["page"], 3);
        assert_eq!(json["meta"]["per_page"], 5);
        assert_eq!(json["meta"]["total"], 12);
        assert_eq!(json["meta"]["total_pages"], 3);
    }
}
