// Ports: server/rest_test.go (TestSearchItems, TestSearchEmbedding) @ add21b5

//! Item search by full text and by embedding, as Go's `TestSearchItems` and
//! `TestSearchEmbedding`. The embedding service is a local server that records
//! each request, so the test checks the requests after the reply, as Go's
//! handler checks them with `suite.Equal`.

use std::future::IntoFuture;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::routing::post;
use gorse_common::gocontext::Context;
use gorse_common::mock;
use gorse_storage::data::types::{Item, Labels};
use gorse_storage::vectors::{self, Distance, Vector, VectorConfig};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use super::{Suite, get};

/// One request the embedding service received.
#[derive(Debug, Default, Clone)]
struct Recorded {
    method: String,
    path: String,
    authorization: String,
    body: Value,
}

/// The embedding the service returns, and the requests it has received.
struct EmbeddingService {
    embedding: Vec<f32>,
    recorded: Mutex<Vec<Recorded>>,
}

/// Go's `httptest.NewServer` for the embedding endpoint. It answers every
/// request with the embedding and records it.
async fn start_embedding_service(embedding: Vec<f32>) -> (String, Arc<EmbeddingService>) {
    let service = Arc::new(EmbeddingService {
        embedding,
        recorded: Mutex::new(Vec::new()),
    });
    let app = Router::new()
        .route("/v1/embeddings", post(embeddings))
        .with_state(service.clone());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the embedding service");
    let address = listener
        .local_addr()
        .expect("the embedding service address");
    tokio::spawn(axum::serve(listener, app).into_future());
    (format!("http://{address}"), service)
}

