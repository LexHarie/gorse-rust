// Ports: client/admin.go @ add21b5

//! The response types of the admin API. Field names are Go's, so the master's
//! JSON keys match them exactly. Go decodes a JSON `null` into the zero value of
//! the field, and so do these types.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Go's zero `time.Time`, in the RFC 3339 form the master encodes it with.
pub const ZERO_TIME: &str = "0001-01-01T00:00:00Z";

fn zero_time() -> String {
    ZERO_TIME.to_owned()
}

/// Reads a field that Go encodes as `null` when it is empty, as its zero value.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// Reads a time field, which Go encodes as `null` only when it is unset.
fn null_as_zero_time<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_else(zero_time))
}

/// `master.Status`: the global statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Status {
    /// The binary version of the master.
    #[serde(default, deserialize_with = "null_as_default")]
    pub binary_version: String,
    /// The number of servers.
    #[serde(default, deserialize_with = "null_as_default")]
    pub num_servers: i64,
    /// The number of workers.
    #[serde(default, deserialize_with = "null_as_default")]
    pub num_workers: i64,
    /// The number of users.
    #[serde(default, deserialize_with = "null_as_default")]
    pub num_users: i64,
    /// The number of items.
    #[serde(default, deserialize_with = "null_as_default")]
    pub num_items: i64,
    /// The number of user labels.
    #[serde(default, deserialize_with = "null_as_default")]
    pub num_user_labels: i64,
    /// The number of item labels.
    #[serde(default, deserialize_with = "null_as_default")]
    pub num_item_labels: i64,
    /// The number of positive feedback, before validation.
    #[serde(default, deserialize_with = "null_as_default")]
    pub num_total_pos_feedback: i64,
    /// The number of valid positive feedback.
    #[serde(default, deserialize_with = "null_as_default")]
    pub num_valid_pos_feedback: i64,
    /// The number of valid negative feedback.
    #[serde(default, deserialize_with = "null_as_default")]
    pub num_valid_neg_feedback: i64,
    /// When the popular items were last updated.
    #[serde(default = "zero_time", deserialize_with = "null_as_zero_time")]
    pub popular_items_update_time: String,
    /// When the latest items were last updated.
    #[serde(default = "zero_time", deserialize_with = "null_as_zero_time")]
    pub latest_items_update_time: String,
    /// When the matching model was last fitted.
    #[serde(default = "zero_time", deserialize_with = "null_as_zero_time")]
    pub matching_model_fit_time: String,
    /// The matching model evaluation scores.
    #[serde(default)]
    pub matching_model_score: Value,
    /// When the ranking model was last fitted.
    #[serde(default = "zero_time", deserialize_with = "null_as_zero_time")]
    pub ranking_model_fit_time: String,
    /// The ranking model evaluation scores.
    #[serde(default)]
    pub ranking_model_score: Value,
}

/// `[]Iterator` of feedback: one page and the cursor for the next.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct FeedbackIterator {
    /// The cursor of the next page; empty when there are no more pages.
    #[serde(default, deserialize_with = "null_as_default")]
    pub cursor: String,
    /// The feedback in this page.
    #[serde(default, deserialize_with = "null_as_default")]
    pub feedback: Vec<Feedback>,
}

/// `meta.Node`: one cluster node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Node {
    /// The node UUID.
    #[serde(rename = "UUID", default, deserialize_with = "null_as_default")]
    pub uuid: String,
    /// The node hostname.
    #[serde(default, deserialize_with = "null_as_default")]
    pub hostname: String,
    /// The node type, such as `Server` or `Worker`.
    #[serde(rename = "Type", default, deserialize_with = "null_as_default")]
    pub kind: String,
    /// The node binary version.
    #[serde(default, deserialize_with = "null_as_default")]
    pub version: String,
    /// When the node last reported.
    #[serde(default = "zero_time", deserialize_with = "null_as_zero_time")]
    pub update_time: String,
}

/// `monitor.Progress`: one task of the master.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Progress {
    /// The component that runs the task.
    #[serde(default, deserialize_with = "null_as_default")]
    pub tracer: String,
    /// The name of the task or the item, depending on the type.
    #[serde(default, deserialize_with = "null_as_default")]
    pub name: String,
    /// The status, such as `Complete` or `Failed`.
    #[serde(default, deserialize_with = "null_as_default")]
    pub status: String,
    /// The error, when the task failed.
    #[serde(default, deserialize_with = "null_as_default")]
    pub error: String,
    /// The number of processed records.
    #[serde(default, deserialize_with = "null_as_default")]
    pub count: i64,
    /// The number of records to process.
    #[serde(default, deserialize_with = "null_as_default")]
    pub total: i64,
    /// When the task started.
    #[serde(default = "zero_time", deserialize_with = "null_as_zero_time")]
    pub start_time: String,
    /// When the task finished.
    #[serde(default = "zero_time", deserialize_with = "null_as_zero_time")]
    pub finish_time: String,
}

