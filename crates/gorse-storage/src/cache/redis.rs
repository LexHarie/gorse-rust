// Ports: storage/cache/redis.go and storage/cache/redis_test.go @ add21b5

//! The Redis cache store, standalone or cluster (Go's `Redis`).
//!
//! `redis://` and `rediss://` open a standalone store. `redis+cluster://` and
//! `rediss+cluster://` open a cluster, and its primaries are scanned one by one
//! (Go's `ForEachMaster`).
//!
//! Score documents are hashes searched with RediSearch (`FT.*`), and time
//! series points are RedisTimeSeries keys (`TS.*`). Both are sent as raw
//! commands, as go-redis sends them, so the server needs those modules: Redis 8
//! or Redis Stack, as `storage/docker-compose.yml` runs.
//!
//! The connection is made on first use, as go-redis's `NewClient` is lazy. A
//! standalone store runs on a `ConnectionManager`, which reconnects by itself.
//! A cluster cannot pipeline commands whose keys span slots, so cluster
//! pipelines are sent one command at a time.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use futures_util::future::BoxFuture;
use gorse_common::gocontext::Context;
use gorse_common::gostrconv;
use gorse_common::gotime::{self, Duration, Time};
use gorse_storage_core::options::{Opt, new_options};
use gorse_storage_core::scheme::{
    REDIS_CLUSTER_PREFIX, REDIS_PREFIX, REDISS_CLUSTER_PREFIX, REDISS_PREFIX,
};
use redis::Cmd;
use redis::Value as RedisValue;
use redis::aio::ConnectionManager;
use redis::cluster_async::ClusterConnection;
use redis::cluster_routing::{RoutingInfo, SingleNodeRoutingInfo};
use tokio::sync::OnceCell;

use super::{
    Creator, Database, Error, Registry, ReturnValue, Score, ScoreCondition, ScorePatch,
    TimeSeriesPoint, Value, new_return_error, new_return_value,
};

/// The COUNT hint of the score scan, and so the size of each pipelined batch of
/// `HGETALL` (Go's `scanScoresPageSize`).
const SCAN_SCORES_PAGE_SIZE: u64 = 1000;

/// The most documents one `FT.SEARCH` returns when no limit is set (Go's
/// `10000`).
const MAX_SEARCH_LIMIT: i64 = 10000;

/// Go's `Redis`: a cache store on a Redis server or a Redis cluster.
pub struct Redis {
    client: Backend,
    /// Go's `RedisClientName`, sent with `CLIENT SETNAME` on connect.
    client_name: String,
    /// The connection, made on first use.
    conn: OnceCell<Conn>,
    /// Go's `TablePrefix`, in front of every key and table name.
    prefix: String,
    /// Go's `maxSearchResults`: the page size of the searches
    /// [`Database::update_scores`] makes. Zero or less means 10000.
    max_search_results: i64,
}

/// How a store reaches Redis.
enum Backend {
    /// One server.
    Standalone(redis::Client),
    /// A cluster, whose primaries are scanned one by one.
    Cluster(Box<redis::cluster::ClusterClient>),
}

/// An open connection to a server or a cluster.
#[derive(Clone)]
enum Conn {
    Standalone(ConnectionManager),
    Cluster(ClusterConnection),
}

/// Where a `SCAN` runs: the server itself, or one primary of a cluster.
#[derive(Clone)]
enum Node {
    Server,
    Primary { host: String, port: u16 },
}

/// The parsed reply of `FT.SEARCH` in RESP2: the total, then the key and the
/// fields of each document.
struct SearchReply {
    total: i64,
    docs: Vec<(String, HashMap<String, String>)>,
}

/// Registers the four Redis schemes with a registry. All of them open through
/// [`open`], which picks a standalone or a cluster store by the scheme, as Go's
/// `Open` does after its `init` registrations.
pub fn register(registry: &mut Registry) {
    registry.register(
        &[
            REDIS_PREFIX,
            REDISS_PREFIX,
            REDIS_CLUSTER_PREFIX,
            REDISS_CLUSTER_PREFIX,
        ],
        create as Creator,
    );
}

/// Go's creator for the Redis schemes, as a [`Creator`]. The open does not
/// connect, so the future is ready when it is first polled.
fn create<'a>(
    path: &'a str,
    table_prefix: &'a str,
    opts: &'a [Opt],
) -> BoxFuture<'a, Result<Box<dyn Database>, Error>> {
    Box::pin(async move {
        let store: Box<dyn Database> = Box::new(open(path, table_prefix, opts)?);
        Ok(store)
    })
}

/// Opens a store on a Redis URL. A `+cluster` scheme opens a cluster, and the
/// others open a standalone server. Nothing is connected until the first call,
/// as go-redis's `NewClient` is lazy too.
pub fn open(path: &str, table_prefix: &str, opts: &[Opt]) -> Result<Redis, Error> {
    if path.starts_with(REDIS_CLUSTER_PREFIX) || path.starts_with(REDISS_CLUSTER_PREFIX) {
        return open_cluster(path, table_prefix, opts);
    }
    let client = redis::Client::open(path).map_err(backend)?;
    Ok(Redis::new(Backend::Standalone(client), table_prefix, opts))
}

