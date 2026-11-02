// Ports: client/setup-test.sh and client/client_test.go @ add21b5 (Leg A); src/lib.rs `tests` and `blocking::tests` of gorse-io/gorse-rs @ 2de2691 (Leg B).

//! The WP-5-2 acceptance tests, the SDK suites against the clusters of the strangler
//! plan. Each leg is set up by the harness of `common/sdk_suite.rs` (WP-4-7), and each
//! leg fails unless every suite test runs and passes.
//!
//! Leg A is the Go SDK suite (`client/client_test.go`, run as `TestGorseClientTestSuite`)
//! against three mixes: a Go master with Rust workers and Go servers, a Go master with
//! Rust workers and Rust servers, and an all-Rust cluster.
//!
//! Leg B is the Rust SDK suite, `gorse_rs` (the tests of `src/lib.rs` in
//! `gorse-io/gorse-rs` at 2de2691, vendored under `third_party/gorse-rs`). Its tests
//! are ported here with the same cases and assertions. The only change is the entry
//! point: the suite calls `http://127.0.0.1:8088/`, the port at which `gorse-in-one`
//! serves the REST API, and in a cluster the REST API is the server's, so each leg
//! passes its server's HTTP port. Leg B runs against the all-Rust cluster, and against
//! the all-Go cluster as the control that the suite agrees with the oracle.

use std::panic::AssertUnwindSafe;
use std::sync::PoisonError;

mod common;

use common::sdk_suite::{LEG_TURN, LEGS, Mix, leg, start_leg};

/// Leg A, the first mix: a Go master with Rust workers and Go servers.
#[test]
fn sdk_go_master_rust_workers_go_servers() {
    leg(LEGS[1]);
}

/// Leg A, the second mix: a Go master with Rust workers and Rust servers.
#[test]
fn sdk_go_master_rust_workers_rust_servers() {
    leg(LEGS[2]);
}

/// Leg A, the third mix: a Rust master with Rust workers and Rust servers.
#[test]
fn sdk_all_rust() {
    leg(LEGS[3]);
}

/// Leg B against the all-Rust cluster: the `gorse_rs` suite.
#[test]
fn sdk_rs_suite_all_rust() {
    rs_leg(LEGS[3]);
}

/// Leg B against the all-Go cluster, the control: the suite must agree with the oracle.
#[test]
fn sdk_rs_suite_all_go() {
    rs_leg(LEGS[0]);
}

/// The `gorse_rs` suite's asynchronous tests, `tests` in `src/lib.rs`.
mod async_suite {
    use gorse_rs::{Error, Feedback, Gorse, Item, RecommendOptions, User};
    use reqwest::StatusCode;
    use serde_json::json;

    type Result<T> = std::result::Result<T, Error>;

    const API_KEY: &str = "zhenghaoz";

    pub async fn test_users(entry: &str) -> Result<()> {
        let client = Gorse::new(entry, API_KEY);
        let user = User {
            user_id: "2000".into(),
            labels: json!({
                "gender": "M",
                "occupation": "engineer"
            }),
            comment: "zhenghaoz".into(),
        };
        let r = client.insert_user(&user).await?;
        assert_eq!(r.row_affected, 1);
        let resp = client.get_user("2000").await?;
        assert_eq!(user, resp);

        let r = client.delete_user("2000").await?;
        assert_eq!(r.row_affected, 1);
        match client.get_user("2000").await {
            Ok(_) => panic!("Expected error"),
            Err(Error::Api { status_code, .. }) => {
                assert_eq!(status_code, StatusCode::NOT_FOUND);
            }
            Err(e) => panic!("Expected API error, got {:?}", e),
        }
        Ok(())
    }