/// An item, as the admin API's item routes and the SDK encode it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Item {
    /// The item ID.
    #[serde(default, deserialize_with = "null_as_default")]
    pub item_id: String,
    /// Whether the item is hidden from recommendations.
    #[serde(default, deserialize_with = "null_as_default")]
    pub is_hidden: bool,
    /// The categories of the item.
    #[serde(default, deserialize_with = "null_as_default")]
    pub categories: Vec<String>,
    /// When the item or the feedback was created.
    #[serde(default = "zero_time", deserialize_with = "null_as_zero_time")]
    pub timestamp: String,
    /// The labels, as the JSON the master holds.
    #[serde(default)]
    pub labels: Value,
    /// The comment.
    #[serde(default, deserialize_with = "null_as_default")]
    pub comment: String,
}

/// A user, as the admin API's user routes and the SDK encode it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct User {
    /// The user ID.
    #[serde(default, deserialize_with = "null_as_default")]
    pub user_id: String,
    /// The labels, as the JSON the master holds.
    #[serde(default)]
    pub labels: Value,
    /// The comment.
    #[serde(default, deserialize_with = "null_as_default")]
    pub comment: String,
}

/// `data.FeedbackKey`: the identity of one feedback.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct FeedbackKey {
    /// The feedback type.
    #[serde(default, deserialize_with = "null_as_default")]
    pub feedback_type: String,
    /// The user ID.
    #[serde(default, deserialize_with = "null_as_default")]
    pub user_id: String,
    /// The item ID.
    #[serde(default, deserialize_with = "null_as_default")]
    pub item_id: String,
}

/// `data.Feedback`: one feedback. Go embeds the key, so the key's fields are
/// flattened into this object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Feedback {
    /// The identity of the feedback.
    #[serde(flatten)]
    pub key: FeedbackKey,
    /// The value of the feedback.
    #[serde(default, deserialize_with = "null_as_default")]
    pub value: f64,
    /// When the item or the feedback was created.
    #[serde(default = "zero_time", deserialize_with = "null_as_zero_time")]
    pub timestamp: String,
    /// When the feedback was last updated.
    #[serde(default = "zero_time", deserialize_with = "null_as_zero_time")]
    pub updated: String,
    /// The labels, as the JSON the master holds.
    #[serde(default)]
    pub labels: Value,
    /// The comment.
    #[serde(default, deserialize_with = "null_as_default")]
    pub comment: String,
}

/// `master.ScoredItem`: an item with its score. The item's fields are flattened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ScoredItem {
    /// The item.
    #[serde(flatten)]
    pub item: Item,
    /// The score.
    #[serde(default, deserialize_with = "null_as_default")]
    pub score: f64,
}

/// `master.ScoreUser`: a user with its score. The user's fields are flattened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ScoreUser {
    /// The user.
    #[serde(flatten)]
    pub user: User,
    /// The score.
    #[serde(default, deserialize_with = "null_as_default")]
    pub score: f64,
}

/// One page of users, as `GET /api/users` returns it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserIterator {
    /// The cursor of the next page; empty when there are no more pages.
    #[serde(default, deserialize_with = "null_as_default")]
    pub cursor: String,
    /// The users in this page.
    #[serde(default, deserialize_with = "null_as_default")]
    pub users: Vec<User>,
}

/// One page of items, as `GET /api/items` returns it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemIterator {
    /// The cursor of the next page; empty when there are no more pages.
    #[serde(default, deserialize_with = "null_as_default")]
    pub cursor: String,
    /// The items in this page.
    #[serde(default, deserialize_with = "null_as_default")]
    pub items: Vec<Item>,
}

/// The counts a restore reports, and how long it took.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DumpStats {
    /// The users in this page.
    #[serde(default, deserialize_with = "null_as_default")]
    pub users: i64,
    /// The items in this page.
    #[serde(default, deserialize_with = "null_as_default")]
    pub items: i64,
    /// The feedback in this page.
    #[serde(default, deserialize_with = "null_as_default")]
    pub feedback: i64,
    /// The duration in nanoseconds, as Go's `time.Duration` encodes it.
    #[serde(default, deserialize_with = "null_as_default")]
    pub duration: i64,
}

/// `cache.TimeSeriesPoint`: one point of a named series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TimeSeriesPoint {
    /// The name of the task or the item, depending on the type.
    #[serde(default, deserialize_with = "null_as_default")]
    pub name: String,
    /// When the item or the feedback was created.
    #[serde(default = "zero_time", deserialize_with = "null_as_zero_time")]
    pub timestamp: String,
    /// The value of the feedback.
    #[serde(default, deserialize_with = "null_as_default")]
    pub value: f64,
}
