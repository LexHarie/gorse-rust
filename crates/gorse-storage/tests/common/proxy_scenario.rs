// The proxy wire scenario (WP-2-12). It runs the steps that
// `contracts/tools/proxygolden` runs, in the same order, through the Rust
// proxy clients, and writes one transcript line per step. The transcript that
// Go's client and server produce is the golden file
// `tests/data/proxy_transcript.txt`, so each line here must match Go's line.
//
// tests/proxy_wire.rs (this crate) and conformance/src/bin/proxy_peer.rs include
// this file, so the scenario is the same wherever it runs.

use std::fmt::Display;

use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, HOUR, Time};
use gorse_storage::cache::proxy::ProxyClient as CacheClient;
use gorse_storage::cache::{self, ReturnValue, Score, ScoreCondition, ScorePatch, TimeSeriesPoint};
use gorse_storage::data::GetOptions;
use gorse_storage::data::proxy::ProxyClient as DataClient;
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, ItemPatch, Labels, User, UserPatch};
use gorse_storage::vectors::proxy::ProxyClient as VectorsClient;
use gorse_storage::vectors::{CollectionInfo, Distance, ScoredVector, Vector, VectorConfig};
use indexmap::IndexMap;
use serde_json::{Value, json};
use tonic::transport::Channel;

