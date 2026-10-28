// Command schemadump creates the data, cache and meta stores with the Go
// implementation's own Init/Reconcile code and dumps the resulting schemas, so
// that contracts/schema/ records what Go actually creates rather than what we
// think it creates. Postgres is dumped afterwards with pg_dump (see
// contracts/tools/dump-schemas.sh); every other backend is dumped here.
//
// Usage: go run ./contracts/tools/schemadump -out contracts/schema \
//
//	-mysql 'root:password@tcp(127.0.0.1:3306)/' \
//	-postgres 'postgres://gorse:gorse_pass@127.0.0.1:5432/' \
//	-mongo 'mongodb://root:password@127.0.0.1:27017/' -redis 'redis://127.0.0.1:6379/'
//
// With -dump-only the tool changes nothing. It writes one dump of an existing
// database to stdout, in the format the full run writes for that kind of store,
// so a database created by another build can be checked against the same
// contract:
//
//	go run ./contracts/tools/schemadump -dump-only mysql -target 'root:password@tcp(127.0.0.1:3306)/gorse_rs_schema_data?parseTime=true'
//	go run ./contracts/tools/schemadump -dump-only postgres -database gorse_rs_schema_data
//	go run ./contracts/tools/schemadump -dump-only mongodb -target 'mongodb://root:password@127.0.0.1:27017/' -database gorse_rs_schema_data
//	go run ./contracts/tools/schemadump -dump-only sqlite -target /path/to/data.sqlite
//	go run ./contracts/tools/schemadump -dump-only redis -target 'redis://127.0.0.1:6379/'
//
// postgres runs pg_dump inside -postgres-container, as dump-schemas.sh does.
package main

import (
	"context"
	"database/sql"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"time"

	_ "github.com/go-sql-driver/mysql"
	"github.com/gorse-io/gorse/config"
	"github.com/gorse-io/gorse/storage/cache"
	"github.com/gorse-io/gorse/storage/data"
	"github.com/gorse-io/gorse/storage/meta"
	_ "github.com/lib/pq"
	"github.com/redis/go-redis/v9"
	"go.mongodb.org/mongo-driver/bson"
	"go.mongodb.org/mongo-driver/mongo"
	"go.mongodb.org/mongo-driver/mongo/options"
	_ "modernc.org/sqlite"
)

// searchConfig exercises every search column kind accepted by Reconcile.
var searchConfig = config.SearchConfig{Columns: []string{"item.ItemId", "item.Categories", "item.Comment", "item.Labels", "item.Labels.title"}}

const (
	dataDB  = "gorse_schema_data"
	cacheDB = "gorse_schema_cache"
)