/// Opens a cluster store on a `redis+cluster://` or `rediss+cluster://` URL.
/// Go's `ParseClusterURL` reads the base address and every `addr` query
/// parameter, and the base URL's credentials apply to all of them.
pub fn open_cluster(path: &str, table_prefix: &str, opts: &[Opt]) -> Result<Redis, Error> {
    let client = redis::cluster::ClusterClientBuilder::new(cluster_urls(path))
        .build()
        .map_err(backend)?;
    Ok(Redis::new(
        Backend::Cluster(Box::new(client)),
        table_prefix,
        opts,
    ))
}

/// The node URLs of a cluster path, in the `redis://` or `rediss://` form the
/// client reads: the base URL, then each `addr` parameter with the base URL's
/// scheme and credentials.
fn cluster_urls(path: &str) -> Vec<String> {
    let (scheme, rest) = if let Some(rest) = path.strip_prefix(REDISS_CLUSTER_PREFIX) {
        (REDISS_PREFIX, rest)
    } else {
        (
            REDIS_PREFIX,
            path.strip_prefix(REDIS_CLUSTER_PREFIX).unwrap_or(path),
        )
    };
    let (base, query) = rest.split_once('?').unwrap_or((rest, ""));
    let userinfo = base
        .rsplit_once('@')
        .map(|(userinfo, _)| format!("{userinfo}@"))
        .unwrap_or_default();
    let mut urls = vec![format!("{scheme}{base}")];
    for param in query.split('&') {
        if let Some(addr) = param.strip_prefix("addr=") {
            urls.push(format!("{scheme}{userinfo}{addr}"));
        }
    }
    urls
}

impl Redis {
    fn new(client: Backend, table_prefix: &str, opts: &[Opt]) -> Redis {
        let options = new_options(opts);
        Redis {
            client,
            client_name: options.redis_client_name,
            conn: OnceCell::new(),
            prefix: table_prefix.to_owned(),
            max_search_results: options.max_search_results,
        }
    }

    /// Go's `TablePrefix.DocumentTable`: the score documents' index and key
    /// prefix.
    fn document_table(&self) -> String {
        format!("{}documents", self.prefix)
    }

    /// Go's `TablePrefix.PointsTable`.
    fn points_table(&self) -> String {
        format!("{}time_series_points", self.prefix)
    }

    /// Go's `TablePrefix.Key`.
    fn key(&self, name: &str) -> String {
        format!("{}{name}", self.prefix)
    }

    /// Go's `documentKey`.
    fn document_key(&self, collection: &str, subset: &str, value: &str) -> String {
        format!("{}:{collection}:{subset}:{value}", self.document_table())
    }

    /// The connection, made on the first call.
    async fn conn(&self) -> Result<Conn, Error> {
        let conn = self.conn.get_or_try_init(|| self.connect()).await?;
        Ok(conn.clone())
    }

    /// Opens the connection and, when one is configured, names it.
    async fn connect(&self) -> Result<Conn, Error> {
        let mut conn = match &self.client {
            Backend::Standalone(client) => {
                Conn::Standalone(client.get_connection_manager().await.map_err(backend)?)
            }
            Backend::Cluster(client) => {
                Conn::Cluster(client.get_async_connection().await.map_err(backend)?)
            }
        };
        if !self.client_name.is_empty() {
            let mut cmd = redis::cmd("CLIENT");
            cmd.arg("SETNAME").arg(&self.client_name);
            let _: RedisValue = query_conn(&mut conn, &cmd).await?;
        }
        Ok(conn)
    }

    /// Sends one command and reads its reply.
    async fn send<T: redis::FromRedisValue>(&self, cmd: &Cmd) -> Result<T, Error> {
        let mut conn = self.conn().await?;
        query_conn(&mut conn, cmd).await
    }

    /// Go's `Pipeline` with `Exec`: sends the commands and returns their
    /// replies, in order. A cluster sends them one at a time, since a cluster
    /// pipeline cannot span slots.
    async fn exec(&self, cmds: &[Cmd]) -> Result<Vec<RedisValue>, Error> {
        if cmds.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.conn().await?;
        if let Conn::Standalone(manager) = &mut conn {
            let mut pipe = redis::pipe();
            for cmd in cmds {
                pipe.add_command(cmd.clone());
            }
            return pipe.query_async(manager).await.map_err(backend);
        }
        let mut replies = Vec::with_capacity(cmds.len());
        for cmd in cmds {
            replies.push(query_conn(&mut conn, cmd).await?);
        }
        Ok(replies)
    }

    /// The nodes a scan visits: the server, or every primary of a cluster
    /// (Go's `ForEachMaster`). Primaries come from `CLUSTER NODES`.
    async fn nodes(&self) -> Result<Vec<Node>, Error> {
        let mut conn = self.conn().await?;
        let Conn::Cluster(cluster) = &mut conn else {
            return Ok(vec![Node::Server]);
        };
        let mut cmd = redis::cmd("CLUSTER");
        cmd.arg("NODES");
        let routing = RoutingInfo::SingleNode(SingleNodeRoutingInfo::Random);
        let value = cluster.route_command(cmd, routing).await.map_err(backend)?;
        let text: String = parse(value)?;
        Ok(text.lines().filter_map(parse_primary).collect())
    }

