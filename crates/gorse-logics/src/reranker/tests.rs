// Ports: logics/reranker_test.go @ add21b5

use std::sync::Arc;
use std::thread;

use gorse_common::gocontext;
use gorse_common::reranker::MockServer;
use gorse_config::config::schema::{DecisionAPIConfig, RerankerAPIConfig};
use gorse_storage::data::types::{Item, User};

use super::{Decision, FeedbackItem, Reranker, new_score};
use crate::testing::{Reply, serve};

#[test]
fn reranker() {
    let server = Arc::new(MockServer::new());
    let serving = {
        let server = Arc::clone(&server);
        thread::spawn(move || server.start())
    };
    server.ready();

    let reranker = Reranker::new(&RerankerAPIConfig {
        auth_token: server.auth_token().to_owned(),
        url: server.url(),
        model: "gte-rerank".to_owned(),
        query_template: "{{ user.UserId }} is a {{ user.Comment }} watched the following movies recently: {% for item in feedback %}{{ item.Comment }}, {% endfor %}".to_owned(),
        document_template: "{{ item.Comment }}".to_owned(),
    })
    .expect("the templates parse");

    let items = reranker.rank(
        &gocontext::Context::background(),
        Some(&User {
            user_id: "Tom".to_owned(),
            comment: "horror movie enthusiast".to_owned(),
            ..User::default()
        }),
        &[
            FeedbackItem {
                feedback_type: String::new(),
                item: Item {
                    item_id: "tt0387564".to_owned(),
                    comment: "Saw".to_owned(),
                    ..Item::default()
                },
            },
            FeedbackItem {
                feedback_type: String::new(),
                item: Item {
                    item_id: "tt0432348".to_owned(),
                    comment: "Saw II".to_owned(),
                    ..Item::default()
                },
            },
            FeedbackItem {
                feedback_type: String::new(),
                item: Item {
                    item_id: "tt0435761".to_owned(),
                    comment: "Saw III".to_owned(),
                    ..Item::default()
                },
            },
        ],
        &[
            Item {
                item_id: "tt1233227".to_owned(),
                comment: "Harry Potter and the Half-Blood Prince".to_owned(),
                ..Item::default()
            },
            Item {
                item_id: "tt0926084".to_owned(),
                comment: "Harry Potter and the Deathly Hallows: Part 1".to_owned(),
                ..Item::default()
            },
            Item {
                item_id: "tt0890870".to_owned(),
                comment: "Saw IV".to_owned(),
                ..Item::default()
            },
            Item {
                item_id: "tt1132626".to_owned(),
                comment: "Saw VI".to_owned(),
                ..Item::default()
            },
            Item {
                item_id: "tt0435761".to_owned(),
                comment: "Saw V".to_owned(),
                ..Item::default()
            },
        ],
    );

    server.close().expect("close the mock server");
    serving
        .join()
        .expect("the mock server thread does not panic")
        .expect("the mock server serves until it is closed");

    assert_eq!(
        items.expect("the rerank succeeds"),
        vec![
            new_score("tt1233227".to_owned(), 1.0),
            new_score("tt0926084".to_owned(), 0.5),
            new_score("tt0890870".to_owned(), 0.333_333_333_333_333_3),
            new_score("tt1132626".to_owned(), 0.25),
            new_score("tt0435761".to_owned(), 0.2),
        ]
    );
}

#[test]
fn decision() {
    let (url, received) = serve(|_| {
        Reply::ok(
            r#"{"answers":{"a":{"type":"noul","noul":0.15},"b":{"type":"noul","noul":0.85}}}"#,
        )
    });
    let ranker = Decision::new(&DecisionAPIConfig {
        auth_token: "test-key".to_owned(),
        url,
        model: "test-model".to_owned(),
        state_template: "{{user.UserId}}: {% for f in feedback %}{{f.ItemId}}{% endfor %}"
            .to_owned(),
        instructions_template: "{% raw %}{{not a template}}{% endraw %} {{item.Comment}}"
            .to_owned(),
    })
    .expect("the templates parse");

    let scores = ranker.rank(
        &gocontext::Context::background(),
        Some(&User {
            user_id: "u1".to_owned(),
            ..User::default()
        }),
        &[FeedbackItem {
            feedback_type: String::new(),
            item: Item {
                item_id: "history".to_owned(),
                ..Item::default()
            },
        }],
        &[
            Item {
                item_id: "a".to_owned(),
                comment: "first".to_owned(),
                ..Item::default()
            },
            Item {
                item_id: "b".to_owned(),
                comment: "second".to_owned(),
                ..Item::default()
            },
        ],
    );

    let request = received
        .recv()
        .expect("the decisions API received a request");
    assert_eq!(request.header("Authorization"), Some("Bearer test-key"));
    let body: serde_json::Value =
        serde_json::from_slice(&request.body).expect("the request body is JSON");
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["state"], "u1: history");
    let questions = body["questions"]
        .as_object()
        .expect("questions are an object");
    assert_eq!(questions.len(), 2);
    assert_eq!(questions["a"]["type"], "noul");
    assert_eq!(questions["a"]["instructions"], "{{not a template}} first");
    assert!(questions["a"].get("criteria").is_none());

    assert_eq!(
        scores.expect("the decision succeeds"),
        vec![
            new_score("b".to_owned(), 0.85),
            new_score("a".to_owned(), 0.15),
        ]
    );

    let empty = ranker.rank(&gocontext::Context::background(), None, &[], &[]);
    assert!(empty.expect("nothing to rank").is_empty());
}
