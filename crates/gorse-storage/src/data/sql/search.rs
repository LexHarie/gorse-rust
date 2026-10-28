// Ports: storage/data/sql.go (Reconcile, the search index builders and SearchItems) @ add21b5

//! The full-text search index of the SQL data store.
//!
//! `Reconcile` brings the index in line with the search columns of the config:
//! MySQL adds a stored generated column and a `FULLTEXT` index over it,
//! PostgreSQL a GIN index over `to_tsvector` expressions, and SQLite an FTS5
//! table kept in step by three triggers. Each column expression is sorted
//! before it is joined, so the index text does not depend on the order of the
//! config (Go's `sort.Strings`, here [`gosort`]).
//!
//! Go's matching checks compare the text of the existing index with the text
//! this build would write, after lower-casing and dropping the spaces. The
//! servers rewrite that text, so the MySQL and PostgreSQL checks do not match
//! and the index is rebuilt. The port keeps that behaviour; see
//! `PORTING_NOTES.md`.

use gorse_common::gocontext::Context;
use gorse_common::gosort;
use gorse_config::config::SearchConfig;

use super::super::Error;
use super::super::types::ScoredItem;
use super::items;
use super::query::{self, Driver, Sql};
use super::{Pools, SqlDatabase};

/// The name of the search index on every dialect (`searchIndexName`).
pub const SEARCH_INDEX_NAME: &str = "gorse_search_index";
/// The generated column MySQL keeps the document in (`mysqlSearchDocumentColumn`).
const MYSQL_SEARCH_DOCUMENT_COLUMN: &str = "gorse_search_document";
const SQLITE_SEARCH_INSERT_TRIGGER: &str = "gorse_search_index_insert";
const SQLITE_SEARCH_UPDATE_TRIGGER: &str = "gorse_search_index_update";
const SQLITE_SEARCH_DELETE_TRIGGER: &str = "gorse_search_index_delete";

/// Sorts strings as Go's `sort.Strings` does.
fn sort_strings(values: &mut [String]) {
    struct Strings<'a>(&'a mut [String]);

    impl gosort::Interface for Strings<'_> {
        fn len(&self) -> usize {
            self.0.len()
        }

        fn less(&self, i: usize, j: usize) -> bool {
            self.0[i] < self.0[j]
        }

        fn swap(&mut self, i: usize, j: usize) {
            self.0.swap(i, j)
        }
    }

    gosort::sort(&mut Strings(values));
}