func main() {
	out := flag.String("out", "contracts/schema", "output directory")
	mysqlDSN := flag.String("mysql", "", "MySQL server DSN without database, e.g. root:password@tcp(127.0.0.1:3306)/")
	postgresURL := flag.String("postgres", "", "Postgres server URL without database")
	mongoURL := flag.String("mongo", "", "MongoDB server URL without database")
	redisURL := flag.String("redis", "", "Redis URL")
	dumpOnly := flag.String("dump-only", "", "dump one existing store to stdout and change nothing: sqlite, mysql, postgres, mongodb or redis")
	target := flag.String("target", "", "with -dump-only: the file (sqlite), the DSN with database (mysql), the URL without database (mongodb) or the URL (redis)")
	database := flag.String("database", "", "with -dump-only postgres or mongodb: the database name")
	postgresContainer := flag.String("postgres-container", "storage-postgres-1", "with -dump-only postgres: the container whose pg_dump runs")
	flag.Parse()
	if *dumpOnly != "" {
		os.Stdout.WriteString(dumpExisting(*dumpOnly, *target, *database, *postgresContainer))
		return
	}
	tmp := must1(os.MkdirTemp("", "gorse-schemadump-"))
	defer os.RemoveAll(tmp)
	config.RootDir = tmp
	for _, d := range []string{"data", "cache", "meta", "samples"} {
		must(os.MkdirAll(filepath.Join(*out, d), 0o755))
	}

	// SQLite
	sqliteData := filepath.Join(tmp, "data.sqlite")
	sqliteCache := filepath.Join(tmp, "cache.sqlite")
	initData("sqlite://"+sqliteData, false)
	writeFile(*out, "data/sqlite.sql", dumpSQLite(sqliteData))
	writeFile(*out, "samples/data.sqlite.json", dumpRows("sqlite", sqliteData, "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"))
	initData("sqlite://"+sqliteData, true)
	writeFile(*out, "data/sqlite.search.sql", dumpSQLite(sqliteData))
	initCache("sqlite://" + sqliteCache)
	writeFile(*out, "cache/sqlite.sql", dumpSQLite(sqliteCache))
	writeFile(*out, "samples/cache.sqlite.json", dumpRows("sqlite", sqliteCache, "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"))
	metaPath := filepath.Join(tmp, "meta.sqlite3")
	metaDB := must1(meta.Open("sqlite://"+metaPath, 0))
	must(metaDB.Init())
	must(metaDB.Close())
	writeFile(*out, "meta/sqlite.sql", dumpSQLite(metaPath))

	// MySQL
	if *mysqlDSN != "" {
		root := must1(sql.Open("mysql", *mysqlDSN))
		for _, db := range []string{dataDB, cacheDB} {
			_ = must1(root.Exec("DROP DATABASE IF EXISTS " + db))
			_ = must1(root.Exec("CREATE DATABASE " + db))
		}
		must(root.Close())
		initData("mysql://"+*mysqlDSN+dataDB+"?parseTime=true", false)
		writeFile(*out, "data/mysql.sql", dumpMySQL(*mysqlDSN+dataDB))
		writeFile(*out, "samples/data.mysql.json", dumpRows("mysql", *mysqlDSN+dataDB, "SHOW TABLES"))
		initData("mysql://"+*mysqlDSN+dataDB+"?parseTime=true", true)
		writeFile(*out, "data/mysql.search.sql", dumpMySQL(*mysqlDSN+dataDB))
		initCache("mysql://" + *mysqlDSN + cacheDB + "?parseTime=true")
		writeFile(*out, "cache/mysql.sql", dumpMySQL(*mysqlDSN+cacheDB))
		writeFile(*out, "samples/cache.mysql.json", dumpRows("mysql", *mysqlDSN+cacheDB, "SHOW TABLES"))
	}

	// Postgres: create the stores here, dump with pg_dump afterwards.
	if *postgresURL != "" {
		root := must1(sql.Open("postgres", *postgresURL+"postgres?sslmode=disable"))
		for _, db := range []string{dataDB, dataDB + "_search", cacheDB} {
			_ = must1(root.Exec("DROP DATABASE IF EXISTS " + db))
			_ = must1(root.Exec("CREATE DATABASE " + db))
		}
		must(root.Close())
		initData(*postgresURL+dataDB+"?sslmode=disable", false)
		initData(*postgresURL+dataDB+"_search?sslmode=disable", true)
		initCache(*postgresURL + cacheDB + "?sslmode=disable")
		const pgTables = "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' ORDER BY table_name"
		writeFile(*out, "samples/data.postgres.json", dumpRows("postgres", *postgresURL+dataDB+"?sslmode=disable", pgTables))
		writeFile(*out, "samples/cache.postgres.json", dumpRows("postgres", *postgresURL+cacheDB+"?sslmode=disable", pgTables))
	}

	// MongoDB
	if *mongoURL != "" {
		client := must1(mongo.Connect(context.Background(), options.Client().ApplyURI(*mongoURL)))
		for _, db := range []string{dataDB, cacheDB} {
			must(client.Database(db).Drop(context.Background()))
		}
		initData(*mongoURL+dataDB+"?authSource=admin", false)
		writeFile(*out, "data/mongodb.json", dumpMongo(client, dataDB))
		writeFile(*out, "samples/data.mongodb.json", dumpMongoDocs(client, dataDB))
		initData(*mongoURL+dataDB+"?authSource=admin", true)
		writeFile(*out, "data/mongodb.search.json", dumpMongo(client, dataDB))
		initCache(*mongoURL + cacheDB + "?authSource=admin")
		writeFile(*out, "cache/mongodb.json", dumpMongo(client, cacheDB))
		writeFile(*out, "samples/cache.mongodb.json", dumpMongoDocs(client, cacheDB))
		must(client.Disconnect(context.Background()))
	}

	// Redis
	if *redisURL != "" {
		opt := must1(redis.ParseURL(*redisURL))
		opt.Protocol = 2
		client := redis.NewClient(opt)
		ctx := context.Background()
		must(client.FlushAll(ctx).Err())
		initCache(*redisURL)
		writeFile(*out, "cache/redis.ft-info.txt", ftInfo(ctx, client))
		writeFile(*out, "samples/cache.redis.json", dumpRedis(client))
		must(client.Close())
	}
}