    pub async fn test_items(entry: &str) -> Result<()> {
        let client = Gorse::new(entry, API_KEY);
        let item = Item {
            item_id: "2000".into(),
            is_hidden: true,
            labels: json!({
                "embedding": [0.1, 0.2, 0.3]
            }),
            categories: vec!["Comedy".into(), "Animation".into()],
            timestamp: "2022-11-20T13:55:27Z".into(),
            comment: "Minions (2015)".into(),
        };
        let r = client.insert_item(&item).await?;
        assert_eq!(r.row_affected, 1);
        let resp = client.get_item("2000").await?;
        assert_eq!(item, resp);

        let result = client.search_items("Toy Story", 3).await?;
        assert!(!result.items.is_empty());
        assert_eq!(result.items[0].comment, "Toy Story (1995)");

        let r = client.delete_item("2000").await?;
        assert_eq!(r.row_affected, 1);
        match client.get_item("2000").await {
            Ok(_) => panic!("Expected error"),
            Err(Error::Api { status_code, .. }) => {
                assert_eq!(status_code, StatusCode::NOT_FOUND);
            }
            Err(e) => panic!("Expected API error, got {:?}", e),
        }
        Ok(())
    }

    pub async fn test_feedback(entry: &str) -> Result<()> {
        let client = Gorse::new(entry, API_KEY);
        client
            .insert_user(&User {
                user_id: "2000".into(),
                labels: json!({}),
                comment: "".into(),
            })
            .await?;

        let feedbacks = vec![
            Feedback {
                feedback_type: "watch".into(),
                user_id: "2000".into(),
                item_id: "1".into(),
                value: 1.0,
                timestamp: "2022-11-20T13:55:27Z".into(),
            },
            Feedback {
                feedback_type: "watch".into(),
                user_id: "2000".into(),
                item_id: "1060".into(),
                value: 2.0,
                timestamp: "2022-11-20T13:55:27Z".into(),
            },
            Feedback {
                feedback_type: "watch".into(),
                user_id: "2000".into(),
                item_id: "11".into(),
                value: 3.0,
                timestamp: "2022-11-20T13:55:27Z".into(),
            },
        ];
        for fb in &feedbacks {
            client.delete_feedback(&fb.user_id, &fb.item_id).await?;
        }
        let r = client.insert_feedback(&feedbacks).await?;
        assert_eq!(r.row_affected, 3);
        let user_feedback = client.list_feedback("2000", "watch").await?;
        assert_eq!(feedbacks, user_feedback);

        let r = client.delete_feedback("2000", "1").await?;
        assert_eq!(r.row_affected, 1);
        let user_feedback = client.list_feedback("2000", "watch").await?;
        assert_eq!(
            vec![feedbacks[1].clone(), feedbacks[2].clone()],
            user_feedback
        );
        Ok(())
    }

    pub async fn test_item_to_item(entry: &str) -> Result<()> {
        let client = Gorse::new(entry, API_KEY);
        let scores = client.get_item_neighbors("1").await?;
        assert_eq!(scores[0].id, "1060".to_string());
        assert_eq!(scores[1].id, "404".to_string());
        assert_eq!(scores[2].id, "1219".to_string());
        Ok(())
    }

    pub async fn test_recommend(entry: &str) -> Result<()> {
        let client = Gorse::new(entry, API_KEY);
        client
            .insert_user(&User {
                user_id: "3000".into(),
                labels: json!({}),
                comment: "".into(),
            })
            .await?;
        let items = client
            .get_recommend(
                "3000",
                RecommendOptions {
                    n: 3,
                    ..Default::default()
                },
            )
            .await?;
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, "315");
        assert_eq!(items[1].id, "1432");
        assert_eq!(items[2].id, "918");
        Ok(())
    }

    pub async fn test_recommend_with_multiple_categories(entry: &str) -> Result<()> {
        let client = Gorse::new(entry, API_KEY);
        client
            .insert_user(&User {
                user_id: "4000".into(),
                labels: json!({}),
                comment: "".into(),
            })
            .await?;
        let items = client
            .get_recommend(
                "4000",
                RecommendOptions {
                    n: 3,
                    categories: vec!["Drama".into(), "Comedy".into()],
                    write_back_type: Some("recommend".into()),
                    write_back_delay: Some("1h".into()),
                    ..Default::default()
                },
            )
            .await?;
        assert_eq!(items.len(), 3);
        for score in items {
            let item = client.get_item(&score.id).await?;
            assert!(
                item.categories
                    .iter()
                    .any(|category| category == "Drama" || category == "Comedy")
            );
        }
        Ok(())
    }
}