/// Go's `isPostgresJSONPathElement`: a non-empty run of ASCII letters, digits
/// and underscores, which can be written unquoted in a JSON path.
fn is_json_path_element(element: &str) -> bool {
    !element.is_empty()
        && element
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// The path of a `item.Labels.<path>` column, or `None` for any other column.
/// The path is valid only when every element is a JSON path element.
fn labels_path(column: &str) -> Option<Vec<&str>> {
    const PREFIX: &str = "item.Labels.";
    let rest = column.strip_prefix(PREFIX)?;
    if rest.is_empty() {
        return None;
    }
    let path: Vec<&str> = rest.split('.').collect();
    path.iter().all(|e| is_json_path_element(e)).then_some(path)
}

/// Go's `mysqlJSONPath`: `$."a"."b"`.
fn mysql_json_path(path: &[&str]) -> String {
    format!("$.\"{}\"", path.join("\".\""))
}

/// The PostgreSQL expression for a search column, or `None` for a column the
/// index does not cover (Go's `postgresSearchColumn`).
fn postgres_search_column(column: &str) -> Option<String> {
    match column {
        "item.ItemId" => Some("item_id".to_owned()),
        "item.Categories" => Some("categories::text".to_owned()),
        "item.Comment" => Some("comment".to_owned()),
        "item.Labels" => Some("labels::text".to_owned()),
        _ => labels_path(column).map(|path| format!("labels::jsonb #>> '{{{}}}'", path.join(","))),
    }
}

/// The MySQL expression for a search column (Go's `mysqlSearchColumn`).
fn mysql_search_column(column: &str) -> Option<String> {
    match column {
        "item.ItemId" => Some("item_id".to_owned()),
        "item.Categories" => Some("JSON_UNQUOTE(JSON_EXTRACT(categories, '$'))".to_owned()),
        "item.Comment" => Some("comment".to_owned()),
        "item.Labels" => Some("JSON_UNQUOTE(JSON_EXTRACT(labels, '$'))".to_owned()),
        _ => labels_path(column).map(|path| {
            format!(
                "JSON_UNQUOTE(JSON_EXTRACT(labels, '{}'))",
                mysql_json_path(&path)
            )
        }),
    }
}

/// The SQLite expression for a search column, with `qualifier.` in front of
/// each column when it is not empty (Go's `sqliteSearchColumn`).
fn sqlite_search_column(column: &str, qualifier: &str) -> Option<String> {
    let name = |name: &str| {
        if qualifier.is_empty() {
            name.to_owned()
        } else {
            format!("{qualifier}.{name}")
        }
    };
    match column {
        "item.ItemId" => Some(name("item_id")),
        "item.Categories" => Some(format!("json_extract({}, '$')", name("categories"))),
        "item.Comment" => Some(name("comment")),
        "item.Labels" => Some(format!("json_extract({}, '$')", name("labels"))),
        _ => labels_path(column).map(|path| {
            format!(
                "json_extract({}, '{}')",
                name("labels"),
                mysql_json_path(&path)
            )
        }),
    }
}

/// Go's `buildPostgresSearchVector`: one `to_tsvector` per column, sorted, joined
/// with ` || `. Empty when no column is searchable.
pub fn build_postgres_search_vector(columns: &[String]) -> String {
    let mut vectors: Vec<String> = columns
        .iter()
        .filter_map(|column| postgres_search_column(column))
        .map(|column| format!("to_tsvector('simple', coalesce({column}, ''))"))
        .collect();
    sort_strings(&mut vectors);
    vectors.join(" || ")
}

/// Go's `buildMySQLSearchDocument`: the columns in a sorted `CONCAT_WS`, or
/// empty when no column is searchable.
pub fn build_mysql_search_document(columns: &[String]) -> String {
    let mut parts: Vec<String> = columns
        .iter()
        .filter_map(|column| mysql_search_column(column))
        .collect();
    if parts.is_empty() {
        return String::new();
    }
    sort_strings(&mut parts);
    format!("CONCAT_WS(' ', {})", parts.join(", "))
}

/// Go's `buildSQLiteSearchDocument`: each column coalesced to text, sorted,
/// joined with a space. `qualifier` is `new` or `old` in a trigger, else empty.
pub fn build_sqlite_search_document(columns: &[String], qualifier: &str) -> String {
    let mut parts: Vec<String> = columns
        .iter()
        .filter_map(|column| sqlite_search_column(column, qualifier))
        .map(|column| format!("coalesce({column}, '')"))
        .collect();
    sort_strings(&mut parts);
    parts.join(" || ' ' || ")
}

/// Go's `sqliteAnyTokenSearchQuery`: each term quoted, joined with `OR`, so a
/// document that holds any term matches.
fn sqlite_any_token_search_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Go's `normalizeSearchIndexExpression`: lower case, and no white space.
fn normalize(expression: &str) -> String {
    expression
        .to_lowercase()
        .split_whitespace()
        .collect::<String>()
}

/// Go's `searchExpressionContainsAll`: whether the normalized `existing`
/// contains each normalized part. Empty parts are skipped.
fn expression_contains_all<'a>(existing: &str, parts: impl IntoIterator<Item = &'a str>) -> bool {
    let existing = normalize(existing);
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .all(|part| existing.contains(&normalize(part)))
}

/// Go's `mysqlSearchDocumentParts`: the parts of a `CONCAT_WS(' ', ...)`
/// document, or the whole document when it is not one.
fn mysql_search_document_parts(document: &str) -> Vec<&str> {
    const PREFIX: &str = "CONCAT_WS(' ', ";
    match document
        .strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_suffix(')'))
    {
        Some(inner) => inner.split(", ").collect(),
        None => vec![document],
    }
}

