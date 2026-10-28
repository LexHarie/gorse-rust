// Ports: storage/data/database_test.go @ add21b5 (the `baseTestSuite` cases; the
// backend-free TestSortFeedbacks and TestValidateLabels are in `types.rs`, and
// the benchmark is not ported, since `go test` does not run it)

//! The suite every data store backend runs against a real service. The backend
//! tests (WP-2-2 SQL, WP-2-3 MongoDB) call [`run`] with their store. No backend
//! exists in this crate yet, so [`run`] is compiled and not called here.
//!
//! Go's testify suite becomes plain functions. Each one does the setup and
//! teardown that `SetupTest` and `TearDownTest` do: it pings and purges the
//! store first and purges it last.
//!
//! Go's testify helpers become the helpers below. Go's `faker` labels are
//! fixed strings here; the cases only check that a label round-trips, so the
//! values do not matter. Go's `[]string` and `[]any` labels are one
//! [`Labels::Array`] here, so Go's conversions to `[]any` are the same value.

#![allow(dead_code, reason = "called by the backend tests of WP-2-2 and WP-2-3")]

use std::fmt::Debug;
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use futures_core::stream::BoxStream;
use futures_util::StreamExt;
use gorse_common::expression::{
    ExprType, FeedbackTypeExpression, must_parse_feedback_type_expression,
};
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Duration, HOUR, MICROSECOND, MILLISECOND, Month, SECOND, Time};
use gorse_config::config::SearchConfig;
use gorse_storage_core::errors::Error as StorageError;
use indexmap::IndexMap;

use super::types::{Feedback, FeedbackKey, Item, ItemPatch, Labels, ScoredItem, User, UserPatch};
use super::{
    Database, Error, GetOptions, ScanOption, with_begin_item_id, with_begin_time,
    with_begin_user_id, with_end_item_id, with_end_time, with_end_user_id, with_feedback_types,
    with_order_by_item_id,
};

const POSITIVE_FEEDBACK_TYPE: &str = "positiveFeedbackType";
const POSITIVE_FEEDBACK_TYPE1: &str = "positiveFeedbackType1";
const POSITIVE_FEEDBACK_TYPE2: &str = "positiveFeedbackType2";
const NEGATIVE_FEEDBACK_TYPE: &str = "negativeFeedbackType";
const DUPLICATE_FEEDBACK_TYPE: &str = "duplicateFeedbackType";

/// What a backend adds to the shared suite. Go's suite learns it with a type
/// switch on `*SQLDatabase` and `*MongoDB`, and with `suite.Database.(*SQLDatabase)`.
#[async_trait]
pub trait Backend: Database {
    /// Go's `analyzeTables`: Postgres refreshes the planner statistics of the
    /// three tables, so the counts see the rows the case inserted. Every other
    /// store does nothing.
    async fn analyze_tables(&self) -> Result<(), Error> {
        Ok(())
    }

    /// How the store keeps time stamps, which [`test_timezone`] checks.
    fn time_stamps(&self) -> TimeStamps;
}

/// How a store keeps time stamps, for the timezone case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeStamps {
    /// Postgres keeps microseconds: the case rounds the stamps to them.
    Microseconds,
    /// SQLite keeps the stamps exactly.
    Exact,
    /// MongoDB keeps milliseconds: the case truncates the stamps to them.
    Milliseconds,
    /// Go skips the timezone case for a store it does not know.
    Unknown,
}

/// Runs every suite case against `db`. The store is not closed.
pub async fn run(db: &dyn Backend) {
    test_init(db).await;
    test_users(db).await;
    test_feedback(db).await;
    test_items(db).await;
    test_batch_get_items(db).await;
    test_delete_user(db).await;
    test_delete_item(db).await;
    test_delete_feedback(db).await;
    test_time_limit(db).await;
    test_timezone(db).await;
    test_collation(db).await;
    test_search(db).await;
    test_purge(db).await;
}

/// Runs the cases of Go's proxy suite (`storage/data/proxy_test.go`): every
/// case of [`run`] except `TestInit` and `TestPurge`, which Go skips there
/// because the proxy client refuses `Init` and `Purge`. The cases keep the
/// order of [`run`].
pub(crate) async fn run_through_proxy(db: &dyn Backend) {
    test_users(db).await;
    test_feedback(db).await;
    test_items(db).await;
    test_batch_get_items(db).await;
    test_delete_user(db).await;
    test_delete_item(db).await;
    test_delete_feedback(db).await;
    test_time_limit(db).await;
    test_timezone(db).await;
    test_collation(db).await;
    test_search(db).await;
}

/// Go's `SetupTest`: pings the store and purges it.
async fn setup(db: &dyn Backend) {
    db.ping().await.unwrap();
    db.purge().await.unwrap();
}

/// Go's `TearDownTest`: purges the store.
async fn teardown(db: &dyn Backend) {
    db.purge().await.unwrap();
}

/// Go's `context.Background()`, which the suite passes as `suite.T().Context()`.
fn ctx() -> Context {
    Context::background()
}

/// Go's `time.Date(year, month, day, hour, 0, 0, 0, time.UTC)`.
fn date(year: i64, month: i64, day: i64, hour: i64) -> Time {
    gotime::date(year, Month(month), day, hour, 0, 0, 0, &gotime::utc())
}

/// Go's `[]string{...}`.
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

/// Go's `[]string{...}` or `[]any{...}` of strings, as a [`Labels::Array`].
fn str_labels(values: &[&str]) -> Labels {
    Labels::Array(
        values
            .iter()
            .map(|v| Labels::String((*v).to_owned()))
            .collect(),
    )
}

/// Go's `map[string]any{...}`, as a [`Labels::Object`].
fn object_labels(entries: &[(&str, Labels)]) -> Labels {
    Labels::Object(
        entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect::<IndexMap<_, _>>(),
    )
}

/// Go's `FeedbackKey{...}`.
fn key(feedback_type: &str, user_id: &str, item_id: &str) -> FeedbackKey {
    FeedbackKey {
        feedback_type: feedback_type.to_owned(),
        user_id: user_id.to_owned(),
        item_id: item_id.to_owned(),
    }
}

/// Go's `Feedback{FeedbackKey: ..., Value: value, Timestamp: ts, Comment: comment}`.
fn feedback(feedback_key: FeedbackKey, value: f64, timestamp: Time, comment: &str) -> Feedback {
    Feedback {
        feedback_key,
        value,
        timestamp,
        comment: comment.to_owned(),
        ..Default::default()
    }
}

/// Go's `ElementsMatch`: the two slices hold the same elements in any order.
fn assert_elements_match<T: Debug + PartialEq>(expected: &[T], actual: &[T]) {
    assert_eq!(expected.len(), actual.len(), "{actual:?}");
    let mut used = vec![false; actual.len()];
    for want in expected {
        let found = actual
            .iter()
            .enumerate()
            .position(|(i, got)| !used[i] && got == want);
        match found {
            Some(i) => used[i] = true,
            None => panic!("{want:?} is not in {actual:?}"),
        }
    }
}