/// The `gorse_rs` suite's blocking tests, `blocking::tests` in `src/lib.rs`.
mod blocking_suite {
    use gorse_rs::blocking::Gorse;
    use gorse_rs::{Error, Feedback, Item, RecommendOptions, User};
    use reqwest::StatusCode;
    use serde_json::json;

    type Result<T> = std::result::Result<T, Error>;

    const API_KEY: &str = "zhenghaoz";

    pub fn test_users(entry: &str) -> Result<()> {
        let client = Gorse::new(entry, API_KEY);
        let user = User {
            user_id: "2000".into(),
            labels: json!({
                "gender": "M",
                "occupation": "engineer"
            }),
            comment: "zhenghaoz".into(),
        };
        let r = client.insert_user(&user)?;
        assert_eq!(r.row_affected, 1);
        let resp = client.get_user("2000")?;
        assert_eq!(user, resp);

        let r = client.delete_user("2000")?;
        assert_eq!(r.row_affected, 1);
        match client.get_user("2000") {
            Ok(_) => panic!("Expected error"),
            Err(Error::Api { status_code, .. }) => {
                assert_eq!(status_code, StatusCode::NOT_FOUND);
            }
            Err(e) => panic!("Expected API error, got {:?}", e),
        }
        Ok(())
    }

    pub fn test_items(entry: &str) -> Result<()> {
        let client = Gorse::new(entry, API_KEY);
        let item = Item {
            item_id: "2000".into(),
            is_hidden: true,
            labels: json!({
                "embedding": [0.1, 0.2, 0.3]
            }),
            categories: vec!["Comedy".into(), "Animation".into()],
            timestamp: "2022-11-20T13:55:27Z".into(),
            comment: "Minions (2015)".into(),
        };
        let r = client.insert_item(&item)?;
        assert_eq!(r.row_affected, 1);
        let resp = client.get_item("2000")?;
        assert_eq!(item, resp);

        let result = client.search_items("Toy Story", 3)?;
        assert!(!result.items.is_empty());
        assert_eq!(result.items[0].comment, "Toy Story (1995)");

        let r = client.delete_item("2000")?;
        assert_eq!(r.row_affected, 1);
        match client.get_item("2000") {
            Ok(_) => panic!("Expected error"),
            Err(Error::Api { status_code, .. }) => {
                assert_eq!(status_code, StatusCode::NOT_FOUND);
            }
            Err(e) => panic!("Expected API error, got {:?}", e),
        }
        Ok(())
    }

    pub fn test_feedback(entry: &str) -> Result<()> {
        let client = Gorse::new(entry, API_KEY);
        client.insert_user(&User {
            user_id: "2000".into(),
            labels: json!({}),
            comment: "".into(),
        })?;

        let feedbacks = vec![
            Feedback {
                feedback_type: "watch".into(),
                user_id: "2000".into(),
                item_id: "1".into(),
                value: 1.0,
                timestamp: "2022-11-20T13:55:27Z".into(),
            },
            Feedback {
                feedback_type: "watch".into(),
                user_id: "2000".into(),
                item_id: "1060".into(),
                value: 2.0,
                timestamp: "2022-11-20T13:55:27Z".into(),
            },
            Feedback {
                feedback_type: "watch".into(),
                user_id: "2000".into(),
                item_id: "11".into(),
                value: 3.0,
                timestamp: "2022-11-20T13:55:27Z".into(),
            },
        ];
        for fb in &feedbacks {
            client.delete_feedback(&fb.user_id, &fb.item_id)?;
        }
        let r = client.insert_feedback(&feedbacks)?;
        assert_eq!(r.row_affected, 3);
        let user_feedback = client.list_feedback("2000", "watch")?;
        assert_eq!(feedbacks, user_feedback);

        let r = client.delete_feedback("2000", "1")?;
        assert_eq!(r.row_affected, 1);
        let user_feedback = client.list_feedback("2000", "watch")?;
        assert_eq!(
            vec![feedbacks[1].clone(), feedbacks[2].clone()],
            user_feedback
        );
        Ok(())
    }