func initData(url string, search bool) {
	db := must1(data.Open(url, ""))
	must(db.Init())
	if search {
		must(db.Reconcile(searchConfig))
	} else {
		insertDataSamples(db)
	}
	must(db.Close())
}

func initCache(url string) {
	db := must1(cache.Open(url, ""))
	must(db.Init())
	insertCacheSamples(db)
	must(db.Close())
}

var sampleTime = time.Date(2026, 1, 2, 3, 4, 5, 678901000, time.UTC)

// insertDataSamples writes one user, item and feedback through the Go data
// store so the dumps show how values are encoded at rest. update_at columns are
// assigned by storage (time.Now) and differ between runs.
func insertDataSamples(db data.Database) {
	ctx := context.Background()
	must(db.BatchInsertUsers(ctx, []data.User{{
		UserId:  "u1",
		Labels:  map[string]any{"gender": "F", "tags": []any{"a", "b"}},
		Comment: "user comment",
	}}))
	must(db.BatchInsertItems(ctx, []data.Item{{
		ItemId:     "i1",
		IsHidden:   true,
		Categories: []string{"c1", "c2"},
		Timestamp:  sampleTime,
		Labels:     map[string]any{"title": "Hello", "embedding": []any{0.25, 0.5}},
		Comment:    "item comment",
	}}))
	must(db.BatchInsertFeedback(ctx, []data.Feedback{{
		FeedbackKey: data.FeedbackKey{FeedbackType: "like", UserId: "u1", ItemId: "i1"},
		Value:       1.5,
		Timestamp:   sampleTime.Add(time.Second),
		Comment:     "feedback comment",
	}}, true, true, true))
}

// insertCacheSamples writes one value of each kind through the Go cache store.
func insertCacheSamples(db cache.Database) {
	ctx := context.Background()
	must(db.Set(ctx,
		cache.Integer(cache.Key(cache.GlobalMeta, cache.NumUsers), 10),
		cache.Time(cache.Key(cache.LastModifyUserTime, "u1"), sampleTime),
		cache.String(cache.Key(cache.RecommendDigest, "u1"), "0123456789abcdef0123456789abcdef"),
	))
	must(db.AddScores(ctx, cache.Recommend, "u1", []cache.Score{{
		Id: "i1", Score: 0.5, IsHidden: false, Categories: []string{"", "c1"}, Timestamp: sampleTime,
	}}))
	must(db.AddTimeSeriesPoints(ctx, []cache.TimeSeriesPoint{{Name: cache.NumUsers, Timestamp: sampleTime, Value: 10}}))
}

func dumpSQLite(path string) string {
	db := must1(sql.Open("sqlite", path))
	defer db.Close()
	rows := must1(db.Query("SELECT type, name, tbl_name, sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY tbl_name, type DESC, name"))
	defer rows.Close()
	var b strings.Builder
	b.WriteString("-- Generated by contracts/tools/schemadump from the Go implementation. Do not edit.\n")
	for rows.Next() {
		var typ, name, tbl, stmt string
		must(rows.Scan(&typ, &name, &tbl, &stmt))
		fmt.Fprintf(&b, "\n-- %s %s (table %s)\n%s;\n", typ, name, tbl, stmt)
	}
	must(rows.Err())
	return b.String()
}

func dumpMySQL(dsn string) string {
	db := must1(sql.Open("mysql", dsn))
	defer db.Close()
	var version string
	must(db.QueryRow("SELECT VERSION()").Scan(&version))
	tables := []string{}
	rows := must1(db.Query("SHOW TABLES"))
	for rows.Next() {
		var t string
		must(rows.Scan(&t))
		tables = append(tables, t)
	}
	must(rows.Close())
	sort.Strings(tables)
	var b strings.Builder
	fmt.Fprintf(&b, "-- Generated by contracts/tools/schemadump from the Go implementation. Do not edit.\n-- Server: MySQL %s\n", version)
	for _, t := range tables {
		var name, stmt string
		must(db.QueryRow("SHOW CREATE TABLE `"+t+"`").Scan(&name, &stmt))
		fmt.Fprintf(&b, "\n%s;\n", stmt)
	}
	return b.String()
}