    /// One `SCAN` page on a node. Go sends `COUNT` only when it is set, so a
    /// count of zero leaves Redis' default.
    async fn scan_page(
        &self,
        node: &Node,
        cursor: u64,
        pattern: &str,
        count: u64,
    ) -> Result<(u64, Vec<String>), Error> {
        let mut cmd = redis::cmd("SCAN");
        cmd.arg(cursor).arg("MATCH").arg(pattern);
        if count > 0 {
            cmd.arg("COUNT").arg(count);
        }
        let mut conn = self.conn().await?;
        if let (Node::Primary { host, port }, Conn::Cluster(cluster)) = (node, &mut conn) {
            let routing = RoutingInfo::SingleNode(SingleNodeRoutingInfo::ByAddress {
                host: host.clone(),
                port: *port,
            });
            let value = cluster.route_command(cmd, routing).await.map_err(backend)?;
            return parse(value);
        }
        query_conn(&mut conn, &cmd).await
    }

    /// Go's `FTSearchWithArgs` for a score search, sorted by score, highest
    /// first. go-redis sends `LIMIT` only when the offset is set or the limit
    /// is positive, which is the condition below.
    async fn ft_search(&self, query: &str, offset: i64, limit: i64) -> Result<SearchReply, Error> {
        let mut cmd = redis::cmd("FT.SEARCH");
        cmd.arg(self.document_table())
            .arg(query)
            .arg("SORTBY")
            .arg("score")
            .arg("DESC");
        if (offset >= 0 && limit > 0) || (offset > 0 && limit == 0) {
            cmd.arg("LIMIT").arg(offset).arg(limit);
        }
        let reply: RedisValue = self.send(&cmd).await?;
        parse_search(reply)
    }
}

/// Sends a command on an open connection and reads its reply.
async fn query_conn<T: redis::FromRedisValue>(conn: &mut Conn, cmd: &Cmd) -> Result<T, Error> {
    let reply = match conn {
        Conn::Standalone(manager) => cmd.query_async(manager).await,
        Conn::Cluster(cluster) => cmd.query_async(cluster).await,
    };
    reply.map_err(backend)
}

/// Wraps a driver error as a backend failure.
fn backend(err: redis::RedisError) -> Error {
    Error::Backend(Arc::new(err))
}

/// Reads a reply as `T`. A reply of the wrong type is a backend failure.
fn parse<T: redis::FromRedisValue>(value: RedisValue) -> Result<T, Error> {
    redis::from_redis_value(value).map_err(|err| backend(err.into()))
}

/// A reply of an unexpected shape.
fn reply_error(desc: &'static str) -> Error {
    backend(redis::RedisError::from((
        redis::ErrorKind::UnexpectedReturnType,
        desc,
    )))
}

/// A stored value that does not parse, as Go's `strconv` error.
fn stored_number(err: gostrconv::NumError) -> Error {
    Error::Backend(Arc::new(err))
}

/// Go's `ctx.Err()` check: the error once the context is done.
fn check_context(ctx: &Context) -> Result<(), Error> {
    match ctx.err() {
        Some(err) => Err(err.into()),
        None => Ok(()),
    }
}

/// Parses one line of `CLUSTER NODES` and returns the primary it describes.
/// The address is `ip:port@cport`, with an optional `,hostname`.
fn parse_primary(line: &str) -> Option<Node> {
    let mut fields = line.split_whitespace();
    let _id = fields.next()?;
    let addr = fields.next()?;
    let flags = fields.next()?;
    if !flags.split(',').any(|flag| flag == "master") {
        return None;
    }
    let addr = addr.split('@').next()?;
    let (host, port) = addr.rsplit_once(':')?;
    Some(Node::Primary {
        host: host.to_owned(),
        port: port.parse().ok()?,
    })
}

/// Parses an `FT.SEARCH` reply: the total, then a key and a field list per
/// document.
fn parse_search(reply: RedisValue) -> Result<SearchReply, Error> {
    let RedisValue::Array(items) = reply else {
        return Err(reply_error("FT.SEARCH reply is not an array"));
    };
    let Some((total, docs)) = items.split_first() else {
        return Err(reply_error("FT.SEARCH reply is empty"));
    };
    let total: i64 = parse(total.clone())?;
    let mut parsed = Vec::with_capacity(docs.len() / 2);
    for pair in docs.chunks(2) {
        let [key, fields] = pair else {
            return Err(reply_error("FT.SEARCH document has no fields"));
        };
        let key: String = parse(key.clone())?;
        let fields: HashMap<String, String> = parse(fields.clone())?;
        parsed.push((key, fields));
    }
    Ok(SearchReply {
        total,
        docs: parsed,
    })
}

/// A field of a hash read with `HGETALL`. A missing field reads as the empty
/// string, as Go's map does.
fn field<'a>(row: &'a HashMap<String, String>, name: &str) -> &'a str {
    row.get(name).map(String::as_str).unwrap_or("")
}

