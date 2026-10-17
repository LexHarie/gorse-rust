// Ports: protocol/*.proto @ add21b5

//! Byte-for-byte compatibility of the generated code with the Go `protocol`
//! package.
//!
//! `tests/data/golden.json` is written by `go run ./contracts/tools/protogolden`
//! from the Go code at the pinned commit. For every message it holds the
//! deterministic encoding of an instance with every field set, and of an empty
//! instance; for every service it lists the method paths and streaming kinds.

use std::collections::BTreeMap;

use prost::Message;
use serde::Deserialize;

#[derive(Deserialize)]
struct Golden {
    messages: BTreeMap<String, String>,
    empty: BTreeMap<String, String>,
    methods: BTreeMap<String, Vec<String>>,
}

fn golden() -> Golden {
    serde_json::from_str(include_str!("data/golden.json")).expect("golden.json is valid")
}

/// Decodes `bytes` as the message named `name` and encodes it again.
fn round_trip(name: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    macro_rules! dispatch {
        ($($ty:ident,)*) => {
            match name {
                $(concat!("protocol.", stringify!($ty)) => Some(
                    gorse_protocol::$ty::decode(bytes)
                        .unwrap_or_else(|err| panic!("decode {name}: {err}"))
                        .encode_to_vec(),
                ),)*
                _ => None,
            }
        };
    }
    dispatch!(
        AddCollectionRequest,
        AddCollectionResponse,
        AddScoresRequest,
        AddScoresResponse,
        AddTimeSeriesPointsRequest,
        AddTimeSeriesPointsResponse,
        AddVectorsRequest,
        AddVectorsResponse,
        BatchGetItemsRequest,
        BatchGetItemsResponse,
        BatchInsertFeedbackRequest,
        BatchInsertFeedbackResponse,
        BatchInsertItemsRequest,
        BatchInsertItemsResponse,
        BatchInsertUsersRequest,
        BatchInsertUsersResponse,
        CountFeedbackRequest,
        CountFeedbackResponse,
        CountItemsRequest,
        CountItemsResponse,
        CountUsersRequest,
        CountUsersResponse,
        CountVectorsRequest,
        CountVectorsResponse,
        DeleteCollectionRequest,
        DeleteCollectionResponse,
        DeleteItemRequest,
        DeleteItemResponse,
        DeleteRequest,
        DeleteResponse,
        DeleteScoresRequest,
        DeleteScoresResponse,
        DeleteUserItemFeedbackRequest,
        DeleteUserItemFeedbackResponse,
        DeleteUserRequest,
        DeleteUserResponse,
        DeleteVectorsRequest,
        DeleteVectorsResponse,
        DescribeCollectionRequest,
        DescribeCollectionResponse,
        DownloadBlobRequest,
        DownloadBlobResponse,
        Feedback,
        FeedbackTypeExpression,
        Fragment,
        GetFeedbackRequest,
        GetFeedbackResponse,
        GetFeedbackStreamRequest,
        GetFeedbackStreamResponse,
        GetItemFeedbackRequest,
        GetItemRequest,
        GetItemResponse,
        GetItemStreamRequest,
        GetItemStreamResponse,
        GetItemsRequest,
        GetItemsResponse,
        GetLatestItemsRequest,
        GetLatestItemsResponse,
        GetOptions,
        GetRequest,
        GetResponse,
        GetTimeSeriesPointsRequest,
        GetTimeSeriesPointsResponse,
        GetUserFeedbackRequest,
        GetUserItemFeedbackRequest,
        GetUserRequest,
        GetUserResponse,
        GetUserStreamRequest,
        GetUserStreamResponse,
        GetUsersRequest,
        GetUsersResponse,
        GetVectorsRequest,
        GetVectorsResponse,
        Item,
        ItemPatch,
        LatentFactor,
        ListBlobsRequest,
        ListBlobsResponse,
        ListCollectionsRequest,
        ListCollectionsResponse,
        Meta,
        ModifyItemRequest,
        ModifyItemResponse,
        ModifyUserRequest,
        ModifyUserResponse,
        NodeInfo,
        PingRequest,
        PingResponse,
        Progress,
        PushProgressRequest,
        PushProgressResponse,
        QueryVectorsRequest,
        QueryVectorsResponse,
        ReconcileRequest,
        ReconcileResponse,
        RemoveBlobRequest,
        RemoveBlobResponse,
        ScanOptions,
        ScanScoresRequest,
        ScanScoresResponse,
        Score,
        ScoreCondition,
        ScorePatch,
        ScoredItem,
        ScoredVector,
        SearchItemsRequest,
        SearchItemsResponse,
        SearchScoresRequest,
        SearchScoresResponse,
        SetRequest,
        SetResponse,
        Tensor,
        TimeSeriesPoint,
        UpdateScoresRequest,
        UpdateScoresResponse,
        UploadBlobRequest,
        UploadBlobResponse,
        User,
        UserPatch,
        Value,
        Vector,
        VectorConfig,
    )
}