    pub fn test_item_to_item(entry: &str) -> Result<()> {
        let client = Gorse::new(entry, API_KEY);
        let scores = client.get_item_neighbors("1")?;
        assert_eq!(scores[0].id, "1060".to_string());
        assert_eq!(scores[1].id, "404".to_string());
        assert_eq!(scores[2].id, "1219".to_string());
        Ok(())
    }

    pub fn test_recommend(entry: &str) -> Result<()> {
        let client = Gorse::new(entry, API_KEY);
        client.insert_user(&User {
            user_id: "3000".into(),
            labels: json!({}),
            comment: "".into(),
        })?;
        let items = client.get_recommend(
            "3000",
            RecommendOptions {
                n: 3,
                ..Default::default()
            },
        )?;
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, "315");
        assert_eq!(items[1].id, "1432");
        assert_eq!(items[2].id, "918");
        Ok(())
    }
}

/// Starts the nodes of one leg, and runs the `gorse_rs` suite against its server. The
/// tests run one after another in the order `cargo test` starts them under
/// `#[serial]`: by name, with the blocking module's names first. Every test runs,
/// even after one fails, and the leg fails with the list of the failures.
fn rs_leg(mix: Mix) {
    let _turn = LEG_TURN.lock().unwrap_or_else(PoisonError::into_inner);
    let runtime = tokio::runtime::Runtime::new().expect("a tokio runtime");
    let cluster = runtime
        .block_on(start_leg(mix))
        .unwrap_or_else(|err| panic!("{}: {err}", mix.label));
    let entry = format!("http://127.0.0.1:{}/", cluster.ports.server_http);
    let entry = entry.as_str();

    type Test<'a> = Box<dyn FnOnce() -> std::result::Result<(), gorse_rs::Error> + 'a>;
    let tests: Vec<(&str, Test<'_>)> = vec![
        (
            "blocking::test_feedback",
            Box::new(|| blocking_suite::test_feedback(entry)),
        ),
        (
            "blocking::test_item_to_item",
            Box::new(|| blocking_suite::test_item_to_item(entry)),
        ),
        (
            "blocking::test_items",
            Box::new(|| blocking_suite::test_items(entry)),
        ),
        (
            "blocking::test_recommend",
            Box::new(|| blocking_suite::test_recommend(entry)),
        ),
        (
            "blocking::test_users",
            Box::new(|| blocking_suite::test_users(entry)),
        ),
        (
            "test_feedback",
            Box::new(|| runtime.block_on(async_suite::test_feedback(entry))),
        ),
        (
            "test_item_to_item",
            Box::new(|| runtime.block_on(async_suite::test_item_to_item(entry))),
        ),
        (
            "test_items",
            Box::new(|| runtime.block_on(async_suite::test_items(entry))),
        ),
        (
            "test_recommend",
            Box::new(|| runtime.block_on(async_suite::test_recommend(entry))),
        ),
        (
            "test_recommend_with_multiple_categories",
            Box::new(|| {
                runtime.block_on(async_suite::test_recommend_with_multiple_categories(entry))
            }),
        ),
        (
            "test_users",
            Box::new(|| runtime.block_on(async_suite::test_users(entry))),
        ),
    ];

    let total = tests.len();
    let mut failures = Vec::new();
    for (name, test) in tests {
        match std::panic::catch_unwind(AssertUnwindSafe(test)) {
            Ok(Ok(())) => eprintln!("{}: gorse_rs {name} passed", mix.label),
            Ok(Err(err)) => failures.push(format!("{name}: {err:?}")),
            Err(_) => failures.push(format!("{name}: panicked")),
        }
    }
    if failures.is_empty() {
        eprintln!(
            "{}: the gorse_rs suite ran {total} tests and all passed",
            mix.label
        );
    } else {
        panic!(
            "{}: the gorse_rs suite did not pass:\n{}",
            mix.label,
            failures.join("\n")
        );
    }
}