/// The data store, through the proxy client on `channel`.
pub async fn data_section(channel: Channel) -> String {
    use gorse_storage::data::Database;

    let client = DataClient::new(channel);
    let ctx = Context::background();
    let t1 = gotime::unix(1704164645, 600_000_000).utc();
    let t2 = gotime::unix(1704164700, 0).utc();
    let t0 = gotime::unix(1704000000, 0).utc();
    let mut out = String::new();

    step(&mut out, "d.ping", client.ping().await, unit);
    step(&mut out, "d.init", client.init().await, unit);
    step(&mut out, "d.purge", client.purge().await, unit);

    let items = [
        Item {
            item_id: "i1".into(),
            categories: Some(vec!["a".into(), "b".into()]),
            timestamp: t1.clone(),
            labels: Labels::Array(vec![Labels::String("x".into()), Labels::String("y".into())]),
            comment: "first".into(),
            ..Default::default()
        },
        Item {
            item_id: "i2".into(),
            is_hidden: true,
            categories: Some(vec![]),
            timestamp: t2.clone(),
            labels: Labels::Object(IndexMap::from([
                ("k".to_owned(), Labels::String("v".into())),
                ("n".to_owned(), Labels::Float64(2.5)),
            ])),
            comment: "second".into(),
            ..Default::default()
        },
    ];
    step(
        &mut out,
        "d.batch_insert_items",
        client.batch_insert_items(&ctx, &items).await,
        unit,
    );
    step(
        &mut out,
        "d.get_item_i1",
        client.get_item(&ctx, "i1").await,
        |item| item_json(&item),
    );
    step(
        &mut out,
        "d.get_item_missing",
        client.get_item(&ctx, "missing").await,
        |item| item_json(&item),
    );
    let batch = client
        .batch_get_items(
            &ctx,
            &["i1".into(), "i2".into(), "missing".into()],
            &GetOptions {
                skip_hidden: true,
                ..Default::default()
            },
        )
        .await;
    step(&mut out, "d.batch_get_items", batch, items_json);
    step(
        &mut out,
        "d.count_items",
        client.count_items(&ctx).await,
        Value::from,
    );
    step(
        &mut out,
        "d.get_items",
        client.get_items(&ctx, "", 10, None).await,
        |(cursor, items)| json!({"cursor": cursor, "items": items_json(items)}),
    );
    let patch = ItemPatch {
        is_hidden: Some(true),
        categories: Some(vec!["z".into()]),
        comment: Some("changed".into()),
        ..Default::default()
    };
    step(
        &mut out,
        "d.modify_item_i1",
        client.modify_item(&ctx, "i1", &patch).await,
        unit,
    );
    step(
        &mut out,
        "d.get_item_i1_modified",
        client.get_item(&ctx, "i1").await,
        |item| item_json(&item),
    );
    let patch = ItemPatch {
        comment: Some("x".into()),
        ..Default::default()
    };
    step(
        &mut out,
        "d.modify_item_missing",
        client.modify_item(&ctx, "missing", &patch).await,
        unit,
    );

    let users = [
        User {
            user_id: "u1".into(),
            labels: Labels::Object(IndexMap::from([(
                "tier".to_owned(),
                Labels::String("gold".into()),
            )])),
            comment: "vip".into(),
            ..Default::default()
        },
        User {
            user_id: "u2".into(),
            ..Default::default()
        },
    ];
    step(
        &mut out,
        "d.batch_insert_users",
        client.batch_insert_users(&ctx, &users).await,
        unit,
    );
    step(
        &mut out,
        "d.get_user_u1",
        client.get_user(&ctx, "u1").await,
        |user| user_json(&user),
    );
    step(
        &mut out,
        "d.get_user_missing",
        client.get_user(&ctx, "missing").await,
        |user| user_json(&user),
    );
    step(
        &mut out,
        "d.count_users",
        client.count_users(&ctx).await,
        Value::from,
    );
    step(
        &mut out,
        "d.get_users",
        client.get_users(&ctx, "", 10).await,
        |(cursor, users)| json!({"cursor": cursor, "users": users_json(users)}),
    );
    let patch = UserPatch {
        comment: Some("changed".into()),
        ..Default::default()
    };
    step(
        &mut out,
        "d.modify_user_u1",
        client.modify_user(&ctx, "u1", &patch).await,
        unit,
    );

    let feedback = [
        Feedback {
            feedback_key: FeedbackKey {
                feedback_type: "like".into(),
                user_id: "u1".into(),
                item_id: "i1".into(),
            },
            value: 1.5,
            timestamp: t1.clone(),
            comment: "f1".into(),
            ..Default::default()
        },
        Feedback {
            feedback_key: FeedbackKey {
                feedback_type: "read".into(),
                user_id: "u1".into(),
                item_id: "i2".into(),
            },
            value: 0.5,
            timestamp: t2.clone(),
            labels: Labels::Array(vec![Labels::String("seen".into())]),
            ..Default::default()
        },
        Feedback {
            feedback_key: FeedbackKey {
                feedback_type: "like".into(),
                user_id: "u2".into(),
                item_id: "i1".into(),
            },
            value: 2.25,
            timestamp: t0.clone(),
            ..Default::default()
        },
    ];
    step(
        &mut out,
        "d.batch_insert_feedback",
        client
            .batch_insert_feedback(&ctx, &feedback, true, true, false)
            .await,
        unit,
    );
    step(
        &mut out,
        "d.get_item_feedback_i1",
        client.get_item_feedback(&ctx, "i1", &[]).await,
        feedback_list_json,
    );
    step(
        &mut out,
        "d.get_user_feedback_u1",
        client.get_user_feedback(&ctx, "u1", None, &[]).await,
        feedback_list_json,
    );
    step(
        &mut out,
        "d.get_user_item_feedback",
        client.get_user_item_feedback(&ctx, "u1", "i1", &[]).await,
        feedback_list_json,
    );
    step(
        &mut out,
        "d.count_feedback",
        client.count_feedback(&ctx).await,
        Value::from,
    );
    step(
        &mut out,
        "d.get_feedback",
        client.get_feedback(&ctx, "", 10, None, None, &[]).await,
        |(cursor, feedback)| json!({"cursor": cursor, "feedback": feedback_list_json(feedback)}),
    );

    let streamed = collect_stream(client.get_user_stream(&ctx, 1)).await;
    step(
        &mut out,
        "d.get_user_stream",
        streamed,
        batches_json(users_json),
    );
    let streamed = collect_stream(client.get_item_stream(&ctx, 1, None)).await;
    step(
        &mut out,
        "d.get_item_stream",
        streamed,
        batches_json(items_json),
    );
    let streamed = collect_stream(client.get_feedback_stream(&ctx, 2, &[])).await;
    step(
        &mut out,
        "d.get_feedback_stream",
        streamed,
        batches_json(feedback_list_json),
    );

    step(
        &mut out,
        "d.delete_user_item_feedback",
        client
            .delete_user_item_feedback(&ctx, "u2", "i1", &[])
            .await,
        Value::from,
    );
    step(
        &mut out,
        "d.delete_item_i2",
        client.delete_item(&ctx, "i2").await,
        unit,
    );
    step(
        &mut out,
        "d.get_item_i2_deleted",
        client.get_item(&ctx, "i2").await,
        |item| item_json(&item),
    );
    step(
        &mut out,
        "d.delete_user_u2",
        client.delete_user(&ctx, "u2").await,
        unit,
    );
    step(
        &mut out,
        "d.count_feedback_after_delete",
        client.count_feedback(&ctx).await,
        Value::from,
    );
    out
}