/// Go's `getUsers`: every user, read in pages of `batch_size`.
async fn get_users(db: &dyn Backend, ctx: &Context, batch_size: i64) -> Vec<User> {
    let mut users = Vec::new();
    let mut cursor = String::new();
    loop {
        let (next, data) = db.get_users(ctx, &cursor, batch_size).await.unwrap();
        let len = data.len() as i64;
        users.extend(data);
        if next.is_empty() {
            assert!(len <= batch_size);
            return users;
        }
        assert_eq!(batch_size, len);
        cursor = next;
    }
}

/// Go's `getUsersStream`: every user, streamed in batches of `batch_size`.
async fn get_users_stream(db: &dyn Backend, ctx: &Context, batch_size: i64) -> Vec<User> {
    collect(db.get_user_stream(ctx, batch_size)).await
}

/// Go's `getItems`: every item, read in pages of `batch_size`.
async fn get_items(db: &dyn Backend, ctx: &Context, batch_size: i64) -> Vec<Item> {
    let mut items = Vec::new();
    let mut cursor = String::new();
    loop {
        let (next, data) = db.get_items(ctx, &cursor, batch_size, None).await.unwrap();
        let len = data.len() as i64;
        items.extend(data);
        if next.is_empty() {
            assert!(len <= batch_size);
            return items;
        }
        assert_eq!(batch_size, len);
        cursor = next;
    }
}

/// Go's `getItemStream`: every item, streamed in batches of `batch_size`.
async fn get_item_stream(db: &dyn Backend, ctx: &Context, batch_size: i64) -> Vec<Item> {
    collect(db.get_item_stream(ctx, batch_size, None)).await
}

/// Go's `getFeedback`: every feedback record in the range, read in pages.
async fn get_feedback(
    db: &dyn Backend,
    ctx: &Context,
    batch_size: i64,
    begin_time: Option<Time>,
    end_time: Option<Time>,
    feedback_types: &[&str],
) -> Vec<Feedback> {
    let feedback_types = strings(feedback_types);
    let mut records = Vec::new();
    let mut cursor = String::new();
    loop {
        let (next, data) = db
            .get_feedback(
                ctx,
                &cursor,
                batch_size,
                begin_time.clone(),
                end_time.clone(),
                &feedback_types,
            )
            .await
            .unwrap();
        let len = data.len() as i64;
        records.extend(data);
        if next.is_empty() {
            assert!(len <= batch_size);
            return records;
        }
        assert_eq!(batch_size, len);
        cursor = next;
    }
}

/// Go's `getFeedbackStream`: the feedback that `options` select, streamed.
async fn get_feedback_stream(
    db: &dyn Backend,
    ctx: &Context,
    batch_size: i64,
    options: &[ScanOption],
) -> Vec<Feedback> {
    collect(db.get_feedback_stream(ctx, batch_size, options)).await
}

/// Drains a batch stream into one vector. Go reads the error channel after the
/// batches, so an error fails the case here.
async fn collect<T>(mut stream: BoxStream<'_, Result<Vec<T>, Error>>) -> Vec<T> {
    let mut out = Vec::new();
    while let Some(batch) = stream.next().await {
        out.extend(batch.unwrap());
    }
    out
}

/// Go's `analyzeTables`, through the backend.
async fn analyze_tables(db: &dyn Backend) {
    db.analyze_tables().await.unwrap();
}

async fn test_init(db: &dyn Backend) {
    setup(db).await;
    db.init().await.unwrap();
    teardown(db).await;
}

