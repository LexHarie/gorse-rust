// Ports: storage/data/no_database_test.go @ add21b5

use std::fmt::Debug;

use futures_core::stream::BoxStream;
use futures_util::StreamExt;
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_config::config::SearchConfig;
use gorse_storage_core::errors::Error as StorageError;

use super::no_database::NoDatabase;
use super::types::{ItemPatch, UserPatch};
use super::{Database, Error, GetOptions};

/// Asserts that a call failed with `ErrNoDatabase`.
fn assert_no_database<T: Debug>(result: Result<T, Error>) {
    let err = result.expect_err("the call must fail");
    assert_eq!(err.kind(), Some(StorageError::NoDatabase), "{err}");
}

/// Asserts that a stream's first item is `ErrNoDatabase`, as Go's error
/// channel is read first.
async fn assert_stream_no_database<T: Debug>(mut stream: BoxStream<'_, Result<T, Error>>) {
    let first = stream.next().await.expect("the stream must send an error");
    assert_no_database(first);
}

#[tokio::test]
async fn test_no_database() {
    let ctx = Context::background();
    let database = NoDatabase;

    assert_no_database(database.close().await);
    assert_no_database(database.optimize().await);
    assert_no_database(database.init().await);
    assert_no_database(database.reconcile(&SearchConfig::default()).await);
    assert_no_database(database.ping().await);
    assert_no_database(database.purge().await);

    assert_no_database(database.batch_insert_items(&ctx, &[]).await);
    assert_no_database(
        database
            .batch_get_items(&ctx, &[], &GetOptions::default())
            .await,
    );
    assert_no_database(database.modify_item(&ctx, "", &ItemPatch::default()).await);
    assert_no_database(database.get_item(&ctx, "").await);
    assert_no_database(database.search_items(&ctx, "", 0).await);
    assert_no_database(database.get_items(&ctx, "", 0, None).await);
    assert_no_database(database.delete_item(&ctx, "").await);
    assert_stream_no_database(database.get_item_stream(&ctx, 0, None)).await;

    assert_no_database(database.batch_insert_users(&ctx, &[]).await);
    assert_no_database(database.get_user(&ctx, "").await);
    assert_no_database(database.modify_user(&ctx, "", &UserPatch::default()).await);
    assert_no_database(database.get_users(&ctx, "", 0).await);
    assert_no_database(database.delete_user(&ctx, "").await);
    assert_stream_no_database(database.get_user_stream(&ctx, 0)).await;

    assert_no_database(
        database
            .batch_insert_feedback(&ctx, &[], false, false, false)
            .await,
    );
    assert_no_database(
        database
            .batch_insert_feedback(&ctx, &[], false, false, false)
            .await,
    );
    assert_no_database(
        database
            .get_user_feedback(&ctx, "", Some(gotime::now()), &[])
            .await,
    );
    assert_no_database(database.get_item_feedback(&ctx, "", &[]).await);
    assert_no_database(
        database
            .get_feedback(&ctx, "", 0, None, Some(gotime::now()), &[])
            .await,
    );
    assert_no_database(database.get_user_item_feedback(&ctx, "", "", &[]).await);
    assert_no_database(database.delete_user_item_feedback(&ctx, "", "", &[]).await);
    assert_stream_no_database(database.get_feedback_stream(&ctx, 0, &[])).await;

    assert_no_database(database.count_users(&ctx).await);
    assert_no_database(database.count_items(&ctx).await);
    assert_no_database(database.count_feedback(&ctx).await);
}