/// Go's `sqliteObjectName`: the name in a `CREATE TRIGGER <name> ...` statement,
/// the third word.
fn sqlite_object_name(create_sql: &str) -> &str {
    create_sql.split_whitespace().nth(2).unwrap_or_default()
}

impl SqlDatabase {
    /// Go's `Reconcile`: brings the search index in line with `config`.
    pub(super) async fn reconcile(&self, config: &SearchConfig) -> Result<(), Error> {
        let columns = config.columns.as_deref().unwrap_or_default();
        if columns.is_empty() {
            return Ok(());
        }
        match self.driver() {
            Driver::Postgres => self.reconcile_postgres(columns).await,
            Driver::MySql => self.reconcile_mysql(columns).await,
            Driver::Sqlite => self.reconcile_sqlite(columns).await,
        }
    }

    async fn reconcile_postgres(&self, columns: &[String]) -> Result<(), Error> {
        let vector = build_postgres_search_vector(columns);
        if vector.is_empty() {
            return Ok(());
        }
        if self.postgres_search_index_matches(&vector).await? {
            self.set_search_vector(vector);
            return Ok(());
        }
        let mut sql = self.sql();
        sql.push(&format!("DROP INDEX IF EXISTS {SEARCH_INDEX_NAME}"));
        self.exec(&sql).await?;
        let mut sql = self.sql();
        sql.push(&format!(
            "CREATE INDEX {SEARCH_INDEX_NAME} ON {} USING GIN (({vector}))",
            self.items_table()
        ));
        self.exec(&sql).await?;
        self.set_search_vector(vector);
        Ok(())
    }

    async fn postgres_search_index_matches(&self, vector: &str) -> Result<bool, Error> {
        let mut sql = self.sql();
        sql.push(
            "SELECT indexdef FROM pg_indexes WHERE schemaname = ANY(current_schemas(false)) AND tablename = ",
        )
        .bind(self.items_table())
        .push(" AND indexname = ")
        .bind(SEARCH_INDEX_NAME);
        let index_def = self.one_text(&sql).await?.unwrap_or_default();
        if index_def.is_empty() {
            return Ok(false);
        }
        Ok(expression_contains_all(&index_def, vector.split(" || ")))
    }

    async fn reconcile_mysql(&self, columns: &[String]) -> Result<(), Error> {
        let document = build_mysql_search_document(columns);
        if document.is_empty() {
            return Ok(());
        }
        let items = self.items_table();
        if self.mysql_search_index_matches(&items, &document).await? {
            return Ok(());
        }
        self.drop_mysql_index_if_exists(&items, SEARCH_INDEX_NAME)
            .await?;
        self.drop_mysql_column_if_exists(&items, MYSQL_SEARCH_DOCUMENT_COLUMN)
            .await?;
        let mut sql = self.sql();
        sql.push(&format!(
            "ALTER TABLE {items} ADD COLUMN {MYSQL_SEARCH_DOCUMENT_COLUMN} TEXT GENERATED ALWAYS AS ({document}) STORED"
        ));
        self.exec(&sql).await?;
        let mut sql = self.sql();
        sql.push(&format!(
            "CREATE FULLTEXT INDEX {SEARCH_INDEX_NAME} ON {items}({MYSQL_SEARCH_DOCUMENT_COLUMN})"
        ));
        self.exec(&sql).await?;
        Ok(())
    }

    async fn mysql_search_index_matches(&self, table: &str, document: &str) -> Result<bool, Error> {
        let mut sql = self.sql();
        sql.push("SELECT COUNT(*) FROM information_schema.statistics WHERE table_schema = DATABASE() AND table_name = ")
            .bind(table)
            .push(" AND index_name = ")
            .bind(SEARCH_INDEX_NAME)
            .push(" AND column_name = ")
            .bind(MYSQL_SEARCH_DOCUMENT_COLUMN);
        if self.one_integer(&sql).await?.unwrap_or_default() == 0 {
            return Ok(false);
        }
        let mut sql = self.sql();
        sql.push("SELECT generation_expression FROM information_schema.columns WHERE table_schema = DATABASE() AND table_name = ")
            .bind(table)
            .push(" AND column_name = ")
            .bind(MYSQL_SEARCH_DOCUMENT_COLUMN);
        let expression = self.one_text(&sql).await?.unwrap_or_default();
        if expression.is_empty() {
            return Ok(false);
        }
        Ok(normalize(&expression) == normalize(document)
            || expression_contains_all(&expression, mysql_search_document_parts(document)))
    }