#[test]
fn full_messages_round_trip_to_go_bytes() {
    let golden = golden();
    assert_eq!(golden.messages.len(), 122);
    for (name, encoded) in &golden.messages {
        let bytes = hex::decode(encoded).expect("valid hex");
        assert!(!bytes.is_empty() || name.ends_with("Request") || name.ends_with("Response"));
        let rust = round_trip(name, &bytes).unwrap_or_else(|| panic!("no Rust type for {name}"));
        assert_eq!(hex::encode(rust), *encoded, "{name}");
    }
}

#[test]
fn empty_messages_encode_like_go() {
    let golden = golden();
    assert_eq!(golden.empty.len(), golden.messages.len());
    for (name, encoded) in &golden.empty {
        assert_eq!(encoded, "", "Go encodes an empty {name} to no bytes");
        let rust = round_trip(name, &[]).unwrap_or_else(|| panic!("no Rust type for {name}"));
        assert!(rust.is_empty(), "{name}");
    }
}

#[test]
fn optional_fields_keep_presence() {
    // `GetItemResponse.item` is `optional`: the Go server answers a missing item
    // with the field unset, and the Go client relies on telling that apart from
    // an empty item (contracts/grpc.md).
    let unset = gorse_protocol::GetItemResponse { item: None };
    assert!(unset.encode_to_vec().is_empty());
    let empty_item = gorse_protocol::GetItemResponse {
        item: Some(gorse_protocol::Item::default()),
    };
    let bytes = empty_item.encode_to_vec();
    assert_eq!(bytes, [0x0a, 0x00]);
    let decoded = gorse_protocol::GetItemResponse::decode(bytes.as_slice()).expect("decodes");
    assert!(decoded.item.is_some());

    let value = gorse_protocol::GetResponse {
        value: Some(String::new()),
    };
    assert_eq!(value.encode_to_vec(), [0x0a, 0x00]);
}

#[test]
fn services_match_go_methods() {
    let golden = golden();
    let generated = include_str!(concat!(env!("OUT_DIR"), "/protocol.rs"));
    let mut total = 0;
    for (service, methods) in &golden.methods {
        assert!(
            generated.contains(&format!("pub const SERVICE_NAME: &str = \"{service}\";")),
            "{service}"
        );
        for method in methods {
            let (path, kind) = method.split_once(' ').expect("path and kind");
            total += 1;
            // Server routing.
            assert!(
                generated.contains(&format!("\"{path}\" => {{")),
                "server route {path}"
            );
            // Client call: the call kind follows the path constant.
            let at = generated
                .find(&format!("\"{path}\",\n"))
                .or_else(|| generated.find(&format!("\"{path}\");")))
                .unwrap_or_else(|| panic!("client path {path}"));
            let rest = &generated[at..];
            let call = rest.find("self.inner.").expect("client call") + "self.inner.".len();
            let call_kind = &rest[call..rest[call..].find('(').expect("call") + call];
            let expected = match kind {
                "unary" => "unary",
                "server-stream" => "server_streaming",
                "client-stream" => "client_streaming",
                "bidi" => "streaming",
                other => panic!("unknown kind {other}"),
            };
            assert_eq!(call_kind, expected, "{path}");
        }
    }
    assert_eq!(total, 53);
    assert_eq!(
        generated.matches("PathAndQuery::from_static(").count(),
        total
    );
}