/// The cache store, through the proxy client on `channel`.
pub async fn cache_section(channel: Channel) -> String {
    use gorse_storage::cache::Database;

    let client = CacheClient::new(channel);
    let ctx = Context::background();
    let t1 = gotime::unix(1704164645, 600_000_000).utc();
    let t2 = gotime::unix(1704164700, 0).utc();
    let t3 = gotime::unix(1704168000, 0).utc();
    let mut out = String::new();

    step(&mut out, "c.ping", client.ping().await, unit);
    step(&mut out, "c.init", client.init().await, unit);
    step(&mut out, "c.purge", client.purge().await, unit);
    let scan = client.scan(&mut |_: &str| Ok(())).await;
    step(&mut out, "c.scan", scan, unit);

    let values = [
        cache::string("a", "1"),
        cache::string("b", "x"),
        cache::integer("n", 7),
        cache::time("t", t1.clone()),
    ];
    step(&mut out, "c.set", client.set(&ctx, &values).await, unit);
    let value = client.get(&ctx, "a").await;
    step(&mut out, "c.get_a", return_json(&value), |v| v);
    let value = client.get(&ctx, "missing").await;
    step(&mut out, "c.get_missing", return_json(&value), |v| v);
    let value = client.get(&ctx, "n").await;
    step(&mut out, "c.get_n", value.integer(), Value::from);
    let value = client.get(&ctx, "t").await;
    step(&mut out, "c.get_t", value.time(), |t| time_json(&t));
    step(&mut out, "c.delete_b", client.delete(&ctx, "b").await, unit);
    let value = client.get(&ctx, "b").await;
    step(&mut out, "c.get_b_deleted", return_json(&value), |v| v);

    let documents = [
        Score {
            id: "x".into(),
            score: 1.5,
            is_hidden: false,
            categories: vec!["g".into()],
            timestamp: t1.clone(),
        },
        Score {
            id: "y".into(),
            score: 2.25,
            is_hidden: false,
            categories: vec![],
            timestamp: t2.clone(),
        },
        Score {
            id: "z".into(),
            score: 0.5,
            is_hidden: true,
            categories: vec!["g".into()],
            timestamp: t2.clone(),
        },
    ];
    step(
        &mut out,
        "c.add_scores",
        client.add_scores(&ctx, "rec", "pop", &documents).await,
        unit,
    );
    step(
        &mut out,
        "c.search_scores",
        client.search_scores(&ctx, "rec", "pop", &[], 0, 10).await,
        scores_json,
    );
    let patch = ScorePatch {
        score: Some(3.75),
        ..Default::default()
    };
    step(
        &mut out,
        "c.update_scores",
        client
            .update_scores(&ctx, &["rec".into()], Some("pop"), "x", &patch)
            .await,
        unit,
    );
    step(
        &mut out,
        "c.search_scores_updated",
        client.search_scores(&ctx, "rec", "pop", &[], 0, 10).await,
        scores_json,
    );
    let condition = ScoreCondition {
        id: Some("z".into()),
        ..Default::default()
    };
    step(
        &mut out,
        "c.delete_scores",
        client
            .delete_scores(&ctx, &["rec".into()], &condition)
            .await,
        unit,
    );

    let mut scanned = Vec::new();
    let scan = client
        .scan_scores(
            &ctx,
            &mut |collection: &str, id: &str, subset: &str, timestamp: Time| {
                scanned.push(json!({
                    "collection": collection,
                    "id": id,
                    "subset": subset,
                    "timestamp": time_json(&timestamp),
                }));
                Ok(())
            },
        )
        .await;
    step(
        &mut out,
        "c.scan_scores",
        scan.map(|()| scanned),
        Value::from,
    );

    let points = [
        TimeSeriesPoint {
            name: "ts".into(),
            timestamp: t1.clone(),
            value: 1.5,
        },
        TimeSeriesPoint {
            name: "ts".into(),
            timestamp: t2.clone(),
            value: 2.25,
        },
        TimeSeriesPoint {
            name: "ts".into(),
            timestamp: t3.clone(),
            value: 4.5,
        },
    ];
    step(
        &mut out,
        "c.add_time_series",
        client.add_time_series_points(&ctx, &points).await,
        unit,
    );
    step(
        &mut out,
        "c.get_time_series",
        client
            .get_time_series_points(&ctx, "ts", t1.clone(), t3.clone(), HOUR)
            .await,
        time_series_json,
    );
    out
}