func dumpMongo(client *mongo.Client, dbName string) string {
	ctx := context.Background()
	db := client.Database(dbName)
	specs := must1(db.ListCollectionSpecifications(ctx, bson.M{}))
	sort.Slice(specs, func(i, j int) bool { return specs[i].Name < specs[j].Name })
	out := []map[string]any{}
	for _, spec := range specs {
		cursor := must1(db.Collection(spec.Name).Indexes().List(ctx))
		var indexes []bson.M
		must(cursor.All(ctx, &indexes))
		for _, idx := range indexes {
			delete(idx, "v") // index format version is chosen by the server, not by Gorse
		}
		sort.Slice(indexes, func(i, j int) bool { return fmt.Sprint(indexes[i]["name"]) < fmt.Sprint(indexes[j]["name"]) })
		var opts bson.M
		must(bson.Unmarshal(spec.Options, &opts))
		out = append(out, map[string]any{"name": spec.Name, "type": spec.Type, "options": opts, "indexes": indexes})
	}
	buf := must1(json.MarshalIndent(map[string]any{
		"_generated":  "contracts/tools/schemadump from the Go implementation. Do not edit.",
		"collections": out,
	}, "", "  "))
	return string(buf) + "\n"
}

// dumpRows renders every row of every table as strings, i.e. exactly what the
// driver returns, so encodings (JSON text, timestamps, booleans) are visible.
func dumpRows(driver, dsn, listTables string) string {
	db := must1(sql.Open(driver, dsn))
	defer db.Close()
	var tables []string
	rows := must1(db.Query(listTables))
	for rows.Next() {
		var t string
		must(rows.Scan(&t))
		tables = append(tables, t)
	}
	must(rows.Close())
	sort.Strings(tables)
	quote := func(t string) string {
		if driver == "mysql" {
			return "`" + t + "`"
		}
		return `"` + t + `"`
	}
	result := []map[string]any{}
	for _, t := range tables {
		rows := must1(db.Query("SELECT * FROM " + quote(t)))
		cols := must1(rows.Columns())
		var records [][]any
		for rows.Next() {
			values := make([]sql.NullString, len(cols))
			ptrs := make([]any, len(cols))
			for i := range values {
				ptrs[i] = &values[i]
			}
			must(rows.Scan(ptrs...))
			record := make([]any, len(cols))
			for i, v := range values {
				if v.Valid {
					record[i] = v.String
				}
			}
			records = append(records, record)
		}
		must(rows.Close())
		result = append(result, map[string]any{"table": t, "columns": cols, "rows": records})
	}
	buf := must1(json.MarshalIndent(map[string]any{
		"_generated": "contracts/tools/schemadump from the Go implementation. update_at values are wall-clock and differ per run.",
		"driver":     driver,
		"tables":     result,
	}, "", "  "))
	return string(buf) + "\n"
}

// dumpMongoDocs renders every document in canonical extended JSON so BSON
// types are explicit. _id values are generated by the server.
func dumpMongoDocs(client *mongo.Client, dbName string) string {
	ctx := context.Background()
	db := client.Database(dbName)
	names := must1(db.ListCollectionNames(ctx, bson.M{}))
	sort.Strings(names)
	var b strings.Builder
	b.WriteString("{\n  \"_generated\": \"contracts/tools/schemadump from the Go implementation (canonical extended JSON). _id and update_at differ per run.\",\n  \"collections\": {")
	for i, name := range names {
		cursor := must1(db.Collection(name).Find(ctx, bson.M{}))
		var docs []bson.Raw
		must(cursor.All(ctx, &docs))
		if i > 0 {
			b.WriteString(",")
		}
		fmt.Fprintf(&b, "\n    %q: [", name)
		for j, doc := range docs {
			if j > 0 {
				b.WriteString(",")
			}
			b.WriteString("\n      ")
			b.Write(must1(bson.MarshalExtJSON(doc, true, false)))
		}
		b.WriteString("\n    ]")
	}
	b.WriteString("\n  }\n}\n")
	return b.String()
}