async fn test_users(db: &dyn Backend) {
    setup(db).await;
    let ctx = ctx();
    // Insert users
    let mut inserted_users = Vec::new();
    for i in (0..10).rev() {
        inserted_users.push(User {
            user_id: i.to_string(),
            labels: object_labels(&[
                ("color", Labels::String("red".to_owned())),
                ("company", str_labels(&["acme", "chrono", "ergon"])),
            ]),
            comment: format!("comment {i}"),
            ..Default::default()
        });
    }
    db.batch_insert_users(&ctx, &inserted_users).await.unwrap();
    // Count users
    analyze_tables(db).await;
    let count = db.count_users(&ctx).await.unwrap();
    assert_eq!(10, count);
    // Get users
    let mut users = get_users(db, &ctx, 3).await;
    assert_eq!(10, users.len());
    for i in 0..users.len() {
        assert!(!users[i].update_at.is_zero());
        users[i].update_at = Time::default();
        assert_eq!(inserted_users[9 - i], users[i]);
    }
    // Get user stream
    let mut users_from_stream = get_users_stream(db, &ctx, 3).await;
    for user in users_from_stream.iter_mut() {
        assert!(!user.update_at.is_zero());
        user.update_at = Time::default();
    }
    assert_elements_match(&inserted_users, &users_from_stream);
    // Get this user
    let user = db.get_user(&ctx, "0").await.unwrap();
    assert_eq!("0", user.user_id);
    // Delete this user
    db.delete_user(&ctx, "0").await.unwrap();
    let err = db.get_user(&ctx, "0").await.unwrap_err();
    assert_eq!(Some(StorageError::NotFound), err.kind());
    // test override
    let user = db.get_user(&ctx, "1").await.unwrap();
    let previous_update_at = user.update_at.clone();
    // MongoDB stores timestamps with millisecond precision.
    std::thread::sleep(StdDuration::from_millis(1));
    db.batch_insert_users(
        &ctx,
        &[User {
            user_id: "1".to_owned(),
            comment: "override".to_owned(),
            ..Default::default()
        }],
    )
    .await
    .unwrap();
    db.optimize().await.unwrap();
    let user = db.get_user(&ctx, "1").await.unwrap();
    assert!(
        user.update_at.after(&previous_update_at),
        "UpdateAt should increase after overwriting a user"
    );
    assert_eq!("override", user.comment);
    // test modify
    let previous_update_at = user.update_at.clone();
    std::thread::sleep(StdDuration::from_millis(1));
    db.modify_user(
        &ctx,
        "1",
        &UserPatch {
            comment: Some("modify".to_owned()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.modify_user(
        &ctx,
        "1",
        &UserPatch {
            labels: str_labels(&["a", "b", "c"]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.optimize().await.unwrap();
    let user = db.get_user(&ctx, "1").await.unwrap();
    assert!(
        user.update_at.after(&previous_update_at),
        "UpdateAt should increase after modifying a user"
    );
    assert_eq!("modify", user.comment);
    assert_eq!(str_labels(&["a", "b", "c"]), user.labels);

    // test insert empty
    db.batch_insert_users(&ctx, &[]).await.unwrap();

    // insert duplicate users
    db.batch_insert_users(
        &ctx,
        &[
            User {
                user_id: "1".to_owned(),
                ..Default::default()
            },
            User {
                user_id: "1".to_owned(),
                ..Default::default()
            },
        ],
    )
    .await
    .unwrap();
    teardown(db).await;
}

async fn test_feedback(db: &dyn Backend) {
    setup(db).await;
    let ctx = ctx();
    // users that already exists
    db.batch_insert_users(
        &ctx,
        &[User {
            user_id: "0".to_owned(),
            labels: str_labels(&["a"]),
            comment: "comment".to_owned(),
            ..Default::default()
        }],
    )
    .await
    .unwrap();
    // items that already exists
    db.batch_insert_items(
        &ctx,
        &[Item {
            item_id: "0".to_owned(),
            labels: str_labels(&["b"]),
            timestamp: date(1996, 4, 8, 10),
            ..Default::default()
        }],
    )
    .await
    .unwrap();
    // insert feedbacks
    let timestamp = date(1996, 3, 15, 0);
    let mut feedback_records = vec![
        feedback(
            key(POSITIVE_FEEDBACK_TYPE1, "0", "8"),
            1.0,
            timestamp.clone(),
            "comment",
        ),
        feedback(
            key(POSITIVE_FEEDBACK_TYPE1, "1", "6"),
            1.0,
            timestamp.clone(),
            "comment",
        ),
        feedback(
            key(POSITIVE_FEEDBACK_TYPE2, "2", "4"),
            1.0,
            timestamp.clone(),
            "comment",
        ),
        feedback(
            key(POSITIVE_FEEDBACK_TYPE2, "3", "2"),
            1.0,
            timestamp.clone(),
            "comment",
        ),
        feedback(
            key(POSITIVE_FEEDBACK_TYPE2, "4", "0"),
            1.0,
            timestamp.clone(),
            "comment",
        ),
    ];
    feedback_records[0].labels = str_labels(&["positive"]);
    db.batch_insert_feedback(&ctx, &feedback_records, true, true, true)
        .await
        .unwrap();
    // set Updated for comparison
    for record in feedback_records.iter_mut() {
        record.updated = record.timestamp.clone();
    }
    // other type
    db.batch_insert_feedback(
        &ctx,
        &[Feedback {
            feedback_key: key(NEGATIVE_FEEDBACK_TYPE, "0", "2"),
            ..Default::default()
        }],
        true,
        true,
        true,
    )
    .await
    .unwrap();
    db.batch_insert_feedback(
        &ctx,
        &[Feedback {
            feedback_key: key(NEGATIVE_FEEDBACK_TYPE, "2", "4"),
            ..Default::default()
        }],
        true,
        true,
        true,
    )
    .await
    .unwrap();
    // future feedback
    let future_feedback: Vec<Feedback> =
        [("0", "0"), ("1", "2"), ("2", "4"), ("3", "6"), ("4", "8")]
            .iter()
            .map(|(user, item)| {
                feedback(
                    key(DUPLICATE_FEEDBACK_TYPE, user, item),
                    0.0,
                    gotime::now().add(HOUR),
                    "comment",
                )
            })
            .collect();
    db.batch_insert_feedback(&ctx, &future_feedback, true, true, true)
        .await
        .unwrap();
    // Count feedback
    analyze_tables(db).await;
    let count = db.count_feedback(&ctx).await.unwrap();
    assert_eq!(12, count);
    // Get feedback
    let ret = get_feedback(
        db,
        &ctx,
        3,
        None,
        Some(gotime::now()),
        &[POSITIVE_FEEDBACK_TYPE1, POSITIVE_FEEDBACK_TYPE2],
    )
    .await;
    assert_eq!(feedback_records, ret);
    let ret = get_feedback(db, &ctx, 2, None, Some(gotime::now()), &[]).await;
    assert_eq!(feedback_records.len() + 2, ret.len());
    let ret = get_feedback(
        db,
        &ctx,
        2,
        Some(timestamp.add(SECOND)),
        Some(gotime::now()),
        &[],
    )
    .await;
    assert!(ret.is_empty());
    // Get feedback stream
    let positive_types = vec![
        must_parse_feedback_type_expression(POSITIVE_FEEDBACK_TYPE1),
        must_parse_feedback_type_expression(POSITIVE_FEEDBACK_TYPE2),
    ];
    let feedback_from_stream = get_feedback_stream(
        db,
        &ctx,
        3,
        &[
            with_end_time(gotime::now()),
            with_feedback_types(positive_types.clone()),
        ],
    )
    .await;
    assert_elements_match(&feedback_records, &feedback_from_stream);
    let feedback_from_stream =
        get_feedback_stream(db, &ctx, 3, &[with_end_time(gotime::now())]).await;
    assert_eq!(feedback_records.len() + 2, feedback_from_stream.len());
    let feedback_from_stream = get_feedback_stream(
        db,
        &ctx,
        3,
        &[
            with_begin_time(timestamp.add(SECOND)),
            with_end_time(gotime::now()),
        ],
    )
    .await;
    assert!(feedback_from_stream.is_empty());
    let feedback_from_stream = get_feedback_stream(
        db,
        &ctx,
        3,
        &[
            with_begin_user_id("1"),
            with_end_user_id("3"),
            with_end_time(gotime::now()),
            with_feedback_types(positive_types.clone()),
        ],
    )
    .await;
    assert_eq!(feedback_records[1..4].to_vec(), feedback_from_stream);
    let feedback_from_stream = get_feedback_stream(
        db,
        &ctx,
        3,
        &[
            with_begin_item_id("2"),
            with_end_item_id("6"),
            with_end_time(gotime::now()),
            with_feedback_types(positive_types),
            with_order_by_item_id(),
        ],
    )
    .await;
    assert_eq!(
        vec![
            feedback_records[3].clone(),
            feedback_records[2].clone(),
            feedback_records[1].clone()
        ],
        feedback_from_stream
    );
    // Get items
    db.optimize().await.unwrap();
    let items = get_items(db, &ctx, 3).await;
    assert_eq!(5, items.len());
    for (i, item) in items.iter().enumerate() {
        assert_eq!((i * 2).to_string(), item.item_id);
        if item.item_id != "0" {
            assert!(item.timestamp.is_zero());
            assert!(item.labels == Labels::Nil);
            assert!(item.comment.is_empty());
        }
    }
    // Get users
    let users = get_users(db, &ctx, 2).await;
    assert_eq!(5, users.len());
    for (i, user) in users.iter().enumerate() {
        assert_eq!(i.to_string(), user.user_id);
        if user.user_id != "0" {
            assert!(user.labels == Labels::Nil);
            assert!(user.comment.is_empty());
        }
    }
    // check users that already exists
    let mut user = db.get_user(&ctx, "0").await.unwrap();
    assert!(!user.update_at.is_zero());
    user.update_at = Time::default();
    assert_eq!(
        User {
            user_id: "0".to_owned(),
            labels: str_labels(&["a"]),
            comment: "comment".to_owned(),
            ..Default::default()
        },
        user
    );
    // check items that already exists
    let mut item = db.get_item(&ctx, "0").await.unwrap();
    assert!(!item.update_at.is_zero());
    item.update_at = Time::default();
    assert_eq!(
        Item {
            item_id: "0".to_owned(),
            labels: str_labels(&["b"]),
            timestamp: date(1996, 4, 8, 10),
            ..Default::default()
        },
        item
    );
    // Get typed feedback by user
    let ret = db
        .get_user_feedback(
            &ctx,
            "2",
            Some(gotime::now()),
            &[
                must_parse_feedback_type_expression(POSITIVE_FEEDBACK_TYPE1),
                must_parse_feedback_type_expression(POSITIVE_FEEDBACK_TYPE2),
            ],
        )
        .await
        .unwrap();
    assert_eq!(1, ret.len());
    assert_eq!("2", ret[0].feedback_key.user_id);
    assert_eq!("4", ret[0].feedback_key.item_id);
    // Get all feedback by user
    let ret = db
        .get_user_feedback(&ctx, "2", Some(gotime::now()), &[])
        .await
        .unwrap();
    assert_eq!(2, ret.len());
    // Get typed feedback by item
    let ret = db
        .get_item_feedback(
            &ctx,
            "4",
            &strings(&[POSITIVE_FEEDBACK_TYPE1, POSITIVE_FEEDBACK_TYPE2]),
        )
        .await
        .unwrap();
    assert_eq!(1, ret.len());
    assert_eq!("2", ret[0].feedback_key.user_id);
    assert_eq!("4", ret[0].feedback_key.item_id);
    // Get all feedback by item
    let ret = db.get_item_feedback(&ctx, "4", &[]).await.unwrap();
    assert_eq!(2, ret.len());
    // test override
    db.batch_insert_feedback(
        &ctx,
        &[Feedback {
            feedback_key: key(POSITIVE_FEEDBACK_TYPE, "0", "8"),
            value: 100.0,
            timestamp: date(1996, 4, 8, 0),
            labels: str_labels(&["override"]),
            comment: "override".to_owned(),
            ..Default::default()
        }],
        true,
        true,
        true,
    )
    .await
    .unwrap();
    db.optimize().await.unwrap();
    // Get feedback by user with value filter
    let ret = db
        .get_user_feedback(
            &ctx,
            "0",
            Some(gotime::now()),
            &[FeedbackTypeExpression {
                feedback_type: POSITIVE_FEEDBACK_TYPE.to_owned(),
                value: 50.0,
                expr_type: ExprType::Greater,
            }],
        )
        .await
        .unwrap();
    assert_eq!(1, ret.len());
    assert_eq!(100.0, ret[0].value);
    let ret = db
        .get_user_feedback(
            &ctx,
            "0",
            Some(gotime::now()),
            &[must_parse_feedback_type_expression(POSITIVE_FEEDBACK_TYPE)],
        )
        .await
        .unwrap();
    assert_eq!(1, ret.len());
    assert_eq!(100.0, ret[0].value);
    assert_eq!(date(1996, 4, 8, 0), ret[0].timestamp);
    assert_eq!(str_labels(&["override"]), ret[0].labels);
    assert_eq!("override", ret[0].comment);
    // test not overwrite
    db.batch_insert_feedback(
        &ctx,
        &[Feedback {
            feedback_key: key(POSITIVE_FEEDBACK_TYPE, "0", "8"),
            value: 80.0,
            timestamp: date(1996, 3, 15, 0),
            labels: str_labels(&["not_override"]),
            comment: "not_override".to_owned(),
            ..Default::default()
        }],
        true,
        true,
        false,
    )
    .await
    .unwrap();
    db.optimize().await.unwrap();
    let ret = db
        .get_user_feedback(
            &ctx,
            "0",
            Some(gotime::now()),
            &[must_parse_feedback_type_expression(POSITIVE_FEEDBACK_TYPE)],
        )
        .await
        .unwrap();
    assert_eq!(1, ret.len());
    assert_eq!(180.0, ret[0].value);
    assert_eq!(date(1996, 3, 15, 0), ret[0].timestamp);
    assert_eq!(str_labels(&["not_override"]), ret[0].labels);
    assert_eq!("not_override", ret[0].comment);

    // insert no feedback
    db.batch_insert_feedback(&ctx, &[], true, true, true)
        .await
        .unwrap();

    // not insert users or items
    db.batch_insert_feedback(
        &ctx,
        &[
            feedback_only("a", "100", "200"),
            feedback_only("a", "0", "200"),
            feedback_only("a", "100", "8"),
        ],
        false,
        false,
        false,
    )
    .await
    .unwrap();
    let result = db
        .get_user_item_feedback(&ctx, "100", "200", &[])
        .await
        .unwrap();
    assert!(result.is_empty());
    let result = db
        .get_user_item_feedback(&ctx, "0", "200", &[])
        .await
        .unwrap();
    assert!(result.is_empty());
    let result = db
        .get_user_item_feedback(&ctx, "100", "8", &[])
        .await
        .unwrap();
    assert!(result.is_empty());

    // insert valid feedback and invalid feedback at the same time
    db.batch_insert_feedback(
        &ctx,
        &[
            feedback_only("a", "0", "8"),
            feedback_only("a", "100", "200"),
        ],
        false,
        false,
        false,
    )
    .await
    .unwrap();

    // insert duplicate feedback
    db.batch_insert_feedback(
        &ctx,
        &[
            feedback(key("a", "0", "0"), 1.0, timestamp.clone(), ""),
            feedback(key("a", "0", "0"), 1.0, timestamp.clone(), ""),
        ],
        true,
        true,
        true,
    )
    .await
    .unwrap();
    db.batch_insert_feedback(
        &ctx,
        &[feedback(key("a", "0", "0"), 1.0, timestamp.clone(), "")],
        true,
        true,
        false,
    )
    .await
    .unwrap();
    // check duplicate feedback
    let ret = db
        .get_user_item_feedback(&ctx, "0", "0", &strings(&["a"]))
        .await
        .unwrap();
    assert_eq!(
        vec![Feedback {
            updated: timestamp.clone(),
            ..feedback(key("a", "0", "0"), 2.0, timestamp.clone(), "")
        }],
        ret
    );
    // put duplicate feedback
    db.batch_insert_feedback(
        &ctx,
        &[feedback(key("a", "0", "0"), 1.0, timestamp.clone(), "")],
        true,
        true,
        true,
    )
    .await
    .unwrap();
    // check duplicate feedback again
    let ret = db
        .get_user_item_feedback(&ctx, "0", "0", &strings(&["a"]))
        .await
        .unwrap();
    assert_eq!(
        vec![Feedback {
            updated: timestamp.clone(),
            ..feedback(key("a", "0", "0"), 1.0, timestamp.clone(), "")
        }],
        ret
    );
    teardown(db).await;
}

/// Go's `Feedback{FeedbackKey: key}`: a feedback record with only its key.
fn feedback_only(feedback_type: &str, user_id: &str, item_id: &str) -> Feedback {
    Feedback {
        feedback_key: key(feedback_type, user_id, item_id),
        ..Default::default()
    }
}

async fn test_items(db: &dyn Backend) {
    setup(db).await;
    let ctx = ctx();
    // Items
    let items = vec![
        Item {
            item_id: "0".to_owned(),
            is_hidden: true,
            categories: Some(strings(&["a"])),
            timestamp: date(1996, 3, 15, 0),
            labels: str_labels(&["a"]),
            comment: "comment 0".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "2".to_owned(),
            categories: Some(strings(&["b"])),
            timestamp: date(1997, 3, 15, 0),
            labels: str_labels(&["a"]),
            comment: "comment 2".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "4".to_owned(),
            is_hidden: true,
            categories: Some(strings(&["a"])),
            timestamp: date(1998, 3, 15, 0),
            labels: str_labels(&["a", "b"]),
            comment: "comment 4".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "6".to_owned(),
            categories: Some(strings(&["b"])),
            timestamp: date(1999, 3, 15, 0),
            labels: str_labels(&["b"]),
            comment: "comment 6".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "8".to_owned(),
            is_hidden: true,
            categories: Some(strings(&["a"])),
            timestamp: date(2000, 3, 15, 0),
            labels: str_labels(&["b"]),
            comment: "comment 8".to_owned(),
            ..Default::default()
        },
    ];
    // Insert item
    db.batch_insert_items(&ctx, &items).await.unwrap();
    // Count items
    analyze_tables(db).await;
    let count = db.count_items(&ctx).await.unwrap();
    assert_eq!(5, count);
    // Get items
    let mut total_items = get_items(db, &ctx, 3).await;
    for item in total_items.iter_mut() {
        assert!(!item.update_at.is_zero());
        item.update_at = Time::default();
    }
    assert_eq!(items, total_items);
    // Get item stream
    let mut items_from_stream = get_item_stream(db, &ctx, 3).await;
    for item in items_from_stream.iter_mut() {
        assert!(!item.update_at.is_zero());
        item.update_at = Time::default();
    }
    assert_elements_match(&items, &items_from_stream);
    // Get item
    for item in &items {
        let mut ret = db.get_item(&ctx, &item.item_id).await.unwrap();
        assert!(!ret.update_at.is_zero());
        ret.update_at = Time::default();
        assert_eq!(item, &ret);
    }
    // batch get items
    let mut batch_item = db
        .batch_get_items(&ctx, &strings(&["2", "6"]), &GetOptions::default())
        .await
        .unwrap();
    for item in batch_item.iter_mut() {
        assert!(!item.update_at.is_zero());
        item.update_at = Time::default();
    }
    assert_eq!(vec![items[1].clone(), items[3].clone()], batch_item);
    // Test GetLatestItems
    let mut latest_items = db.get_latest_items(&ctx, 3, &[], None).await.unwrap();
    for item in latest_items.iter_mut() {
        assert!(!item.update_at.is_zero());
        item.update_at = Time::default();
    }
    assert_eq!(vec![items[3].clone(), items[1].clone()], latest_items);
    let mut latest_items_with_category = db
        .get_latest_items(&ctx, 3, &strings(&["b"]), None)
        .await
        .unwrap();
    for item in latest_items_with_category.iter_mut() {
        assert!(!item.update_at.is_zero());
        item.update_at = Time::default();
    }
    assert_eq!(
        vec![items[3].clone(), items[1].clone()],
        latest_items_with_category
    );
    // Test GetLatestItems with after time filter
    let after_time = date(1998, 1, 1, 0);
    let mut latest_items_after = db
        .get_latest_items(&ctx, 3, &[], Some(after_time))
        .await
        .unwrap();
    for item in latest_items_after.iter_mut() {
        assert!(!item.update_at.is_zero());
        item.update_at = Time::default();
    }
    // only the newest item has timestamp > afterTime
    assert_eq!(vec![items[3].clone()], latest_items_after);
    // Delete item
    db.delete_item(&ctx, "0").await.unwrap();
    let err = db.get_item(&ctx, "0").await.unwrap_err();
    assert_eq!(Some(StorageError::NotFound), err.kind());

    // test override
    let item = db.get_item(&ctx, "4").await.unwrap();
    let previous_update_at = item.update_at.clone();
    // MongoDB stores timestamps with millisecond precision.
    std::thread::sleep(StdDuration::from_millis(1));
    db.batch_insert_items(
        &ctx,
        &[Item {
            item_id: "4".to_owned(),
            is_hidden: false,
            categories: Some(strings(&["b"])),
            labels: str_labels(&["o"]),
            comment: "override".to_owned(),
            ..Default::default()
        }],
    )
    .await
    .unwrap();
    db.optimize().await.unwrap();
    let item = db.get_item(&ctx, "4").await.unwrap();
    assert!(
        item.update_at.after(&previous_update_at),
        "UpdateAt should increase after overwriting an item"
    );
    assert!(!item.is_hidden);
    assert_eq!(Some(strings(&["b"])), item.categories);
    assert_eq!(str_labels(&["o"]), item.labels);
    assert_eq!("override", item.comment);

    // test modify
    let item = db.get_item(&ctx, "2").await.unwrap();
    let previous_update_at = item.update_at.clone();
    std::thread::sleep(StdDuration::from_millis(1));
    let timestamp = date(2000, 1, 1, 1);
    db.modify_item(
        &ctx,
        "2",
        &ItemPatch {
            is_hidden: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.modify_item(
        &ctx,
        "2",
        &ItemPatch {
            categories: Some(strings(&["a"])),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.modify_item(
        &ctx,
        "2",
        &ItemPatch {
            comment: Some("modify".to_owned()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.modify_item(
        &ctx,
        "2",
        &ItemPatch {
            labels: str_labels(&["a", "b", "c"]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.modify_item(
        &ctx,
        "2",
        &ItemPatch {
            timestamp: Some(timestamp.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.optimize().await.unwrap();
    let item = db.get_item(&ctx, "2").await.unwrap();
    assert!(
        item.update_at.after(&previous_update_at),
        "UpdateAt should increase after modifying an item"
    );
    assert!(item.is_hidden);
    assert_eq!(Some(strings(&["a"])), item.categories);
    assert_eq!("modify", item.comment);
    assert_eq!(str_labels(&["a", "b", "c"]), item.labels);
    assert_eq!(timestamp, item.timestamp);

    // test insert empty
    db.batch_insert_items(&ctx, &[]).await.unwrap();
    // test get empty
    let items = db
        .batch_get_items(&ctx, &[], &GetOptions::default())
        .await
        .unwrap();
    assert!(items.is_empty());

    // test insert duplicate items
    db.batch_insert_items(
        &ctx,
        &[
            Item {
                item_id: "1".to_owned(),
                ..Default::default()
            },
            Item {
                item_id: "1".to_owned(),
                ..Default::default()
            },
        ],
    )
    .await
    .unwrap();
    teardown(db).await;
}

async fn test_batch_get_items(db: &dyn Backend) {
    setup(db).await;
    let ctx = ctx();
    // Insert test items
    let timestamp = date(1996, 3, 15, 0);
    let items = vec![
        Item {
            item_id: "1".to_owned(),
            is_hidden: false,
            categories: Some(strings(&["a"])),
            timestamp: timestamp.clone(),
            ..Default::default()
        },
        Item {
            item_id: "2".to_owned(),
            is_hidden: false,
            categories: Some(strings(&["b"])),
            timestamp: timestamp.clone(),
            ..Default::default()
        },
        Item {
            item_id: "3".to_owned(),
            is_hidden: true,
            categories: Some(strings(&["c"])),
            timestamp: timestamp.clone(),
            ..Default::default()
        },
        Item {
            item_id: "4".to_owned(),
            is_hidden: false,
            categories: Some(strings(&["a", "b"])),
            timestamp: timestamp.clone(),
            ..Default::default()
        },
    ];
    db.batch_insert_items(&ctx, &items).await.unwrap();
    let get = |item_ids: &[&str], opts: GetOptions| {
        let ids = strings(item_ids);
        let ctx = &ctx;
        async move { db.batch_get_items(ctx, &ids, &opts).await.unwrap() }
    };

    // Test basic batch get
    let got_items = get(&["1", "2", "3"], GetOptions::default()).await;
    assert_eq!(3, got_items.len());

    // Test batch get with empty list
    let empty_items = get(&[], GetOptions::default()).await;
    assert!(empty_items.is_empty());

    // Test batch get with non-existent items
    let non_existent = get(&["999", "1000"], GetOptions::default()).await;
    assert!(non_existent.is_empty());

    // Test batch get with category filter (AND logic - must contain all specified categories)
    let category_items = get(
        &["1", "2", "3", "4"],
        GetOptions {
            categories: strings(&["a"]),
            ..Default::default()
        },
    )
    .await;
    // items 1 and 4 have category "a"
    assert_eq!(2, category_items.len());

    // Test batch get with multiple category filter (AND logic - must contain both "a" AND "b")
    let multi_category_items = get(
        &["1", "2", "3", "4"],
        GetOptions {
            categories: strings(&["a", "b"]),
            ..Default::default()
        },
    )
    .await;
    // only item 4 has both "a" and "b"
    assert_eq!(1, multi_category_items.len());

    // Test batch get with category filter that matches nothing
    let no_match_items = get(
        &["1", "2", "3"],
        GetOptions {
            categories: strings(&["z"]),
            ..Default::default()
        },
    )
    .await;
    assert!(no_match_items.is_empty());

    // Test batch get with SkipHidden filter
    let visible_items = get(
        &["1", "2", "3"],
        GetOptions {
            skip_hidden: true,
            ..Default::default()
        },
    )
    .await;
    // items 1 and 2 are not hidden, item 3 is hidden
    assert_eq!(2, visible_items.len());

    // Test batch get with both category and SkipHidden filter
    let visible_category_items = get(
        &["1", "2", "3", "4"],
        GetOptions {
            categories: strings(&["a"]),
            skip_hidden: true,
            ..Default::default()
        },
    )
    .await;
    // items 1 and 4 have category "a", both are not hidden
    assert_eq!(2, visible_category_items.len());

    // Test batch get with ReturnId filter
    let id_only_items = get(
        &["1", "2", "3"],
        GetOptions {
            return_id: true,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(3, id_only_items.len());
    for item in &id_only_items {
        assert!(!item.item_id.is_empty());
        // other fields should be empty
        assert!(item.categories.as_ref().is_none_or(|c| c.is_empty()));
    }

    // Test batch get with After time filter (cutoff at item timestamp)
    // all items have timestamp exactly at 1996-03-15, so none are strictly after
    let after_items = get(
        &["1", "2", "3", "4"],
        GetOptions {
            after: Some(timestamp.clone()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(0, after_items.len());

    // Test batch get with After time filter (cutoff before item timestamps)
    let cutoff_before_items = date(1996, 3, 14, 0);
    let items_after_cutoff = get(
        &["1", "2", "3", "4"],
        GetOptions {
            after: Some(cutoff_before_items),
            ..Default::default()
        },
    )
    .await;
    // all requested items have timestamp after 1996-03-14
    assert_eq!(4, items_after_cutoff.len());
    teardown(db).await;
}

async fn test_delete_user(db: &dyn Backend) {
    setup(db).await;
    let ctx = ctx();
    // Insert ret
    let feedback_records: Vec<Feedback> = ["0", "2", "4", "6", "8"]
        .iter()
        .map(|item| {
            feedback(
                key(POSITIVE_FEEDBACK_TYPE, "a", item),
                0.0,
                date(1996, 3, 15, 0),
                "comment",
            )
        })
        .collect();
    db.batch_insert_feedback(&ctx, &feedback_records, true, true, true)
        .await
        .unwrap();
    // Delete user
    db.delete_user(&ctx, "a").await.unwrap();
    assert!(
        db.get_user(&ctx, "a").await.is_err(),
        "failed to delete user"
    );
    let ret = db
        .get_user_feedback(
            &ctx,
            "a",
            Some(gotime::now()),
            &[must_parse_feedback_type_expression(POSITIVE_FEEDBACK_TYPE)],
        )
        .await
        .unwrap();
    assert!(ret.is_empty());
    let ret = get_feedback(
        db,
        &ctx,
        100,
        None,
        Some(gotime::now()),
        &[POSITIVE_FEEDBACK_TYPE],
    )
    .await;
    assert!(ret.is_empty());
    teardown(db).await;
}

async fn test_delete_item(db: &dyn Backend) {
    setup(db).await;
    let ctx = ctx();
    // Insert ret
    let feedbacks: Vec<Feedback> = ["0", "1", "2", "3", "4"]
        .iter()
        .map(|user| {
            feedback(
                key(POSITIVE_FEEDBACK_TYPE, user, "b"),
                0.0,
                date(1996, 3, 15, 0),
                "comment",
            )
        })
        .collect();
    db.batch_insert_feedback(&ctx, &feedbacks, true, true, true)
        .await
        .unwrap();
    // Delete item
    db.delete_item(&ctx, "b").await.unwrap();
    assert!(
        db.get_item(&ctx, "b").await.is_err(),
        "failed to delete item"
    );
    let ret = db
        .get_item_feedback(&ctx, "b", &strings(&[POSITIVE_FEEDBACK_TYPE]))
        .await
        .unwrap();
    assert!(ret.is_empty());
    let ret = get_feedback(
        db,
        &ctx,
        100,
        None,
        Some(gotime::now()),
        &[POSITIVE_FEEDBACK_TYPE],
    )
    .await;
    assert!(ret.is_empty());
    teardown(db).await;
}

async fn test_delete_feedback(db: &dyn Backend) {
    setup(db).await;
    let ctx = ctx();
    let timestamp = date(1996, 3, 15, 0);
    let mut feedbacks = vec![
        feedback(key("type1", "2", "3"), 0.0, timestamp.clone(), "comment"),
        feedback(key("type2", "2", "3"), 0.0, timestamp.clone(), "comment"),
        feedback(key("type3", "2", "3"), 0.0, timestamp.clone(), "comment"),
        feedback(key("type1", "2", "4"), 0.0, timestamp.clone(), "comment"),
        feedback(key("type1", "1", "3"), 0.0, timestamp.clone(), "comment"),
    ];
    db.batch_insert_feedback(&ctx, &feedbacks, true, true, true)
        .await
        .unwrap();
    // set Updated for comparison
    for record in feedbacks.iter_mut() {
        record.updated = record.timestamp.clone();
    }
    // get user-item feedback
    let ret = db
        .get_user_item_feedback(&ctx, "2", "3", &[])
        .await
        .unwrap();
    assert_elements_match(&feedbacks[0..3], &ret);
    let feedback_type2 = "type2";
    let ret = db
        .get_user_item_feedback(&ctx, "2", "3", &strings(&[feedback_type2]))
        .await
        .unwrap();
    assert_eq!(vec![feedbacks[1].clone()], ret);
    // delete user-item feedback
    let delete_count = db
        .delete_user_item_feedback(&ctx, "2", "3", &[])
        .await
        .unwrap();
    assert_eq!(3, delete_count);
    db.optimize().await.unwrap();
    let ret = db
        .get_user_item_feedback(&ctx, "2", "3", &[])
        .await
        .unwrap();
    assert!(ret.is_empty());
    let feedback_type1 = "type1";
    let delete_count = db
        .delete_user_item_feedback(&ctx, "1", "3", &strings(&[feedback_type1]))
        .await
        .unwrap();
    assert_eq!(1, delete_count);
    let ret = db
        .get_user_item_feedback(&ctx, "1", "3", &strings(&[feedback_type2]))
        .await
        .unwrap();
    assert!(ret.is_empty());
    teardown(db).await;
}

async fn test_time_limit(db: &dyn Backend) {
    setup(db).await;
    let ctx = ctx();
    // insert items
    let items = vec![
        Item {
            item_id: "0".to_owned(),
            timestamp: date(1996, 3, 15, 0),
            labels: str_labels(&["a"]),
            comment: "comment 0".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "2".to_owned(),
            timestamp: date(1997, 3, 15, 0),
            labels: str_labels(&["a"]),
            comment: "comment 2".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "4".to_owned(),
            timestamp: date(1998, 3, 15, 0),
            labels: str_labels(&["a", "b"]),
            comment: "comment 4".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "6".to_owned(),
            timestamp: date(1999, 3, 15, 0),
            labels: str_labels(&["b"]),
            comment: "comment 6".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "8".to_owned(),
            timestamp: date(2000, 3, 15, 0),
            labels: str_labels(&["b"]),
            comment: "comment 8".to_owned(),
            ..Default::default()
        },
    ];
    db.batch_insert_items(&ctx, &items).await.unwrap();
    let time_limit = date(1998, 1, 1, 0);
    let (_, mut ret) = db
        .get_items(&ctx, "", 100, Some(time_limit.clone()))
        .await
        .unwrap();
    for item in ret.iter_mut() {
        assert!(!item.update_at.is_zero());
        item.update_at = Time::default();
    }
    assert_eq!(
        vec![items[2].clone(), items[3].clone(), items[4].clone()],
        ret
    );

    // insert feedback
    let mut feedbacks = vec![
        feedback(key("type1", "2", "3"), 0.0, date(1996, 3, 15, 0), "comment"),
        feedback(key("type2", "2", "3"), 0.0, date(1997, 3, 15, 0), "comment"),
        feedback(key("type3", "2", "3"), 0.0, date(1998, 3, 15, 0), "comment"),
        feedback(key("type1", "2", "4"), 0.0, date(1999, 3, 15, 0), "comment"),
        feedback(key("type1", "1", "3"), 0.0, date(2000, 3, 15, 0), "comment"),
    ];
    db.batch_insert_feedback(&ctx, &feedbacks, true, true, true)
        .await
        .unwrap();
    // set Updated for comparison
    for record in feedbacks.iter_mut() {
        record.updated = record.timestamp.clone();
    }
    let ret_feedback = get_feedback(
        db,
        &ctx,
        100,
        Some(time_limit.clone()),
        Some(gotime::now()),
        &[],
    )
    .await;
    assert_eq!(
        vec![
            feedbacks[4].clone(),
            feedbacks[3].clone(),
            feedbacks[2].clone()
        ],
        ret_feedback
    );
    let ret_feedback = get_feedback(
        db,
        &ctx,
        100,
        Some(time_limit),
        Some(gotime::now()),
        &["type1"],
    )
    .await;
    assert_eq!(
        vec![feedbacks[4].clone(), feedbacks[3].clone()],
        ret_feedback
    );

    // get user feedback with time limit
    for i in 0..100 {
        db.batch_insert_feedback(
            &ctx,
            &[Feedback {
                feedback_key: key("type", "user_2", &format!("item_{i}")),
                timestamp: gotime::now(),
                updated: gotime::now(),
                ..Default::default()
            }],
            true,
            true,
            true,
        )
        .await
        .unwrap();
        let feedback = db
            .get_user_feedback(
                &ctx,
                "user_2",
                Some(gotime::now()),
                &[must_parse_feedback_type_expression("type")],
            )
            .await
            .unwrap();
        assert_eq!(i + 1, feedback.len());
    }
    teardown(db).await;
}

async fn test_timezone(db: &dyn Backend) {
    setup(db).await;
    let ctx = ctx();
    let loc = gotime::load_location("Asia/Tokyo").unwrap();
    let past = || gotime::now().add(Duration(-SECOND.0)).in_location(&loc);
    let future = || gotime::now().add(HOUR).in_location(&loc);
    // insert feedbacks
    db.batch_insert_feedback(
        &ctx,
        &[
            feedback(key("read", "1", "1"), 0.0, past(), ""),
            feedback(key("read", "1", "2"), 0.0, past(), ""),
            feedback(key("read", "2", "2"), 0.0, past(), ""),
            feedback(key("like", "1", "1"), 0.0, future(), ""),
            feedback(key("like", "1", "2"), 0.0, future(), ""),
            feedback(key("like", "2", "2"), 0.0, future(), ""),
        ],
        true,
        true,
        true,
    )
    .await
    .unwrap();
    // get feedback stream
    let feedback = get_feedback_stream(db, &ctx, 10, &[with_end_time(gotime::now())]).await;
    assert_eq!(3, feedback.len());
    // get feedback
    let (_, feedback) = db
        .get_feedback(&ctx, "", 10, None, Some(gotime::now()), &[])
        .await
        .unwrap();
    assert_eq!(3, feedback.len());
    // get user feedback
    let feedback = db
        .get_user_feedback(&ctx, "1", Some(gotime::now()), &[])
        .await
        .unwrap();
    assert_eq!(2, feedback.len());
    // get item feedback
    // no future feedback by default
    let feedback = db.get_item_feedback(&ctx, "2", &[]).await.unwrap();
    assert_eq!(2, feedback.len());
    // get user item feedback
    // return future feedback by default
    let feedback = db
        .get_user_item_feedback(&ctx, "1", "1", &[])
        .await
        .unwrap();
    assert_eq!(2, feedback.len());

    // insert items
    let now = gotime::now().in_location(&loc);
    db.batch_insert_items(
        &ctx,
        &[
            Item {
                item_id: "100".to_owned(),
                timestamp: now.clone(),
                ..Default::default()
            },
            Item {
                item_id: "200".to_owned(),
                ..Default::default()
            },
        ],
    )
    .await
    .unwrap();
    db.modify_item(
        &ctx,
        "200",
        &ItemPatch {
            timestamp: Some(now.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.optimize().await.unwrap();
    let utc = gotime::utc();
    match db.time_stamps() {
        TimeStamps::Microseconds => {
            let item = db.get_item(&ctx, "100").await.unwrap();
            assert_eq!(now.round(MICROSECOND).in_location(&utc), item.timestamp);
            let item = db.get_item(&ctx, "200").await.unwrap();
            assert_eq!(now.round(MICROSECOND).in_location(&utc), item.timestamp);
        }
        TimeStamps::Exact => {
            let item = db.get_item(&ctx, "100").await.unwrap();
            assert_eq!(now.in_location(&utc), item.timestamp.in_location(&utc));
            let item = db.get_item(&ctx, "200").await.unwrap();
            assert_eq!(now.in_location(&utc), item.timestamp.in_location(&utc));
        }
        TimeStamps::Milliseconds => {
            let item = db.get_item(&ctx, "100").await.unwrap();
            assert_eq!(now.truncate(MILLISECOND).in_location(&utc), item.timestamp);
            let item = db.get_item(&ctx, "200").await.unwrap();
            assert_eq!(now.truncate(MILLISECOND).in_location(&utc), item.timestamp);
        }
        // Go skips the case for a store it does not know.
        TimeStamps::Unknown => {}
    }
    teardown(db).await;
}

async fn test_collation(db: &dyn Backend) {
    setup(db).await;
    let ctx = ctx();
    db.batch_insert_feedback(
        &ctx,
        &[
            feedback_only("type", "user", "A"),
            feedback_only("type", "user", "a"),
            feedback_only("type", "user", "B"),
            feedback_only("type", "user", "b"),
        ],
        true,
        true,
        true,
    )
    .await
    .unwrap();
    let feedbacks = get_feedback_stream(
        db,
        &ctx,
        10,
        &[with_begin_item_id("a"), with_end_item_id("B")],
    )
    .await;
    assert!(feedbacks.is_empty());
    teardown(db).await;
}

async fn test_search(db: &dyn Backend) {
    setup(db).await;
    let ctx = ctx();
    db.reconcile(&SearchConfig {
        columns: Some(strings(&[
            "item.ItemId",
            "item.Categories",
            "item.Labels.brand",
            "item.Comment",
        ])),
    })
    .await
    .unwrap();

    let items = vec![
        Item {
            item_id: "running-shoes".to_owned(),
            categories: Some(strings(&["sports", "shoes"])),
            labels: object_labels(&[("brand", Labels::String("acme".to_owned()))]),
            comment: "Lightweight running shoes for marathon training".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "trail-watch".to_owned(),
            categories: Some(strings(&["sports", "electronics"])),
            labels: object_labels(&[("brand", Labels::String("chrono".to_owned()))]),
            comment: "GPS watch for trail running and hiking".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "coffee-grinder".to_owned(),
            categories: Some(strings(&["kitchen"])),
            labels: object_labels(&[("brand", Labels::String("acme".to_owned()))]),
            comment: "Burr grinder for espresso and pour over coffee".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "office-chair".to_owned(),
            categories: Some(strings(&["office"])),
            labels: object_labels(&[("brand", Labels::String("ergon".to_owned()))]),
            comment: "Ergonomic chair with lumbar support".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "gorse-io:gorse".to_owned(),
            categories: Some(strings(&["repository"])),
            labels: object_labels(&[("brand", Labels::String("gorse".to_owned()))]),
            comment: "Open source recommender system".to_owned(),
            ..Default::default()
        },
    ];
    db.batch_insert_items(&ctx, &items).await.unwrap();
    db.optimize().await.unwrap();

    let search_item_ids = |query: &'static str, n: i64| {
        let ctx = &ctx;
        async move {
            let result: Vec<ScoredItem> = db.search_items(ctx, query, n).await.unwrap();
            result
                .into_iter()
                .map(|item| item.item.item_id)
                .collect::<Vec<_>>()
        }
    };

    assert_elements_match(
        &strings(&["gorse-io:gorse"]),
        &search_item_ids("gorse-io:gorse", 10).await,
    );
    assert_elements_match(
        &strings(&["running-shoes", "trail-watch"]),
        &search_item_ids("running", 10).await,
    );
    assert_elements_match(
        &strings(&["coffee-grinder"]),
        &search_item_ids("coffee", 10).await,
    );
    assert_elements_match(
        &strings(&["running-shoes", "trail-watch", "coffee-grinder"]),
        &search_item_ids("running coffee", 10).await,
    );
    assert_elements_match(
        &strings(&["trail-watch"]),
        &search_item_ids("electronics", 10).await,
    );
    assert_elements_match(
        &strings(&["running-shoes", "coffee-grinder"]),
        &search_item_ids("acme", 10).await,
    );
    assert_eq!(1, search_item_ids("running", 1).await.len());
    let result = db.search_items(&ctx, "coffee", 10).await.unwrap();
    for item in &result {
        assert_ne!(0.0, item.score);
    }
    teardown(db).await;
}

async fn test_purge(db: &dyn Backend) {
    setup(db).await;
    let ctx = ctx();
    // insert data
    let records: Vec<Feedback> = (0..100)
        .map(|t| feedback_only("click", &t.to_string(), &t.to_string()))
        .collect();
    db.batch_insert_feedback(&ctx, &records, true, true, true)
        .await
        .unwrap();
    let (_, users) = db.get_users(&ctx, "", 100).await.unwrap();
    assert_eq!(100, users.len());
    let (_, items) = db.get_items(&ctx, "", 100, None).await.unwrap();
    assert_eq!(100, items.len());
    let (_, feedbacks) = db
        .get_feedback(&ctx, "", 100, None, Some(gotime::now()), &[])
        .await
        .unwrap();
    assert_eq!(100, feedbacks.len());
    // purge data
    db.purge().await.unwrap();
    let (_, users) = db.get_users(&ctx, "", 100).await.unwrap();
    assert!(users.is_empty());
    let (_, items) = db.get_items(&ctx, "", 100, None).await.unwrap();
    assert!(items.is_empty());
    let (_, feedbacks) = db
        .get_feedback(&ctx, "", 100, None, Some(gotime::now()), &[])
        .await
        .unwrap();
    assert!(feedbacks.is_empty());
    // purge empty database
    db.purge().await.unwrap();
    teardown(db).await;
}