/// The vector store, through the proxy client on `channel`.
pub async fn vectors_section(channel: Channel) -> String {
    use gorse_storage::vectors::Database;

    let client = VectorsClient::new(channel);
    let ctx = Context::background();
    let t1 = gotime::unix(1704164645, 600_000_000).utc();
    let t2 = gotime::unix(1704164700, 0).utc();
    let mut out = String::new();

    step(&mut out, "v.init", client.init().await, unit);
    step(
        &mut out,
        "v.add_collection",
        client
            .add_collection(&ctx, "docs", 2, Distance::Dot, VectorConfig::default())
            .await,
        unit,
    );
    step(
        &mut out,
        "v.list_collections",
        client.list_collections(&ctx).await,
        Value::from,
    );
    step(
        &mut out,
        "v.describe_docs",
        client.describe_collection(&ctx, "docs").await,
        |info| collection_json(&info),
    );
    step(
        &mut out,
        "v.describe_missing",
        client.describe_collection(&ctx, "missing").await,
        |info| collection_json(&info),
    );

    let vectors = [
        Vector {
            id: "a".into(),
            values: vec![1.5, 0.5],
            categories: vec!["c".into()],
            timestamp: t1.clone(),
            ..Default::default()
        },
        Vector {
            id: "b".into(),
            values: vec![0.5, 2.5],
            timestamp: t2.clone(),
            ..Default::default()
        },
        Vector {
            id: "c".into(),
            values: vec![3.5, 1.5],
            is_hidden: true,
            timestamp: t2.clone(),
            ..Default::default()
        },
    ];
    step(
        &mut out,
        "v.add_vectors",
        client.add_vectors(&ctx, "docs", &vectors).await,
        unit,
    );
    step(
        &mut out,
        "v.count_vectors",
        client.count_vectors(&ctx, "docs").await,
        Value::from,
    );
    step(
        &mut out,
        "v.get_vectors",
        client
            .get_vectors(&ctx, "docs", &["a", "c", "missing"])
            .await,
        vectors_json,
    );
    let query = Vector {
        values: vec![1.5, 0.5],
        ..Default::default()
    };
    step(
        &mut out,
        "v.query_vectors",
        client.query_vectors(&ctx, "docs", &query, &[], 2).await,
        scored_json,
    );
    step(
        &mut out,
        "v.delete_vectors",
        client.delete_vectors(&ctx, "docs", &t2).await,
        unit,
    );
    step(
        &mut out,
        "v.count_vectors_after_delete",
        client.count_vectors(&ctx, "docs").await,
        Value::from,
    );
    step(
        &mut out,
        "v.optimize",
        client.optimize(&ctx, "docs").await,
        unit,
    );
    step(
        &mut out,
        "v.delete_collection",
        client.delete_collection(&ctx, "docs").await,
        unit,
    );
    step(
        &mut out,
        "v.list_collections_after_delete",
        client.list_collections(&ctx).await,
        Value::from,
    );
    step(&mut out, "v.close", client.close().await, unit);
    out
}

/// Appends one line for a step: its name, then `{"ok":value}` or
/// `{"err":text}`, with the text the error shows.
fn step<T, E: Display>(
    out: &mut String,
    name: &str,
    result: Result<T, E>,
    render: impl FnOnce(T) -> Value,
) {
    let body = match result {
        Ok(value) => json!({ "ok": render(value) }),
        Err(err) => json!({ "err": err.to_string() }),
    };
    out.push_str(name);
    out.push('\t');
    canonical(&body, out);
    out.push('\n');
}

fn unit((): ()) -> Value {
    Value::Null
}

/// Collects a stream of batches, and the error it ends with, if any.
async fn collect_stream<T, E: Display>(
    mut stream: futures_core::stream::BoxStream<'_, Result<T, E>>,
) -> Result<Vec<T>, E> {
    use futures_util::StreamExt;

    let mut batches = Vec::new();
    while let Some(batch) = stream.next().await {
        batches.push(batch?);
    }
    Ok(batches)
}