/// Go's `scoreFromFields`: the document a search reply describes.
fn score_from_fields(fields: &HashMap<String, String>) -> Result<Score, Error> {
    let (score, err) = gostrconv::parse_float(field(fields, "score"), 64);
    if let Some(err) = err {
        return Err(stored_number(err));
    }
    let (is_hidden, err) = gostrconv::parse_int(field(fields, "is_hidden"), 10, 64);
    if let Some(err) = err {
        return Err(stored_number(err));
    }
    let categories = decode_categories(field(fields, "categories"))?;
    let (timestamp, err) = gostrconv::parse_int(field(fields, "timestamp"), 10, 64);
    if let Some(err) = err {
        return Err(stored_number(err));
    }
    Ok(Score {
        id: field(fields, "id").to_owned(),
        score,
        is_hidden: is_hidden != 0,
        categories,
        timestamp: gotime::unix_micro(timestamp).in_location(&gotime::utc()),
    })
}

/// Go's `escape`: backslash-escapes the characters RediSearch reads as syntax.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '-' | ':' | '.' | '/' | '+') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The `"1"` or `"0"` go-redis writes for a bool.
fn bool_str(b: bool) -> &'static str {
    if b { "1" } else { "0" }
}

/// Go's `encodeCategory`: base64 without padding, of the category with a `_`
/// in front. The `_` keeps the empty category from encoding to nothing.
fn encode_category(category: &str) -> String {
    STANDARD_NO_PAD.encode(format!("_{category}"))
}

/// Go's `decodeCategory`. Go slices off the first byte without checking it is
/// there, so a token that decodes to nothing is an error here, not a panic.
fn decode_category(s: &str) -> Result<String, Error> {
    let bytes = STANDARD_NO_PAD
        .decode(s)
        .map_err(|err| Error::Backend(Arc::new(err)))?;
    let category = bytes.get(1..).ok_or_else(|| {
        Error::Backend(Arc::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "encoded category is empty",
        )))
    })?;
    String::from_utf8(category.to_vec()).map_err(|err| Error::Backend(Arc::new(err)))
}

/// Go's `encodeCategories`: the encoded categories, joined with `;`.
fn encode_categories(categories: &[String]) -> String {
    categories
        .iter()
        .map(|category| encode_category(category))
        .collect::<Vec<_>>()
        .join(";")
}

/// Go's `decodeCategories`: an empty field is no categories.
fn decode_categories(s: &str) -> Result<Vec<String>, Error> {
    if s.is_empty() {
        return Ok(Vec::new());
    }
    s.split(';').map(decode_category).collect()
}

/// Whether a `TS.RANGE` error says the series does not exist yet. Go checks
/// the two prefixes the server uses; a substring check covers both.
fn is_missing_series(err: &Error) -> bool {
    let message = err.to_string();
    message.contains("TSDB: the key does not exist") || message.contains("key does not exist")
}

#[async_trait]
impl Database for Redis {
    async fn close(&self) -> Result<(), Error> {
        // The connection is dropped with the store. Go's `client.Close` is
        // the same drop.
        Ok(())
    }

    async fn ping(&self) -> Result<(), Error> {
        let _: RedisValue = self.send(&redis::cmd("PING")).await?;
        Ok(())
    }

    /// Creates the score index unless it exists (Go's `Init`).
    async fn init(&self) -> Result<(), Error> {
        let indices: Vec<String> = self.send(&redis::cmd("FT._LIST")).await?;
        let table = self.document_table();
        if indices.contains(&table) {
            return Ok(());
        }
        let mut cmd = redis::cmd("FT.CREATE");
        cmd.arg(&table)
            .arg("ON")
            .arg("HASH")
            .arg("PREFIX")
            .arg(1)
            .arg(format!("{table}:"))
            .arg("SCHEMA")
            .arg("collection")
            .arg("TAG")
            .arg("subset")
            .arg("TAG")
            .arg("id")
            .arg("TAG")
            .arg("score")
            .arg("NUMERIC")
            .arg("is_hidden")
            .arg("NUMERIC")
            .arg("categories")
            .arg("TAG")
            .arg("SEPARATOR")
            .arg(";")
            .arg("timestamp")
            .arg("NUMERIC");
        match self.send::<RedisValue>(&cmd).await {
            Ok(_) => Ok(()),
            Err(err) if err.to_string().contains("Index already exists") => Ok(()),
            Err(err) => Err(err),
        }
    }

