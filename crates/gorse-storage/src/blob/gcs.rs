// Ports: storage/blob/gcs.go @ add21b5, storage/blob/gcs_test.go @ add21b5

//! A Google Cloud Storage bucket: `blob.uri` = `gs://bucket/prefix`.

use std::sync::Arc;

use gorse_config::config::schema::GCSConfig;
use object_store::StaticCredentialProvider;
use object_store::gcp::{GcpCredential, GoogleCloudStorageBuilder};

use super::BlobError;
use super::cloud::CloudStore;

/// Opens `bucket` under `prefix` (Go's `NewGCS`). The environment variable
/// `GCS_EMULATOR_ENDPOINT`, when set, points the store at an emulator without
/// authentication, as in Go.
pub fn new(cfg: &GCSConfig, bucket: &str, prefix: &str) -> Result<CloudStore, BlobError> {
    let emulator = std::env::var("GCS_EMULATOR_ENDPOINT").ok();
    build(
        cfg,
        bucket,
        prefix,
        emulator.as_deref().filter(|endpoint| !endpoint.is_empty()),
    )
}

/// Builds the store. `emulator` is the endpoint from `GCS_EMULATOR_ENDPOINT`;
/// the tests pass a local listener here instead of setting the variable.
fn build(
    cfg: &GCSConfig,
    bucket: &str,
    prefix: &str,
    emulator: Option<&str>,
) -> Result<CloudStore, BlobError> {
    let mut builder = GoogleCloudStorageBuilder::new().with_bucket_name(bucket);
    if let Some(endpoint) = emulator {
        // Go's SDK takes the JSON API root, such as "http://host:5050/storage/v1/";
        // object_store wants the server root.
        let base_url = endpoint
            .trim_end_matches('/')
            .trim_end_matches("/storage/v1");
        // The emulator takes no credentials. A static empty token stops
        // object_store from asking the metadata server for one.
        let credential = GcpCredential {
            bearer: String::new(),
        };
        builder = builder
            .with_base_url(base_url)
            .with_skip_signature(true)
            .with_credentials(Arc::new(StaticCredentialProvider::new(credential)));
    }
    if !cfg.credentials_file.is_empty() {
        builder = builder.with_service_account_path(&cfg.credentials_file);
    }
    Ok(CloudStore::new(Arc::new(builder.build()?), prefix, "GCS"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::Store;
    use crate::blob::cloud::required_env;
    use gorse_common::gocontext::Context;
    use gorse_storage_core::errors::Error as StorageError;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    /// Go writes through the emulator here. object_store writes through the GCS
    /// XML API, which fake-gcs-server does not serve (v1.54.0 and v1.56.1 both
    /// answer 400 "invalid uploadType"), and it answers the XML list and read
    /// with 404 (O-62). The emulator cannot run this sequence, so the in-process
    /// round trip below covers it instead.
    #[ignore = "fake-gcs-server does not serve the XML write, list or read that object_store uses (O-31, O-62)"]
    #[tokio::test]
    async fn test_gcs() {
        let ctx = Context::background();
        // Go starts an in-process fake-gcs-server here; the Rust test needs an
        // emulator already running at GCS_EMULATOR_ENDPOINT, with the bucket
        // `gorse-test` created before the test.
        required_env("GCS_EMULATOR_ENDPOINT");

        // create client
        let client = new(&GCSConfig::default(), "gorse-test", "blob").unwrap();

        // create file
        let (mut w, done) = client.create(&ctx, "test.txt").await.unwrap();
        w.write(&ctx, b"hello").await.unwrap();
        w.close(&ctx).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(60), done)
                .await
                .unwrap()
                .is_err()
        );

        // list files
        let names = client.list(&ctx).await.unwrap();
        assert_eq!(names, vec!["test.txt".to_string()]);

        // read file
        let mut r = client.open(&ctx, "test.txt").await.unwrap();
        let mut data = Vec::new();
        r.read_to_end(&mut data).await.unwrap();
        assert_eq!(data, b"hello");
        drop(r);

        // remove file
        client.remove(&ctx, "test.txt").await.unwrap();

        // list files again
        let names = client.list(&ctx).await.unwrap();
        assert!(names.is_empty());
    }

    /// Not in Go. The object is seeded through the emulator's JSON upload API,
    /// which fake-gcs-server does serve, and then listed, read and removed
    /// through object_store. Against fake-gcs-server 1.50 to 1.56 this passes
    /// only with `-external-url` set (O-62).
    #[tokio::test]
    async fn test_gcs_read_list_remove() {
        let ctx = Context::background();
        let endpoint = required_env("GCS_EMULATOR_ENDPOINT");
        let base_url = endpoint
            .trim_end_matches('/')
            .trim_end_matches("/storage/v1");
        reqwest::Client::new()
            .post(format!(
                "{base_url}/upload/storage/v1/b/gorse-test/o?uploadType=media&name=blob%2Fseeded.txt"
            ))
            .body("hello")
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();

        let client = new(&GCSConfig::default(), "gorse-test", "blob").unwrap();

        let names = client.list(&ctx).await.unwrap();
        assert!(names.contains(&"seeded.txt".to_string()));

        let mut r = client.open(&ctx, "seeded.txt").await.unwrap();
        let mut data = Vec::new();
        r.read_to_end(&mut data).await.unwrap();
        assert_eq!(data, b"hello");
        drop(r);

        client.remove(&ctx, "seeded.txt").await.unwrap();
        let names = client.list(&ctx).await.unwrap();
        assert!(!names.contains(&"seeded.txt".to_string()));
    }

    /// Not in Go's tests. The sequence of `test_gcs` run against the XML
    /// listener: write, list, read, remove, list. The requests the store sends
    /// are checked one by one.
    ///
    /// The probe that recorded Go's client (proxied to fake-gcs-server, O-62)
    /// sent the same five calls in the same order: a JSON multipart upload, a
    /// JSON list with `prefix=blob`, a path-style read of `/gorse-test/blob%2Ftest.txt`,
    /// a JSON delete, and a JSON list. object_store uses the XML API for all
    /// four, so only the read is the same request: method and decoded path agree.
    #[tokio::test]
    async fn test_gcs_xml_round_trip() {
        let ctx = Context::background();
        let (endpoint, fake) = serve_xml().await;
        let client = build(&GCSConfig::default(), "gorse-test", "blob", Some(&endpoint)).unwrap();

        let (mut w, done) = client.create(&ctx, "test.txt").await.unwrap();
        w.write(&ctx, b"hello").await.unwrap();
        w.close(&ctx).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(10), done)
                .await
                .unwrap()
                .is_err()
        );

        let names = client.list(&ctx).await.unwrap();
        assert_eq!(names, vec!["test.txt".to_string()]);

        let mut r = client.open(&ctx, "test.txt").await.unwrap();
        let mut data = Vec::new();
        r.read_to_end(&mut data).await.unwrap();
        assert_eq!(data, b"hello");
        drop(r);

        client.remove(&ctx, "test.txt").await.unwrap();
        assert!(client.list(&ctx).await.unwrap().is_empty());

        let sent: Vec<String> = std::mem::take(&mut fake.lock().unwrap().requests)
            .iter()
            .map(Recorded::shape)
            .collect();
        assert_eq!(
            sent,
            vec![
                "PUT /gorse-test/blob/test.txt",
                "GET /gorse-test?list-type=2&prefix=blob/",
                "GET /gorse-test/blob/test.txt",
                "DELETE /gorse-test/blob/test.txt",
                "GET /gorse-test?list-type=2&prefix=blob/",
            ]
        );
    }

    /// Not in Go's tests. A read of a missing object reports the storage
    /// sentinel NotFound, as the POSIX store does.
    #[tokio::test]
    async fn test_gcs_xml_missing_object_is_not_found() {
        let ctx = Context::background();
        let (endpoint, _fake) = serve_xml().await;
        let client = build(&GCSConfig::default(), "gorse-test", "blob", Some(&endpoint)).unwrap();

        let err = client.open(&ctx, "missing.txt").await.err().unwrap();
        assert_eq!(err.kind(), Some(StorageError::NotFound));
    }

    /// Not in Go's tests. One object write goes up as a single `PUT` of the
    /// object's path, with its length and body.
    #[tokio::test]
    async fn test_gcs_xml_put_sends_one_request() {
        let ctx = Context::background();
        let (endpoint, fake) = serve_xml().await;
        let client = build(&GCSConfig::default(), "gorse-test", "blob", Some(&endpoint)).unwrap();

        let (mut w, done) = client.create(&ctx, "test.txt").await.unwrap();
        w.write(&ctx, b"hello").await.unwrap();
        w.close(&ctx).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(10), done)
                .await
                .unwrap()
                .is_err()
        );

        let requests = std::mem::take(&mut fake.lock().unwrap().requests);
        assert_eq!(requests.len(), 1, "one object write and no multipart calls");
        let put = &requests[0];
        assert_eq!(put.method, "PUT");
        assert_eq!(put.path(), "/gorse-test/blob/test.txt");
        assert_eq!(put.header("content-length"), Some("5"));
        assert_eq!(put.header("content-type"), Some("application/octet-stream"));
        assert_eq!(put.body, b"hello");
    }

    /// Not in Go's tests. Data past one part goes up as a multipart upload: the
    /// initiate call, one `PUT` per part numbered from 1 with the upload id,
    /// then the completion listing every part's number and ETag in order.
    #[tokio::test]
    async fn test_gcs_xml_multipart_sends_parts_then_complete() {
        let ctx = Context::background();
        let (endpoint, fake) = serve_xml().await;
        let client = build(&GCSConfig::default(), "gorse-test", "blob", Some(&endpoint)).unwrap();

        let data: Vec<u8> = (0..12 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        let (mut w, done) = client.create(&ctx, "big").await.unwrap();
        for chunk in data.chunks(1024 * 1024) {
            w.write(&ctx, chunk).await.unwrap();
        }
        w.close(&ctx).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(10), done)
                .await
                .unwrap()
                .is_err()
        );

        let requests = std::mem::take(&mut fake.lock().unwrap().requests);
        let (initiate, rest) = requests.split_first().unwrap();
        assert_eq!(initiate.method, "POST");
        assert_eq!(initiate.path(), "/gorse-test/blob/big");
        assert!(
            initiate.query_value("uploads").is_some(),
            "the upload starts with ?uploads"
        );

        let (complete, parts) = rest.split_last().unwrap();
        assert!(parts.len() >= 2, "12 MiB is more than one part");
        assert_eq!(complete.method, "POST");
        assert_eq!(
            complete.query_value("uploadId").as_deref(),
            Some("upload-id")
        );

        // object_store sends up to four parts at once, so the parts may arrive
        // out of order. Order them by part number before checking.
        let mut numbered: Vec<(usize, &Recorded)> = parts
            .iter()
            .map(|part| {
                let number = part.query_value("partNumber").unwrap().parse().unwrap();
                (number, part)
            })
            .collect();
        numbered.sort_by_key(|(number, _)| *number);
        let mut uploaded = Vec::new();
        for (index, (number, part)) in numbered.iter().enumerate() {
            assert_eq!(part.method, "PUT");
            assert_eq!(part.path(), "/gorse-test/blob/big");
            assert_eq!(*number, index + 1, "parts are numbered 1..n with no gaps");
            assert_eq!(part.query_value("uploadId").as_deref(), Some("upload-id"));
            assert_eq!(
                part.header("content-length"),
                Some(part.body.len().to_string().as_str())
            );
            uploaded.extend_from_slice(&part.body);
        }
        assert!(
            uploaded == data,
            "the parts do not add up to the written data"
        );

        // The completion lists every part by ETag and number, in order. The
        // quotes around each ETag are the ones the reply carried.
        let xml = String::from_utf8(complete.body.clone()).unwrap();
        let listed: String = (1..=parts.len())
            .map(|number| {
                format!(
                    "<Part><ETag>\"etag-{number}\"</ETag><PartNumber>{number}</PartNumber></Part>"
                )
            })
            .collect();
        assert!(
            xml.contains(&listed),
            "the completion lists the parts in order: {xml}"
        );
    }

    /// One request the in-process server received.
    struct Recorded {
        method: String,
        /// The request target as sent: the path, then the query.
        target: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Recorded {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        }

        /// The object path, percent-decoded: `/bucket/name`.
        fn path(&self) -> String {
            let (path, _) = self
                .target
                .split_once('?')
                .unwrap_or((self.target.as_str(), ""));
            decode(path)
        }

        /// The query parameters in order, percent-decoded.
        fn query(&self) -> Vec<(String, String)> {
            let Some((_, query)) = self.target.split_once('?') else {
                return Vec::new();
            };
            query
                .split('&')
                .filter(|pair| !pair.is_empty())
                .map(|pair| {
                    let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                    (decode(key), decode(value))
                })
                .collect()
        }

        fn query_value(&self, key: &str) -> Option<String> {
            self.query()
                .into_iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value)
        }

        /// The method, the decoded path and the decoded query: what the request
        /// asks for, without the encoding.
        fn shape(&self) -> String {
            let query: Vec<String> = self
                .query()
                .into_iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            if query.is_empty() {
                format!("{} {}", self.method, self.path())
            } else {
                format!("{} {}?{}", self.method, self.path(), query.join("&"))
            }
        }
    }

    fn decode(text: &str) -> String {
        percent_encoding::percent_decode_str(text)
            .decode_utf8_lossy()
            .into_owned()
    }

    /// The state of the in-process GCS listener: the objects it holds, keyed
    /// by `bucket/name`, and every request it has answered.
    #[derive(Default)]
    struct Fake {
        objects: BTreeMap<String, Vec<u8>>,
        requests: Vec<Recorded>,
    }

    /// Serves the GCS XML API on a local port, with an in-memory bucket. It
    /// answers each call the way the XML API does for the calls object_store
    /// makes, and records every request.
    async fn serve_xml() -> (String, Arc<Mutex<Fake>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let fake = Arc::new(Mutex::new(Fake::default()));
        let state = Arc::clone(&fake);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve_connection(stream, Arc::clone(&state)));
            }
        });
        (format!("http://{addr}"), fake)
    }

    /// Answers the requests of one keep-alive connection until the client closes it.
    async fn serve_connection(stream: TcpStream, state: Arc<Mutex<Fake>>) {
        let mut reader = BufReader::new(stream);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap() == 0 {
                return;
            }
            let mut words = line.split_whitespace();
            let method = words.next().unwrap().to_string();
            let target = words.next().unwrap().to_string();
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (key, value) = line.split_once(':').unwrap();
                headers.push((key.trim().to_string(), value.trim().to_string()));
            }
            let length = headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .map_or(0, |(_, value)| value.parse::<usize>().unwrap());
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).await.unwrap();
            let request = Recorded {
                method,
                target,
                headers,
                body,
            };
            // The lock ends before the reply is written: a std guard must not be
            // held across an await.
            let answer = {
                let mut fake = state.lock().unwrap();
                let answer = answer(&mut fake, &request);
                fake.requests.push(request);
                answer
            };
            reader.get_mut().write_all(&answer).await.unwrap();
        }
    }

    /// The reply to `request`, as the XML API gives it.
    fn answer(fake: &mut Fake, request: &Recorded) -> Vec<u8> {
        let query = request.query();
        let has = |key: &str| query.iter().any(|(name, _)| name == key);
        let value = |key: &str| {
            query
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        };
        // The bucket, or the bucket and object name, without the leading slash.
        let key = request.path().trim_start_matches('/').to_string();
        match request.method.as_str() {
            "GET" if has("list-type") => {
                let prefix = format!("{key}/{}", value("prefix").unwrap_or_default());
                let mut contents = String::new();
                let mut count = 0;
                for (name, body) in fake
                    .objects
                    .iter()
                    .filter(|(name, _)| name.starts_with(&prefix))
                {
                    let name = &name[key.len() + 1..];
                    contents.push_str(&format!(
                        "<Contents><Key>{name}</Key><LastModified>2026-10-28T00:00:00.000Z</LastModified><ETag>\"etag-object\"</ETag><Size>{}</Size></Contents>",
                        body.len()
                    ));
                    count += 1;
                }
                xml_reply(&format!(
                    "<ListBucketResult><Name>{key}</Name><Prefix>{}</Prefix><KeyCount>{count}</KeyCount><IsTruncated>false</IsTruncated>{contents}</ListBucketResult>",
                    value("prefix").unwrap_or_default()
                ))
            }
            "POST" if has("uploads") => xml_reply(
                "<InitiateMultipartUploadResult><UploadId>upload-id</UploadId></InitiateMultipartUploadResult>",
            ),
            "PUT" if has("partNumber") => {
                let part = value("partNumber").unwrap_or_default();
                reply("200 OK", Some(format!("\"etag-{part}\"")), &[])
            }
            "POST" if has("uploadId") => xml_reply(
                "<CompleteMultipartUploadResult><ETag>\"complete\"</ETag></CompleteMultipartUploadResult>",
            ),
            "PUT" => {
                fake.objects.insert(key, request.body.clone());
                reply("200 OK", Some("\"etag-put\"".to_string()), &[])
            }
            "GET" => match fake.objects.get(&key) {
                Some(body) => reply("200 OK", Some("\"etag-object\"".to_string()), body),
                None => not_found(),
            },
            "DELETE" => match fake.objects.remove(&key) {
                Some(_) => reply("204 No Content", None, &[]),
                None => not_found(),
            },
            _ => not_found(),
        }
    }

    /// A complete HTTP/1.1 reply with its ETag and body. Every reply carries a
    /// `Last-Modified` header, as GCS sends on object reads; object_store fails
    /// an object read without it.
    fn reply(status: &str, etag: Option<String>, body: &[u8]) -> Vec<u8> {
        let etag = etag
            .map(|tag| format!("ETag: {tag}\r\n"))
            .unwrap_or_default();
        let mut out = format!(
            "HTTP/1.1 {status}\r\n{etag}Last-Modified: Wed, 28 Oct 2026 00:00:00 GMT\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn xml_reply(xml: &str) -> Vec<u8> {
        reply("200 OK", None, xml.as_bytes())
    }

    fn not_found() -> Vec<u8> {
        let xml = "<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message></Error>";
        reply("404 Not Found", None, xml.as_bytes())
    }
}