fn batches_json<T>(render: fn(Vec<T>) -> Value) -> impl FnOnce(Vec<Vec<T>>) -> Value {
    move |batches| Value::Array(batches.into_iter().map(render).collect())
}

fn time_json(time: &Time) -> Value {
    json!({"unix": time.unix(), "nanos": time.nanosecond()})
}

fn item_json(item: &Item) -> Value {
    json!({
        "ItemId": item.item_id,
        "IsHidden": item.is_hidden,
        "Categories": item.categories,
        "Timestamp": time_json(&item.timestamp),
        "Labels": item.labels,
        "Comment": item.comment,
    })
}

fn items_json(items: Vec<Item>) -> Value {
    Value::Array(items.iter().map(item_json).collect())
}

fn user_json(user: &User) -> Value {
    json!({
        "UserId": user.user_id,
        "Labels": user.labels,
        "Comment": user.comment,
    })
}

fn users_json(users: Vec<User>) -> Value {
    Value::Array(users.iter().map(user_json).collect())
}

fn feedback_json(feedback: &Feedback) -> Value {
    json!({
        "FeedbackType": feedback.feedback_key.feedback_type,
        "UserId": feedback.feedback_key.user_id,
        "ItemId": feedback.feedback_key.item_id,
        "Value": feedback.value,
        "Timestamp": time_json(&feedback.timestamp),
        "Updated": time_json(&feedback.updated),
        "Labels": feedback.labels,
        "Comment": feedback.comment,
    })
}

fn feedback_list_json(feedback: Vec<Feedback>) -> Value {
    Value::Array(feedback.iter().map(feedback_json).collect())
}

fn score_json(score: &Score) -> Value {
    json!({
        "Id": score.id,
        "Score": score.score,
        "IsHidden": score.is_hidden,
        "Categories": score.categories,
        "Timestamp": time_json(&score.timestamp),
    })
}

fn scores_json(scores: Vec<Score>) -> Value {
    Value::Array(scores.iter().map(score_json).collect())
}

fn time_series_json(points: Vec<TimeSeriesPoint>) -> Value {
    Value::Array(
        points
            .iter()
            .map(|point| {
                json!({
                    "Name": point.name,
                    "Timestamp": time_json(&point.timestamp),
                    "Value": point.value,
                })
            })
            .collect(),
    )
}

/// A cache value as `{"value":…,"exists":…}`, or the error it holds.
fn return_json(value: &ReturnValue) -> Result<Value, cache::Error> {
    Ok(json!({"value": value.string()?, "exists": value.exists()}))
}

fn vector_json(vector: &Vector) -> Value {
    json!({
        "Id": vector.id,
        "Values": vector.values,
        "IsHidden": vector.is_hidden,
        "Categories": vector.categories,
        "Timestamp": time_json(&vector.timestamp),
    })
}

fn vectors_json(vectors: Vec<Vector>) -> Value {
    Value::Array(vectors.iter().map(vector_json).collect())
}

fn scored_json(results: Vec<ScoredVector>) -> Value {
    Value::Array(
        results
            .iter()
            .map(|result| json!({"Id": result.vector.id, "Score": result.score}))
            .collect(),
    )
}

fn distance_name(distance: Distance) -> &'static str {
    match distance {
        Distance::Cosine => "cosine",
        Distance::Euclidean => "euclidean",
        Distance::Dot => "dot",
    }
}

fn collection_json(info: &CollectionInfo) -> Value {
    json!({
        "Name": info.name,
        "Dimension": info.dimension,
        "Distance": distance_name(info.distance),
        "Type": info.config.quantization.as_str(),
        "Bits": info.config.bits,
    })
}

/// Writes `value` as the transcript writes JSON: compact, with the keys of
/// each object sorted, and a number with no fraction written as an integer,
/// as Go's `encoding/json` writes a float64 such as 2.
fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&number_text(number)),
        Value::String(text) => out.push_str(&json_string(text)),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
            keys.sort_unstable();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&json_string(key));
                out.push(':');
                canonical(&map[*key], out);
            }
            out.push('}');
        }
    }
}

fn number_text(number: &serde_json::Number) -> String {
    match number.as_f64() {
        Some(float) if number.is_f64() && float.fract() == 0.0 && float.abs() < 1e15 => {
            format!("{}", float as i64)
        }
        _ => number.to_string(),
    }
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}
