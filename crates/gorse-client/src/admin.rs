// Ports: client/admin.go @ add21b5

//! The admin API client. Its requests go to the master's `/api` routes with the
//! `X-Api-Key` header. The SDK calls at the end (`get_user`, `get_items` and
//! the rest) follow the Go SDK's `gorse-go` requests, which go to the same host
//! without the admin prefix.

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use thiserror::Error;

use crate::model::{
    DumpStats, Feedback, FeedbackIterator, Item, ItemIterator, Node, Progress, ScoreUser,
    ScoredItem, Status, TimeSeriesPoint, User, UserIterator,
};
use crate::url::{Query, path_escape, query_escape};

/// Why a request failed. The messages are Go's.
#[derive(Debug, Error)]
pub enum ClientError {
    /// The request could not be sent, or its response could not be decoded.
    #[error("failed to send request: {0}")]
    Send(String),
    /// The admin API answered with an error status.
    #[error("API request failed: status={status} body={body}")]
    Api {
        /// The HTTP status of the response.
        status: u16,
        /// The body of the response.
        body: String,
    },
    /// A dump could not be written to the output.
    #[error("failed to write response: {0}")]
    Write(String),
    /// The SDK answered with a non-200 status. Go's SDK reports only the body.
    #[error("{0}")]
    Sdk(String),
    /// The SDK's response could not be decoded. Go's SDK reports the decoder's
    /// error as it is.
    #[error("{0}")]
    Decode(String),
}

/// The client of the admin API, and of the SDK routes the CLI uses.
#[derive(Debug, Clone)]
pub struct AdminClient {
    http: reqwest::Client,
    endpoint: String,
    api_key: String,
}