    /// Go's `dropMySQLIndexIfExists`.
    async fn drop_mysql_index_if_exists(&self, table: &str, index: &str) -> Result<(), Error> {
        let mut sql = self.sql();
        sql.push("SELECT COUNT(*) FROM information_schema.statistics WHERE table_schema = DATABASE() AND table_name = ")
            .bind(table)
            .push(" AND index_name = ")
            .bind(index);
        if self.one_integer(&sql).await?.unwrap_or_default() == 0 {
            return Ok(());
        }
        let mut sql = self.sql();
        sql.push(&format!("DROP INDEX {index} ON {table}"));
        self.exec(&sql).await?;
        Ok(())
    }

    /// Go's `dropMySQLColumnIfExists`.
    async fn drop_mysql_column_if_exists(&self, table: &str, column: &str) -> Result<(), Error> {
        let mut sql = self.sql();
        sql.push("SELECT COUNT(*) FROM information_schema.columns WHERE table_schema = DATABASE() AND table_name = ")
            .bind(table)
            .push(" AND column_name = ")
            .bind(column);
        if self.one_integer(&sql).await?.unwrap_or_default() == 0 {
            return Ok(());
        }
        let mut sql = self.sql();
        sql.push(&format!("ALTER TABLE {table} DROP COLUMN {column}"));
        self.exec(&sql).await?;
        Ok(())
    }

    async fn reconcile_sqlite(&self, columns: &[String]) -> Result<(), Error> {
        let search_document = build_sqlite_search_document(columns, "");
        let new_document = build_sqlite_search_document(columns, "new");
        let old_document = build_sqlite_search_document(columns, "old");
        if search_document.is_empty() {
            return Ok(());
        }
        let items = self.items_table();
        let insert_trigger = format!(
            "CREATE TRIGGER {SQLITE_SEARCH_INSERT_TRIGGER} AFTER INSERT ON {items} BEGIN INSERT INTO {SEARCH_INDEX_NAME}(item_id, search_document) VALUES (new.item_id, {new_document}); END"
        );
        let update_trigger = format!(
            "CREATE TRIGGER {SQLITE_SEARCH_UPDATE_TRIGGER} AFTER UPDATE ON {items} BEGIN DELETE FROM {SEARCH_INDEX_NAME} WHERE item_id = old.item_id; INSERT INTO {SEARCH_INDEX_NAME}(item_id, search_document) VALUES (new.item_id, {new_document}); END"
        );
        let delete_trigger = format!(
            "CREATE TRIGGER {SQLITE_SEARCH_DELETE_TRIGGER} AFTER DELETE ON {items} BEGIN DELETE FROM {SEARCH_INDEX_NAME} WHERE item_id = old.item_id AND search_document = {old_document}; END"
        );
        if self
            .sqlite_search_index_matches(&[&insert_trigger, &update_trigger, &delete_trigger])
            .await?
        {
            return Ok(());
        }
        for trigger in [
            SQLITE_SEARCH_INSERT_TRIGGER,
            SQLITE_SEARCH_UPDATE_TRIGGER,
            SQLITE_SEARCH_DELETE_TRIGGER,
        ] {
            let mut sql = self.sql();
            sql.push(&format!("DROP TRIGGER IF EXISTS {trigger}"));
            self.exec(&sql).await?;
        }
        let mut sql = self.sql();
        sql.push(&format!("DROP TABLE IF EXISTS {SEARCH_INDEX_NAME}"));
        self.exec(&sql).await?;
        let mut sql = self.sql();
        sql.push(&format!(
            "CREATE VIRTUAL TABLE {SEARCH_INDEX_NAME} USING fts5(item_id UNINDEXED, search_document)"
        ));
        self.exec(&sql).await?;
        let mut sql = self.sql();
        sql.push(&format!(
            "INSERT INTO {SEARCH_INDEX_NAME}(item_id, search_document) SELECT item_id, {search_document} FROM {items}"
        ));
        self.exec(&sql).await?;
        for trigger in [&insert_trigger, &update_trigger, &delete_trigger] {
            let mut sql = self.sql();
            sql.push(trigger);
            self.exec(&sql).await?;
        }
        Ok(())
    }