async fn embeddings(
    State(service): State<Arc<EmbeddingService>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> axum::Json<Value> {
    let authorization = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
    service.recorded.lock().expect("the record").push(Recorded {
        method: method.to_string(),
        path: uri.path().to_owned(),
        authorization,
        body,
    });
    axum::Json(json!({"data": [{"embedding": service.embedding}]}))
}

/// Go's `Labels: map[string]any{...}` of a stored item.
fn labels(value: Value) -> Labels {
    serde_json::from_value(value).expect("labels")
}

#[tokio::test]
async fn search_items() {
    let suite = Suite::default_suite().await;

    suite
        .send(get("/api/items?q=running&n=10").key())
        .await
        .status(StatusCode::BAD_REQUEST)
        .text("item search is not supported because [recommend.search].columns is empty");

    suite.update_config(|c| {
        c.recommend.search.columns = Some(vec![
            "item.ItemId".to_owned(),
            "item.Categories".to_owned(),
            "item.Labels.brand".to_owned(),
            "item.Comment".to_owned(),
        ])
    });
    let config = (*suite.rest.config()).clone();
    suite
        .data
        .reconcile(&config.recommend.search)
        .await
        .expect("reconcile the search columns");

    let items = vec![
        Item {
            item_id: "running-shoes".to_owned(),
            categories: Some(vec!["sports".to_owned(), "shoes".to_owned()]),
            labels: labels(json!({"brand": "acme"})),
            comment: "Lightweight running shoes for marathon training".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "coffee-grinder".to_owned(),
            categories: Some(vec!["kitchen".to_owned()]),
            labels: labels(json!({"brand": "acme"})),
            comment: "Burr grinder for espresso and pour over coffee".to_owned(),
            ..Default::default()
        },
    ];
    let ctx = Context::background();
    suite
        .data
        .batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");
    suite.data.optimize().await.expect("optimize the store");

    suite
        .send(get("/api/items?q=espresso&n=10").key())
        .await
        .status(StatusCode::OK)
        .json(json!({
            "Cursor": "",
            "Items": [{
                "ItemId": "coffee-grinder",
                "IsHidden": false,
                "Categories": ["kitchen"],
                "Timestamp": "0001-01-01T00:00:00Z",
                "Labels": {"brand": "acme"},
                "Comment": "Burr grinder for espresso and pour over coffee",
            }],
        }));
}

#[tokio::test]
async fn search_embedding() {
    let suite = Suite::default_suite().await;
    const QUERY: &str = "semantic query 推荐系统";
    let embedding = mock::hash(QUERY);
    let (endpoint, service) = start_embedding_service(embedding.clone()).await;

    suite.update_config(|c| {
        c.openai.base_url = format!("{endpoint}/v1");
        c.openai.auth_token = "embedding-token".to_owned();
        c.openai.embedding_model = "test-embedding-model".to_owned();
        c.openai.embedding_dimensions = embedding.len() as i64;
        c.recommend.item_to_item = Some(vec![gorse_config::config::schema::ItemToItemConfig {
            name: "semantic".to_owned(),
            kind: "embedding".to_owned(),
            ..Default::default()
        }]);
    });
    let config = (*suite.rest.config()).clone();
    assert!(
        config
            .recommend
            .search
            .columns
            .as_ref()
            .is_none_or(Vec::is_empty)
    );

    let items = vec![
        Item {
            item_id: "a-far".to_owned(),
            categories: Some(vec!["far".to_owned()]),
            comment: "First alphabetically".to_owned(),
            labels: labels(json!({"rank": "far"})),
            ..Default::default()
        },
        Item {
            item_id: "z-near".to_owned(),
            categories: Some(vec!["near".to_owned()]),
            comment: "Most similar".to_owned(),
            labels: labels(json!({"rank": "near"})),
            ..Default::default()
        },
        Item {
            item_id: "hidden".to_owned(),
            is_hidden: true,
            ..Default::default()
        },
    ];
    let ctx = Context::background();
    suite
        .data
        .batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");

    let mut far = embedding.clone();
    let mut near = embedding.clone();
    far[0] += 100.0;
    near[0] += 10.0;
    let collection = vectors::item_to_item_collection("semantic");
    suite
        .vectors
        .add_collection(
            &ctx,
            &collection,
            embedding.len() as i64,
            Distance::Euclidean,
            VectorConfig::default(),
        )
        .await
        .expect("add the collection");
    suite
        .vectors
        .add_vectors(
            &ctx,
            &collection,
            &[
                Vector {
                    id: items[0].item_id.clone(),
                    values: far,
                    ..Default::default()
                },
                Vector {
                    id: items[1].item_id.clone(),
                    values: near,
                    ..Default::default()
                },
                Vector {
                    id: items[2].item_id.clone(),
                    values: embedding.clone(),
                    is_hidden: true,
                    ..Default::default()
                },
            ],
        )
        .await
        .expect("add the vectors");

    let uri = format!("/api/items?q={}&index=semantic&n=2", url_encode(QUERY));
    suite
        .send(get(&uri).key())
        .await
        .status(StatusCode::OK)
        .json(json!({
            "Cursor": "",
            "Items": [
                {
                    "ItemId": "z-near",
                    "IsHidden": false,
                    "Categories": ["near"],
                    "Timestamp": "0001-01-01T00:00:00Z",
                    "Labels": {"rank": "near"},
                    "Comment": "Most similar",
                },
                {
                    "ItemId": "a-far",
                    "IsHidden": false,
                    "Categories": ["far"],
                    "Timestamp": "0001-01-01T00:00:00Z",
                    "Labels": {"rank": "far"},
                    "Comment": "First alphabetically",
                },
            ],
        }));

    let recorded = service.recorded.lock().expect("the record").clone();
    assert_eq!(recorded.len(), 1, "one embedding request");
    let request = &recorded[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/embeddings");
    assert_eq!(request.authorization, "Bearer embedding-token");
    assert_eq!(request.body["input"], json!(QUERY));
    assert_eq!(request.body["model"], json!("test-embedding-model"));
    assert_eq!(request.body["dimensions"], json!(embedding.len()));
}

/// Percent-encodes a query value, so that the query text reaches the handler
/// as Go's `QueryParams` sends it.
fn url_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}
