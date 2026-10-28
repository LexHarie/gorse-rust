// Ports: storage/cache/no_database_test.go @ add21b5

use gorse_common::gocontext::Context;
use gorse_common::gotime::{Duration, Time};
use gorse_storage_core::errors::Error as StorageError;

use super::no_database::NoDatabase;
use super::{Database, Error, ScoreCondition, ScorePatch, key};

/// Asserts that a call failed with `ErrNoDatabase`.
fn assert_no_database<T>(result: Result<T, Error>) {
    let err = result.err().expect("the call must fail");
    assert_eq!(err.kind(), Some(StorageError::NoDatabase), "{err}");
}

#[tokio::test]
async fn test_no_database() {
    let ctx = Context::background();
    let database = NoDatabase;
    assert_no_database(database.close().await);
    assert_no_database(database.ping().await);
    assert_no_database(database.init().await);
    assert_no_database(database.scan(&mut |_: &str| Ok(())).await);
    assert_no_database(database.purge().await);
    assert_no_database(database.set(&ctx, &[]).await);
    let value = database.get(&ctx, &key(&["", ""])).await;
    assert_no_database(value.string());
    assert_no_database(value.integer());
    assert_no_database(value.time());
    assert_no_database(database.delete(&ctx, &key(&["", ""])).await);

    assert_no_database(database.add_scores(&ctx, "", "", &[]).await);
    assert_no_database(database.search_scores(&ctx, "", "", &[], 0, 0).await);
    assert_no_database(
        database
            .update_scores(&ctx, &[], None, "", &ScorePatch::default())
            .await,
    );
    assert_no_database(
        database
            .delete_scores(&ctx, &[], &ScoreCondition::default())
            .await,
    );
    assert_no_database(
        database
            .scan_scores(&ctx, &mut |_: &str, _: &str, _: &str, _: Time| Ok(()))
            .await,
    );

    assert_no_database(database.add_time_series_points(&ctx, &[]).await);
    assert_no_database(
        database
            .get_time_series_points(&ctx, "", Time::default(), Time::default(), Duration(0))
            .await,
    );
}