    async fn sqlite_search_index_matches(&self, triggers: &[&String]) -> Result<bool, Error> {
        for trigger in triggers {
            let mut sql = self.sql();
            sql.push("SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ")
                .bind(sqlite_object_name(trigger));
            let existing = self.one_text(&sql).await?.unwrap_or_default();
            if existing.is_empty() || normalize(&existing) != normalize(trigger) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Go's `SearchItems`: up to `n` items that match `query`, best first, with
    /// their relevance score.
    pub(super) async fn search_items(
        &self,
        ctx: &Context,
        query: &str,
        n: i64,
    ) -> Result<Vec<ScoredItem>, Error> {
        query::check(ctx)?;
        if n <= 0 {
            return Ok(Vec::new());
        }
        let sql = self.search_sql(query, n);
        match &self.pools {
            Pools::MySql(pool) => query::fetch_all(pool, &sql, items::scored_item_float).await,
            Pools::Postgres(pool) => query::fetch_all(pool, &sql, items::scored_item_text).await,
            Pools::Sqlite(pool) => query::fetch_all(pool, &sql, items::scored_item_float).await,
        }
    }

    /// The search statement for `query`, for this store's dialect.
    fn search_sql(&self, query: &str, n: i64) -> Sql {
        let driver = self.driver();
        let items = self.items_table();
        let mut sql = self.sql();
        match driver {
            Driver::Postgres => {
                let vector = self.search_vector();
                let rank = |sql: &mut Sql| {
                    sql.push(&format!("ts_rank({vector}, "));
                    push_tsquery(sql, query);
                    sql.push(")");
                };
                sql.push(&format!("SELECT {}, ", items::columns(driver, "")));
                rank(&mut sql);
                sql.push("::text AS score FROM ")
                    .push(&items)
                    .push(&format!(" WHERE {vector} @@ ("));
                push_tsquery(&mut sql, query);
                sql.push(") ORDER BY ");
                rank(&mut sql);
                sql.push(" DESC LIMIT ").bind(n);
                sql
            }
            Driver::MySql => {
                let column = MYSQL_SEARCH_DOCUMENT_COLUMN;
                sql.push(&format!(
                    "SELECT {}, MATCH({column}) AGAINST (",
                    items::columns(driver, "")
                ))
                .bind(query)
                .push(" IN NATURAL LANGUAGE MODE) AS score FROM ")
                .push(&items)
                .push(&format!(" WHERE MATCH({column}) AGAINST ("))
                .bind(query)
                .push(" IN NATURAL LANGUAGE MODE) ORDER BY MATCH(")
                .push(column)
                .push(") AGAINST (")
                .bind(query)
                .push(" IN NATURAL LANGUAGE MODE) DESC LIMIT ")
                .bind(n);
                sql
            }
            Driver::Sqlite => {
                let query = sqlite_any_token_search_query(query);
                sql.push(&format!(
                    "SELECT {}, -bm25({SEARCH_INDEX_NAME}) AS score FROM {items} AS items JOIN {SEARCH_INDEX_NAME} ON items.item_id = {SEARCH_INDEX_NAME}.item_id WHERE {SEARCH_INDEX_NAME} MATCH ",
                    items::columns(driver, "items.")
                ))
                .bind(query)
                .push(&format!(" ORDER BY bm25({SEARCH_INDEX_NAME}) LIMIT "))
                .bind(n);
                sql
            }
        }
    }
}

/// Appends `plainto_tsquery('simple', term)` for each term of `query`, joined
/// with `||` (Go's search query on PostgreSQL). Each term is a bound value.
fn push_tsquery(sql: &mut Sql, query: &str) {
    for (i, term) in query.split_whitespace().enumerate() {
        if i > 0 {
            sql.push(" || ");
        }
        sql.push("plainto_tsquery('simple', ").bind(term).push(")");
    }
}