    async fn scan(
        &self,
        work: &mut (dyn for<'a> FnMut(&'a str) -> Result<(), Error> + Send),
    ) -> Result<(), Error> {
        let pattern = format!("{}*", self.prefix);
        for node in self.nodes().await? {
            let mut cursor = 0;
            loop {
                let (next, keys) = self.scan_page(&node, cursor, &pattern, 0).await?;
                for key in &keys {
                    work(&key[self.prefix.len()..])?;
                }
                if next == 0 {
                    break;
                }
                cursor = next;
            }
        }
        Ok(())
    }

    async fn purge(&self) -> Result<(), Error> {
        let pattern = format!("{}*", self.prefix);
        for node in self.nodes().await? {
            let mut cursor = 0;
            loop {
                let (next, keys) = self.scan_page(&node, cursor, &pattern, 0).await?;
                if !keys.is_empty() {
                    match node {
                        Node::Server => {
                            let mut cmd = redis::cmd("DEL");
                            for key in &keys {
                                cmd.arg(key);
                            }
                            let _: i64 = self.send(&cmd).await?;
                        }
                        Node::Primary { .. } => {
                            let cmds: Vec<Cmd> = keys.iter().map(del_cmd).collect();
                            self.exec(&cmds).await?;
                        }
                    }
                }
                if next == 0 {
                    break;
                }
                cursor = next;
            }
        }
        Ok(())
    }

    async fn set(&self, ctx: &Context, values: &[Value]) -> Result<(), Error> {
        check_context(ctx)?;
        let cmds: Vec<Cmd> = values
            .iter()
            .map(|value| {
                let mut cmd = redis::cmd("SET");
                cmd.arg(self.key(value.name())).arg(value.value());
                cmd
            })
            .collect();
        self.exec(&cmds).await.map(|_| ())
    }

    async fn get(&self, ctx: &Context, name: &str) -> ReturnValue {
        if let Err(err) = check_context(ctx) {
            return new_return_error(err);
        }
        let mut cmd = redis::cmd("GET");
        cmd.arg(self.key(name));
        match self.send::<Option<String>>(&cmd).await {
            Ok(Some(value)) => new_return_value(value, true),
            Ok(None) => new_return_value("", false),
            Err(err) => new_return_error(err),
        }
    }

    async fn delete(&self, ctx: &Context, name: &str) -> Result<(), Error> {
        check_context(ctx)?;
        let mut cmd = redis::cmd("DEL");
        cmd.arg(self.key(name));
        let _: i64 = self.send(&cmd).await?;
        Ok(())
    }

    async fn add_scores(
        &self,
        ctx: &Context,
        collection: &str,
        subset: &str,
        documents: &[Score],
    ) -> Result<(), Error> {
        check_context(ctx)?;
        let cmds: Vec<Cmd> = documents
            .iter()
            .map(|doc| {
                let mut cmd = redis::cmd("HSET");
                cmd.arg(self.document_key(collection, subset, &doc.id))
                    .arg("collection")
                    .arg(collection)
                    .arg("subset")
                    .arg(subset)
                    .arg("id")
                    .arg(&doc.id)
                    .arg("score")
                    .arg(doc.score.to_string())
                    .arg("is_hidden")
                    .arg(bool_str(doc.is_hidden))
                    .arg("categories")
                    .arg(encode_categories(&doc.categories))
                    .arg("timestamp")
                    .arg(doc.timestamp.unix_micro());
                cmd
            })
            .collect();
        self.exec(&cmds).await.map(|_| ())
    }

    async fn search_scores(
        &self,
        ctx: &Context,
        collection: &str,
        subset: &str,
        query: &[String],
        begin: i64,
        end: i64,
    ) -> Result<Vec<Score>, Error> {
        check_context(ctx)?;
        let mut builder = format!("@collection:{{ {} }} @is_hidden:[0 0]", escape(collection));
        if !subset.is_empty() {
            builder.push_str(&format!(" @subset:{{ {} }}", escape(subset)));
        }
        for category in query {
            builder.push_str(&format!(
                " @categories:{{ {} }}",
                escape(&encode_category(category))
            ));
        }
        let limit = if end == -1 {
            MAX_SEARCH_LIMIT
        } else {
            end - begin
        };
        let reply = self.ft_search(&builder, begin, limit).await?;
        reply
            .docs
            .iter()
            .map(|(_, fields)| score_from_fields(fields))
            .collect()
    }

    /// Changes the matching documents, in two phases: the keys of every match
    /// are collected page by page, then each key is changed. A score patch can
    /// reorder the matches, so paging by offset while changing them would skip
    /// some.
    async fn update_scores(
        &self,
        ctx: &Context,
        collections: &[String],
        subset: Option<&str>,
        id: &str,
        patch: &ScorePatch,
    ) -> Result<(), Error> {
        check_context(ctx)?;
        if collections.is_empty() {
            return Ok(());
        }
        if patch.score.is_none() && patch.is_hidden.is_none() && patch.categories.is_none() {
            return Ok(());
        }
        let mut builder = format!(
            "@collection:{{ {} }} @id:{{ {} }}",
            escape(&collections.join(" | ")),
            escape(id)
        );
        if let Some(subset) = subset {
            builder.push_str(&format!(" @subset:{{ {} }}", escape(subset)));
        }
        let limit = if self.max_search_results <= 0 {
            MAX_SEARCH_LIMIT
        } else {
            self.max_search_results
        };

        let mut keys: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        let mut offset = 0;
        loop {
            let reply = self.ft_search(&builder, offset, limit).await?;
            if reply.docs.is_empty() {
                break;
            }
            let mut new_keys = 0;
            for (key, _) in &reply.docs {
                if seen.insert(key.clone()) {
                    keys.push(key.clone());
                    new_keys += 1;
                }
            }
            let page = reply.docs.len() as i64;
            offset += page;
            // Stop at a short page, or when a page adds no new key: the
            // defensive check Go makes for engines that count differently.
            if page < limit || new_keys == 0 {
                break;
            }
        }

        let mut values: Vec<(&str, String)> = Vec::new();
        if let Some(score) = patch.score {
            values.push(("score", score.to_string()));
        }
        if let Some(is_hidden) = patch.is_hidden {
            values.push(("is_hidden", bool_str(is_hidden).to_owned()));
        }
        if let Some(categories) = &patch.categories {
            values.push(("categories", encode_categories(categories)));
        }
        for key in &keys {
            // Go's `Watch` here runs no MULTI/EXEC, so the existence check and
            // the write are not atomic. The port keeps that.
            let mut exists = redis::cmd("EXISTS");
            exists.arg(key);
            if self.send::<i64>(&exists).await? == 0 {
                continue;
            }
            let mut hset = redis::cmd("HSET");
            hset.arg(key);
            for (field, value) in &values {
                hset.arg(*field).arg(value);
            }
            let _: i64 = self.send(&hset).await?;
        }
        Ok(())
    }

    async fn delete_scores(
        &self,
        ctx: &Context,
        collections: &[String],
        condition: &ScoreCondition,
    ) -> Result<(), Error> {
        check_context(ctx)?;
        condition.check()?;
        let mut builder = format!("@collection:{{ {} }}", escape(&collections.join(" | ")));
        if let Some(subset) = &condition.subset {
            builder.push_str(&format!(" @subset:{{ {} }}", escape(subset)));
        }
        if let Some(id) = &condition.id {
            builder.push_str(&format!(" @id:{{ {} }}", escape(id)));
        }
        if let Some(before) = &condition.before {
            builder.push_str(&format!(" @timestamp:[-inf ({}]", before.unix_micro()));
        }
        loop {
            let reply = self.ft_search(&builder, 0, MAX_SEARCH_LIMIT).await?;
            let cmds: Vec<Cmd> = reply.docs.iter().map(|(key, _)| del_cmd(key)).collect();
            self.exec(&cmds).await?;
            if reply.total == reply.docs.len() as i64 {
                break;
            }
        }
        Ok(())
    }

    async fn scan_scores(
        &self,
        ctx: &Context,
        callback: &mut (
                 dyn for<'a, 'b, 'c> FnMut(&'a str, &'b str, &'c str, Time) -> Result<(), Error>
                     + Send
             ),
    ) -> Result<(), Error> {
        check_context(ctx)?;
        let pattern = format!("{}*", self.document_table());
        for node in self.nodes().await? {
            let mut cursor = 0;
            loop {
                let (next, keys) = self
                    .scan_page(&node, cursor, &pattern, SCAN_SCORES_PAGE_SIZE)
                    .await?;
                if !keys.is_empty() {
                    // The whole page is fetched in one round trip. The callback
                    // still runs once per key, in scan order.
                    let cmds: Vec<Cmd> = keys
                        .iter()
                        .map(|key| {
                            let mut cmd = redis::cmd("HGETALL");
                            cmd.arg(key);
                            cmd
                        })
                        .collect();
                    for reply in self.exec(&cmds).await? {
                        check_context(ctx)?;
                        let row: HashMap<String, String> = parse(reply)?;
                        // A key deleted between SCAN and HGETALL reads as an
                        // empty row, so its timestamp fails to parse and the
                        // scan fails. Go does the same: its `redis.Nil` skip
                        // never fires, since HGETALL returns an empty map.
                        let (usec, err) = gostrconv::parse_int(field(&row, "timestamp"), 10, 64);
                        if let Some(err) = err {
                            return Err(stored_number(err));
                        }
                        callback(
                            field(&row, "collection"),
                            field(&row, "id"),
                            field(&row, "subset"),
                            gotime::unix_micro(usec).in_location(&gotime::utc()),
                        )?;
                    }
                }
                if next == 0 {
                    break;
                }
                cursor = next;
            }
        }
        Ok(())
    }

    async fn add_time_series_points(
        &self,
        ctx: &Context,
        points: &[TimeSeriesPoint],
    ) -> Result<(), Error> {
        check_context(ctx)?;
        let cmds: Vec<Cmd> = points
            .iter()
            .map(|point| {
                let mut cmd = redis::cmd("TS.ADD");
                cmd.arg(format!("{}:{}", self.points_table(), point.name))
                    .arg(point.timestamp.unix_milli())
                    .arg(point.value.to_string())
                    .arg("DUPLICATE_POLICY")
                    .arg("LAST");
                cmd
            })
            .collect();
        self.exec(&cmds).await.map(|_| ())
    }

    async fn get_time_series_points(
        &self,
        ctx: &Context,
        name: &str,
        begin: Time,
        end: Time,
        duration: Duration,
    ) -> Result<Vec<TimeSeriesPoint>, Error> {
        check_context(ctx)?;
        let bucket_ms = duration.milliseconds();
        let mut cmd = redis::cmd("TS.RANGE");
        cmd.arg(format!("{}:{}", self.points_table(), name))
            .arg(begin.unix_milli())
            .arg(end.unix_milli())
            .arg("AGGREGATION")
            .arg("last")
            .arg(bucket_ms);
        // Each reply is `[timestamp, value]`, the value as text.
        let samples: Vec<(i64, String)> = match self.send(&cmd).await {
            Ok(samples) => samples,
            Err(err) if is_missing_series(&err) => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        samples
            .into_iter()
            .map(|(millis, value)| {
                let (value, err) = gostrconv::parse_float(&value, 64);
                if let Some(err) = err {
                    return Err(stored_number(err));
                }
                Ok(TimeSeriesPoint {
                    name: name.to_owned(),
                    timestamp: gotime::unix_milli(millis).in_location(&gotime::utc()),
                    value,
                })
            })
            .collect()
    }
}