// dumpRedis lists every key with its type and raw value.
func dumpRedis(client *redis.Client) string {
	ctx := context.Background()
	keys := must1(client.Keys(ctx, "*").Result())
	sort.Strings(keys)
	entries := []map[string]any{}
	for _, key := range keys {
		typ := must1(client.Type(ctx, key).Result())
		entry := map[string]any{"key": key, "type": typ}
		switch typ {
		case "string":
			entry["value"] = must1(client.Get(ctx, key).Result())
		case "hash":
			entry["value"] = must1(client.HGetAll(ctx, key).Result())
		case "TSDB-TYPE":
			entry["value"] = must1(client.Do(ctx, "TS.RANGE", key, "-", "+").Result())
			info := must1(client.Do(ctx, "TS.INFO", key).Result())
			entry["info"] = fmt.Sprint(info)
		default:
			entry["value"] = "(not dumped)"
		}
		entries = append(entries, entry)
	}
	buf := must1(json.MarshalIndent(map[string]any{
		"_generated": "contracts/tools/schemadump from the Go implementation (RESP2).",
		"keys":       entries,
	}, "", "  "))
	return string(buf) + "\n"
}

func writeFile(dir, name, content string) {
	must(os.WriteFile(filepath.Join(dir, name), []byte(content), 0o644))
	fmt.Println("wrote", filepath.Join(dir, name))
}

func must(err error) {
	if err != nil {
		panic(err)
	}
}

func must1[T any](v T, err error) T {
	must(err)
	return v
}

// dumpExisting writes the dump for -dump-only: the same text the full run writes
// for a store of this kind, read from a database that already exists.
func dumpExisting(kind, target, database, container string) string {
	switch kind {
	case "sqlite":
		return dumpSQLite(target)
	case "mysql":
		return dumpMySQL(target)
	case "postgres":
		return dumpPostgres(container, database)
	case "mongodb":
		client := must1(mongo.Connect(context.Background(), options.Client().ApplyURI(target)))
		defer func() { must(client.Disconnect(context.Background())) }()
		return dumpMongo(client, database)
	case "redis":
		opt := must1(redis.ParseURL(target))
		opt.Protocol = 2
		client := redis.NewClient(opt)
		defer func() { must(client.Close()) }()
		return ftInfo(context.Background(), client)
	default:
		panic("unknown -dump-only kind " + kind + " (want sqlite, mysql, postgres, mongodb or redis)")
	}
}

// ftInfo is the text of cache/redis.ft-info.txt: the RediSearch index that
// cache.Redis.Init creates, as FT.INFO returns it under RESP2.
func ftInfo(ctx context.Context, client *redis.Client) string {
	info := must1(client.Do(ctx, "FT.INFO", "documents").Result())
	return fmt.Sprintf("FT.INFO documents (RESP2, created by cache.Redis.Init)\n\n%v\n", info)
}

// dumpPostgres is dump-schemas.sh for one database: its header lines, then
// pg_dump -s inside the container, filtered and squeezed as the script filters
// it. The script's grep and cat -s are reproduced line by line.
func dumpPostgres(container, database string) string {
	version := strings.TrimRight(dockerOutput("exec", container, "postgres", "--version"), "\n")
	dump := dockerOutput("exec", container, "pg_dump", "-s", "--no-owner", "--no-privileges", "-U", "gorse", database)
	var b strings.Builder
	b.WriteString("-- Generated by contracts/tools/dump-schemas.sh (pg_dump -s) from the Go implementation. Do not edit.\n")
	fmt.Fprintf(&b, "-- Server: %s\n", version)
	if dump == "" {
		return b.String()
	}
	blank := false
	for _, line := range strings.Split(strings.TrimSuffix(dump, "\n"), "\n") {
		if pgNoise(line) {
			continue
		}
		if line == "" {
			if blank {
				continue
			}
			blank = true
		} else {
			blank = false
		}
		b.WriteString(line)
		b.WriteString("\n")
	}
	return b.String()
}

// pgNoise reports whether dump-schemas.sh's grep -v drops the line.
func pgNoise(line string) bool {
	return strings.HasPrefix(line, "SET ") ||
		strings.HasPrefix(line, "SELECT pg_catalog") ||
		line == "--" ||
		strings.HasPrefix(line, "-- Dumped") ||
		strings.HasPrefix(line, "-- PostgreSQL database dump") ||
		strings.Contains(line, "plpgsql") ||
		strings.HasPrefix(line, "-- Name: EXTENSION")
}

// dockerOutput runs docker with args and returns its standard output.
func dockerOutput(args ...string) string {
	out, err := exec.Command("docker", args...).Output()
	if err != nil {
		panic(fmt.Sprintf("docker %s: %v", strings.Join(args, " "), err))
	}
	return string(out)
}