impl AdminClient {
    /// Creates a client for the master at `endpoint`, with its admin API key.
    /// A trailing `/` in `endpoint` is removed, as Go's `NewAdminClient` does.
    pub fn new(endpoint: &str, api_key: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
        }
    }

    /// The cluster's nodes.
    pub async fn get_cluster(&self) -> Result<Vec<Node>, ClientError> {
        self.get("/dashboard/cluster", None).await
    }

    /// The master's tasks and their progress.
    pub async fn get_tasks(&self) -> Result<Vec<Progress>, ClientError> {
        self.get("/dashboard/tasks", None).await
    }

    /// The whole config, as the master reports it.
    pub async fn get_config(&self) -> Result<Map<String, Value>, ClientError> {
        self.get("/dashboard/config", None).await
    }

    /// The same config as [`AdminClient::get_config`]; Go keeps both names.
    pub async fn get_config_map(&self) -> Result<Map<String, Value>, ClientError> {
        self.get("/dashboard/config", None).await
    }

    /// The JSON schema of the config.
    pub async fn get_config_schema(&self) -> Result<Map<String, Value>, ClientError> {
        self.get("/dashboard/config/schema", None).await
    }

    /// Applies `patch` to the config and returns the config the master keeps.
    pub async fn update_config(
        &self,
        patch: &Map<String, Value>,
    ) -> Result<Map<String, Value>, ClientError> {
        let body = encode_json(patch)?;
        let response = self
            .admin_request(reqwest::Method::POST, "/dashboard/config")
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(send_error)?;
        decode_admin_response(response).await
    }

    /// Resets the config to the defaults in the config file.
    pub async fn reset_config(&self) -> Result<Map<String, Value>, ClientError> {
        let response = self
            .admin_request(reqwest::Method::DELETE, "/dashboard/config")
            .send()
            .await
            .map_err(send_error)?;
        decode_admin_response(response).await
    }

    /// The item categories.
    pub async fn get_categories(&self) -> Result<Vec<String>, ClientError> {
        self.get("/dashboard/categories", None).await
    }

    /// The global statistics.
    pub async fn get_stats(&self) -> Result<Status, ClientError> {
        self.get("/dashboard/stats", None).await
    }

    /// The points of the series `name`, between `begin` and `end`, at `duration`
    /// steps. An empty argument is left out of the query.
    pub async fn get_timeseries(
        &self,
        name: &str,
        begin: &str,
        end: &str,
        duration: &str,
    ) -> Result<Vec<TimeSeriesPoint>, ClientError> {
        let mut params = Query::new();
        if !begin.is_empty() {
            params.set("begin", begin);
        }
        if !end.is_empty() {
            params.set("end", end);
        }
        if !duration.is_empty() {
            params.set("duration", duration);
        }
        self.get(
            &format!("/dashboard/timeseries/{}", path_escape(name)),
            Some(&params),
        )
        .await
    }

    /// One page of feedback. `n` of zero leaves the page size to the master.
    pub async fn get_feedback(&self, n: i64) -> Result<FeedbackIterator, ClientError> {
        self.get("/feedback", Some(&count_query(n))).await
    }

    /// One page of feedback of one type.
    pub async fn get_typed_feedback(
        &self,
        feedback_type: &str,
        n: i64,
    ) -> Result<FeedbackIterator, ClientError> {
        self.get(
            &format!("/feedback/{}", path_escape(feedback_type)),
            Some(&count_query(n)),
        )
        .await
    }

    /// The feedback of one user on one item.
    pub async fn get_user_item_feedback(
        &self,
        user_id: &str,
        item_id: &str,
    ) -> Result<Vec<Feedback>, ClientError> {
        self.get(
            &format!(
                "/feedback/{}/{}",
                path_escape(user_id),
                path_escape(item_id)
            ),
            None,
        )
        .await
    }

    /// The feedback of one type from one user on one item.
    pub async fn get_typed_user_item_feedback(
        &self,
        feedback_type: &str,
        user_id: &str,
        item_id: &str,
    ) -> Result<Feedback, ClientError> {
        self.get(
            &format!(
                "/feedback/{}/{}/{}",
                path_escape(feedback_type),
                path_escape(user_id),
                path_escape(item_id)
            ),
            None,
        )
        .await
    }

    /// The feedback of one user.
    pub async fn get_user_feedback(&self, user_id: &str) -> Result<Vec<Feedback>, ClientError> {
        self.get(&format!("/user/{}/feedback", path_escape(user_id)), None)
            .await
    }

    /// The feedback of one type from one user.
    pub async fn get_typed_user_feedback(
        &self,
        user_id: &str,
        feedback_type: &str,
    ) -> Result<Vec<Feedback>, ClientError> {
        self.get(
            &format!(
                "/user/{}/feedback/{}",
                path_escape(user_id),
                path_escape(feedback_type)
            ),
            None,
        )
        .await
    }

    /// The feedback on one item. Go's route ends with a `/`, and so does this one.
    pub async fn get_item_feedback(&self, item_id: &str) -> Result<Vec<Feedback>, ClientError> {
        self.get(&format!("/item/{}/feedback/", path_escape(item_id)), None)
            .await
    }

    /// The feedback of one type on one item.
    pub async fn get_typed_item_feedback(
        &self,
        item_id: &str,
        feedback_type: &str,
    ) -> Result<Vec<Feedback>, ClientError> {
        self.get(
            &format!(
                "/item/{}/feedback/{}",
                path_escape(item_id),
                path_escape(feedback_type)
            ),
            None,
        )
        .await
    }

    /// The latest items, in the given categories.
    pub async fn get_latest(
        &self,
        n: i64,
        categories: &[String],
    ) -> Result<Vec<ScoredItem>, ClientError> {
        let params = categories_query(count_query(n), categories);
        self.get("/dashboard/latest", Some(&params)).await
    }

    /// A non-personalized recommendation `name`. `user_id` removes the items the
    /// user has read, when it is not empty.
    pub async fn get_non_personalized(
        &self,
        name: &str,
        n: i64,
        user_id: &str,
        categories: &[String],
    ) -> Result<Vec<ScoredItem>, ClientError> {
        let mut params = count_query(n);
        if !user_id.is_empty() {
            params.set("user-id", user_id);
        }
        let params = categories_query(params, categories);
        self.get(
            &format!("/dashboard/non-personalized/{}", path_escape(name)),
            Some(&params),
        )
        .await
    }

    /// Recommendations for a user. An empty `recommender` or `name` is left out
    /// of the path, as Go does; a `name` without a `recommender` takes the
    /// `recommender` position.
    pub async fn get_recommend(
        &self,
        user_id: &str,
        recommender: &str,
        name: &str,
        n: i64,
        categories: &[String],
    ) -> Result<Vec<ScoredItem>, ClientError> {
        let mut path = format!("/dashboard/recommend/{}", path_escape(user_id));
        if !recommender.is_empty() {
            path.push('/');
            path.push_str(&path_escape(recommender));
        }
        if !name.is_empty() {
            path.push('/');
            path.push_str(&path_escape(name));
        }
        let params = categories_query(count_query(n), categories);
        self.get(&path, Some(&params)).await
    }

    /// Item-to-item recommendations of `item_id` from the neighbours `name`.
    pub async fn get_item_to_item(
        &self,
        name: &str,
        item_id: &str,
        n: i64,
        categories: &[String],
    ) -> Result<Vec<ScoredItem>, ClientError> {
        let params = categories_query(count_query(n), categories);
        self.get(
            &format!(
                "/dashboard/item-to-item/{}/{}",
                path_escape(name),
                path_escape(item_id)
            ),
            Some(&params),
        )
        .await
    }

    /// User-to-user recommendations of `user_id` from the neighbours `name`.
    pub async fn get_user_to_user(
        &self,
        name: &str,
        user_id: &str,
        n: i64,
    ) -> Result<Vec<ScoreUser>, ClientError> {
        self.get(
            &format!(
                "/dashboard/user-to-user/{}/{}",
                path_escape(name),
                path_escape(user_id)
            ),
            Some(&count_query(n)),
        )
        .await
    }

    /// Restores a dump, which the master reads as the request body.
    pub async fn restore(&self, dump: Vec<u8>) -> Result<DumpStats, ClientError> {
        let response = self
            .admin_request(reqwest::Method::POST, "/restore")
            .header("Content-Type", "application/octet-stream")
            .body(dump)
            .send()
            .await
            .map_err(send_error)?;
        decode_admin_response(response).await
    }

    /// The whole dump of the master's data, as the bytes Go's client writes.
    pub async fn dump(&self) -> Result<Vec<u8>, ClientError> {
        let response = self
            .admin_request(reqwest::Method::GET, "/dump")
            .send()
            .await
            .map_err(send_error)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(send_error)?;
        if status > 399 {
            return Err(ClientError::Api {
                status,
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }
        Ok(body.to_vec())
    }

    /// The user `user_id`, through the SDK route.
    pub async fn get_user(&self, user_id: &str) -> Result<User, ClientError> {
        self.sdk_get(&format!("/api/user/{user_id}")).await
    }

    /// One page of users, through the SDK route.
    pub async fn get_users(&self, n: i64, cursor: &str) -> Result<UserIterator, ClientError> {
        self.sdk_get(&format!("/api/users?n={n}&cursor={cursor}"))
            .await
    }

    /// The item `item_id`, through the SDK route.
    pub async fn get_item(&self, item_id: &str) -> Result<Item, ClientError> {
        self.sdk_get(&format!("/api/item/{item_id}")).await
    }

    /// One page of items, through the SDK route.
    pub async fn get_items(&self, n: i64, cursor: &str) -> Result<ItemIterator, ClientError> {
        self.sdk_get(&format!("/api/items?n={n}&cursor={cursor}"))
            .await
    }

    /// The items that match `query`, through the SDK route.
    pub async fn search_items(&self, query: &str, n: i64) -> Result<ItemIterator, ClientError> {
        self.sdk_get(&format!("/api/items?q={}&n={n}", query_escape(query)))
            .await
    }

    /// `GET` on an admin route. `path` is relative to `/api` and carries no query;
    /// `params` is the query, if any.
    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        params: Option<&Query>,
    ) -> Result<T, ClientError> {
        let mut url = path.to_owned();
        if let Some(params) = params.filter(|params| !params.is_empty()) {
            url.push('?');
            url.push_str(&params.encode());
        }
        let response = self
            .admin_request(reqwest::Method::GET, &url)
            .send()
            .await
            .map_err(send_error)?;
        decode_admin_response(response).await
    }

    /// Starts an admin request. `path` is relative to the master's `/api`.
    fn admin_request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}/api{}", self.endpoint, path))
            .header("X-Api-Key", &self.api_key)
    }

    /// `GET` on an SDK route, which takes the path as the SDK builds it.
    async fn sdk_get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let response = self
            .http
            .get(format!("{}{}", self.endpoint, path))
            .header("X-API-Key", &self.api_key)
            .header("Content-Type", "application/json")
            .send()
            .await
            .map_err(send_error)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(send_error)?;
        if status != 200 {
            return Err(ClientError::Sdk(
                String::from_utf8_lossy(&body).into_owned(),
            ));
        }
        serde_json::from_slice(&body).map_err(|err| ClientError::Decode(err.to_string()))
    }
}

/// The query with `n`, when `n` is not zero.
fn count_query(n: i64) -> Query {
    let mut params = Query::new();
    if n != 0 {
        params.set("n", n.to_string());
    }
    params
}

/// `query` with one `category` parameter for each of `categories`.
fn categories_query(mut query: Query, categories: &[String]) -> Query {
    for category in categories {
        query.add("category", category);
    }
    query
}

fn send_error(err: reqwest::Error) -> ClientError {
    ClientError::Send(err.to_string())
}

fn encode_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, ClientError> {
    serde_json::to_vec(value).map_err(|err| ClientError::Send(err.to_string()))
}

/// Reads an admin response. A status above 399 is an error, as Go's `IsError`
/// says; a body that does not decode is a send error, as resty reports it.
async fn decode_admin_response<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ClientError> {
    let status = response.status().as_u16();
    let body = response.bytes().await.map_err(send_error)?;
    if status > 399 {
        return Err(ClientError::Api {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
        });
    }
    serde_json::from_slice(&body).map_err(|err| ClientError::Send(err.to_string()))
}