/// A `DEL` of one key.
fn del_cmd(key: impl redis::ToRedisArgs) -> Cmd {
    let mut cmd = redis::cmd("DEL");
    cmd.arg(key);
    cmd
}

#[cfg(test)]
mod tests {
    use gorse_common::gocontext::Context;
    use gorse_common::gotime::{self, Month, Time};

    use super::*;
    use crate::cache::database_test;
    use crate::cache::{Database, Score, ScoreCondition, ScorePatch};

    /// The Go test's `REDIS_URI`. Unset, the Redis tests are skipped, as Go's
    /// `t.Skip` skips them.
    fn redis_dsn() -> Option<String> {
        let dsn = std::env::var("REDIS_URI").ok().filter(|s| !s.is_empty());
        if dsn.is_none() {
            eprintln!("REDIS_URI is not set, skipping Redis test");
        }
        dsn
    }

    /// Opens a store under its own table prefix, purged and with its schema.
    /// Go's suite flushes the whole database; each test here owns a prefix, so
    /// tests that run in parallel do not purge each other's keys.
    async fn test_store(dsn: &str, name: &str) -> Redis {
        let db = open(dsn, &format!("gorse_{name}_"), &[]).unwrap();
        db.purge().await.unwrap();
        db.init().await.unwrap();
        db
    }

    /// A UTC date at midnight.
    fn utc(year: i64, month: Month, day: i64) -> Time {
        gotime::date(year, month, day, 0, 0, 0, 0, &gotime::utc())
    }

    /// The 2023-01-01 timestamp the tests use.
    fn ts() -> Time {
        utc(2023, Month::JANUARY, 1)
    }

    /// Compares score lists in order. Timestamps compare as instants.
    fn assert_docs(got: &[Score], want: &[Score]) {
        let key = |d: &Score| {
            (
                d.id.clone(),
                d.score,
                d.is_hidden,
                d.categories.clone(),
                d.timestamp.unix_nano(),
            )
        };
        let got: Vec<_> = got.iter().map(key).collect();
        let want: Vec<_> = want.iter().map(key).collect();
        assert_eq!(got, want);
    }

    /// Go's `TestRedis`: the shared suite runs against a live Redis.
    #[tokio::test]
    async fn redis_suite() {
        let Some(dsn) = redis_dsn() else { return };
        let db = test_store(&dsn, "suite").await;
        database_test::run(&db).await;
    }

    /// Go's `TestEscapeCharacters`.
    #[tokio::test]
    async fn escape_characters() {
        let Some(dsn) = redis_dsn() else { return };
        let db = test_store(&dsn, "escape").await;
        let ctx = Context::background();
        for c in ["-", ":", ".", "/"] {
            let collection = format!("a{c}1");
            let subset = format!("b{c}2");
            let id = format!("c{c}3");
            let doc = Score {
                id: id.clone(),
                score: f64::MAX,
                is_hidden: false,
                categories: vec!["a".into(), "b".into()],
                timestamp: ts(),
            };
            db.add_scores(&ctx, &collection, &subset, std::slice::from_ref(&doc))
                .await
                .unwrap();
            let documents = db
                .search_scores(&ctx, &collection, &subset, &["b".into()], 0, -1)
                .await
                .unwrap();
            assert_docs(&documents, std::slice::from_ref(&doc));

            db.update_scores(
                &ctx,
                std::slice::from_ref(&collection),
                None,
                &id,
                &ScorePatch {
                    score: Some(1.0),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            let documents = db
                .search_scores(&ctx, &collection, &subset, &["b".into()], 0, -1)
                .await
                .unwrap();
            assert_docs(&documents, &[Score { score: 1.0, ..doc }]);

            db.delete_scores(
                &ctx,
                std::slice::from_ref(&collection),
                &ScoreCondition {
                    subset: Some(subset.clone()),
                    id: Some(id.clone()),
                    before: None,
                },
            )
            .await
            .unwrap();
            let documents = db
                .search_scores(&ctx, &collection, &subset, &["b".into()], 0, -1)
                .await
                .unwrap();
            assert!(documents.is_empty());
        }
    }

    /// Go's `TestScanScoresMultiplePages`: a corpus larger than one SCAN page.
    /// Every document comes back exactly once, and the ids are the full set.
    #[tokio::test]
    async fn scan_scores_multiple_pages() {
        let Some(dsn) = redis_dsn() else { return };
        let db = test_store(&dsn, "scan_pages").await;
        let ctx = Context::background();
        const NUM_SCORES: usize = 2500;
        let written: Vec<Score> = (0..NUM_SCORES)
            .map(|i| Score {
                id: i.to_string(),
                score: i as f64,
                is_hidden: false,
                categories: vec!["a".into()],
                timestamp: ts(),
            })
            .collect();
        db.add_scores(&ctx, "collection", "subset", &written)
            .await
            .unwrap();

        let mut seen = std::collections::HashSet::new();
        let mut scanned = 0;
        db.scan_scores(&ctx, &mut |collection, id, subset, t| {
            scanned += 1;
            seen.insert(id.to_owned());
            assert_eq!(collection, "collection");
            assert_eq!(subset, "subset");
            assert_eq!(t.unix_nano(), ts().unix_nano());
            Ok(())
        })
        .await
        .unwrap();
        // A document returned twice would make `scanned` exceed the set size.
        assert_eq!(scanned, NUM_SCORES);
        assert_eq!(seen.len(), NUM_SCORES);
        for i in 0..NUM_SCORES {
            assert!(seen.contains(&i.to_string()), "missing document {i}");
        }
    }

    /// Go's `TestUpdateScoresWithPagination`.
    #[tokio::test]
    async fn update_scores_with_pagination() {
        let Some(dsn) = redis_dsn() else { return };
        let mut db = test_store(&dsn, "update_pages").await;
        let ctx = Context::background();
        db.max_search_results = 2;

        for i in 0..5 {
            let subset = format!("subset-{i}");
            let doc = Score {
                id: "shared-item".into(),
                score: i as f64,
                is_hidden: false,
                categories: vec!["old".into()],
                timestamp: gotime::now(),
            };
            db.add_scores(&ctx, "collection-a", &subset, &[doc])
                .await
                .unwrap();
        }

        db.update_scores(
            &ctx,
            &["collection-a".into()],
            None,
            "shared-item",
            &ScorePatch {
                categories: Some(vec!["new".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        for i in 0..5 {
            let subset = format!("subset-{i}");
            let docs = db
                .search_scores(&ctx, "collection-a", &subset, &["new".into()], 0, -1)
                .await
                .unwrap();
            assert_eq!(docs.len(), 1);
            assert_eq!(docs[0].id, "shared-item");
        }
    }

    /// Go's `TestUpdateScoresWithPaginationAndScorePatch`.
    #[tokio::test]
    async fn update_scores_with_pagination_and_score_patch() {
        let Some(dsn) = redis_dsn() else { return };
        let mut db = test_store(&dsn, "update_score").await;
        let ctx = Context::background();
        db.max_search_results = 1;

        let initial_scores = [3.0, 2.0, 1.0];
        for (i, score) in initial_scores.into_iter().enumerate() {
            let subset = format!("score-subset-{i}");
            let doc = Score {
                id: "shared-item".into(),
                score,
                is_hidden: false,
                categories: vec!["score-old".into()],
                timestamp: gotime::now(),
            };
            db.add_scores(&ctx, "collection-b", &subset, &[doc])
                .await
                .unwrap();
        }

        let target_score = 0.0;
        db.update_scores(
            &ctx,
            &["collection-b".into()],
            None,
            "shared-item",
            &ScorePatch {
                score: Some(target_score),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        for i in 0..initial_scores.len() {
            let subset = format!("score-subset-{i}");
            let docs = db
                .search_scores(&ctx, "collection-b", &subset, &[], 0, -1)
                .await
                .unwrap();
            assert_eq!(docs.len(), 1);
            assert_eq!(docs[0].score, target_score);
        }
    }

    /// Go's `TestUpdateScoresWithPaginationAndTiedScores`.
    #[tokio::test]
    async fn update_scores_with_pagination_and_tied_scores() {
        let Some(dsn) = redis_dsn() else { return };
        let mut db = test_store(&dsn, "update_tie").await;
        let ctx = Context::background();
        db.max_search_results = 2;

        for i in 0..5 {
            let subset = format!("tie-subset-{i}");
            let doc = Score {
                id: "shared-item".into(),
                score: 1.0,
                is_hidden: false,
                categories: vec!["tie-old".into()],
                timestamp: gotime::now(),
            };
            db.add_scores(&ctx, "collection-c", &subset, &[doc])
                .await
                .unwrap();
        }

        db.update_scores(
            &ctx,
            &["collection-c".into()],
            None,
            "shared-item",
            &ScorePatch {
                categories: Some(vec!["tie-new".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        for i in 0..5 {
            let subset = format!("tie-subset-{i}");
            let docs = db
                .search_scores(&ctx, "collection-c", &subset, &["tie-new".into()], 0, -1)
                .await
                .unwrap();
            assert_eq!(docs.len(), 1);
            assert_eq!(docs[0].id, "shared-item");
        }
    }

    /// Go's `TestEncodeDecodeCategories`. It needs no server.
    #[test]
    fn encode_decode_categories() {
        let encoded = encode_categories(&["z".into(), "h".into()]);
        assert_eq!(
            decode_categories(&encoded).unwrap(),
            vec!["z".to_owned(), "h".to_owned()]
        );

        let encoded = encode_categories(&[]);
        assert_eq!(decode_categories(&encoded).unwrap(), Vec::<String>::new());
    }
}
