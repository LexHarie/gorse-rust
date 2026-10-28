// Command proxygolden runs one scenario through Gorse's proxy stores, the
// data, cache and vectors ones (storage/{data,cache,vectors}/proxy.go at the
// pinned commit), and writes what each step returns as a transcript. The
// Rust port (WP-2-12, crates/gorse-storage/src/{data,cache,vectors}/proxy.rs)
// runs the same scenario in tests/common/proxy_scenario.rs, so the transcript
// is the oracle for wire compatibility in both directions:
//
//   - with no flags, Go's client and Go's server, in this process, write the
//     transcript (the golden file);
//   - -rust-server runs Go's client against the Rust server that the peer
//     example serves, and checks the transcript against the golden file;
//   - -rust-client serves Go's proxies here and checks the transcript that
//     the peer example's Rust client writes against the golden file.
//
// One line per step: the step's name, a tab, then {"ok":value} or
// {"err":text}. Records are written as maps, so their keys are sorted, and
// every error is the text Go's client shows.
//
// The Rust side is the proxy_peer binary of the conformance crate
// (conformance/src/bin/proxy_peer.rs). conformance/tests/proxy_wire.rs runs
// these modes under `cargo test -p gorse-conformance proxy`, so the checks need
// no manual steps. By hand, from the repository root:
//
//	go run ./contracts/tools/proxygolden \
//		-out crates/gorse-storage/tests/data/proxy_transcript.txt
//	cargo build -p gorse-conformance --bin proxy_peer
//	go run ./contracts/tools/proxygolden \
//		-rust-server target/debug/proxy_peer
//	go run ./contracts/tools/proxygolden \
//		-rust-client target/debug/proxy_peer
package main

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/gorse-io/gorse/config"
	"github.com/gorse-io/gorse/protocol"
	"github.com/gorse-io/gorse/storage/cache"
	"github.com/gorse-io/gorse/storage/data"
	"github.com/gorse-io/gorse/storage/vectors"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

// transcript collects one line per step.
type transcript struct {
	buf bytes.Buffer
}

// emit writes a step: its name, then its outcome as JSON. A non-nil error
// is {"err":text}; otherwise the value is {"ok":value}.
func (t *transcript) emit(name string, value any, err error) {
	var body any = map[string]any{"ok": value}
	if err != nil {
		body = map[string]any{"err": err.Error()}
	}
	line, mErr := marshal(body)
	if mErr != nil {
		log.Fatal(mErr)
	}
	t.buf.WriteString(name)
	t.buf.WriteByte('\t')
	t.buf.Write(line)
	t.buf.WriteByte('\n')
}

func (t *transcript) String() string {
	return t.buf.String()
}

// marshal is json.Marshal without HTML escaping, and without the trailing
// newline that json.Encoder adds.
func marshal(v any) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return nil, err
	}
	return bytes.TrimSuffix(buf.Bytes(), []byte("\n")), nil
}

func timeJSON(t time.Time) map[string]any {
	return map[string]any{"unix": t.Unix(), "nanos": t.Nanosecond()}
}

// nonNil writes a nil list as an empty one, for the lists a Rust Vec cannot
// tell from nil.
func nonNil[T any](xs []T) []T {
	if xs == nil {
		return []T{}
	}
	return xs
}

func itemJSON(item data.Item) map[string]any {
	return map[string]any{
		"ItemId":     item.ItemId,
		"IsHidden":   item.IsHidden,
		"Categories": item.Categories,
		"Timestamp":  timeJSON(item.Timestamp),
		"Labels":     item.Labels,
		"Comment":    item.Comment,
	}
}

func itemsJSON(items []data.Item) []any {
	out := make([]any, 0, len(items))
	for _, item := range items {
		out = append(out, itemJSON(item))
	}
	return out
}

// scoredItemJSON is a search result: the item's fields and its score.
func scoredItemJSON(s data.ScoredItem) map[string]any {
	out := itemJSON(s.Item)
	out["Score"] = s.Score
	return out
}

func scoredItemsJSON(items []data.ScoredItem) []any {
	out := make([]any, 0, len(items))
	for _, s := range items {
		out = append(out, scoredItemJSON(s))
	}
	return out
}

func userJSON(user data.User) map[string]any {
	return map[string]any{
		"UserId":  user.UserId,
		"Labels":  user.Labels,
		"Comment": user.Comment,
	}
}

func usersJSON(users []data.User) []any {
	out := make([]any, 0, len(users))
	for _, user := range users {
		out = append(out, userJSON(user))
	}
	return out
}

func feedbackJSON(f data.Feedback) map[string]any {
	return map[string]any{
		"FeedbackType": f.FeedbackType,
		"UserId":       f.UserId,
		"ItemId":       f.ItemId,
		"Value":        f.Value,
		"Timestamp":    timeJSON(f.Timestamp),
		"Updated":      timeJSON(f.Updated),
		"Labels":       f.Labels,
		"Comment":      f.Comment,
	}
}

func feedbackListJSON(feedback []data.Feedback) []any {
	out := make([]any, 0, len(feedback))
	for _, f := range feedback {
		out = append(out, feedbackJSON(f))
	}
	return out
}

// batches collects a stream the way the Rust scenario does: every batch in
// order, then the error the stream ends with, if any.
func collectItems(items chan []data.Item, errs chan error) (any, error) {
	var batches []any
	for batch := range items {
		batches = append(batches, itemsJSON(batch))
	}
	if err := <-errs; err != nil {
		return nil, err
	}
	return nonNil(batches), nil
}

func collectUsers(users chan []data.User, errs chan error) (any, error) {
	var batches []any
	for batch := range users {
		batches = append(batches, usersJSON(batch))
	}
	if err := <-errs; err != nil {
		return nil, err
	}
	return nonNil(batches), nil
}

func collectFeedback(feedback chan []data.Feedback, errs chan error) (any, error) {
	var batches []any
	for batch := range feedback {
		batches = append(batches, feedbackListJSON(batch))
	}
	if err := <-errs; err != nil {
		return nil, err
	}
	return nonNil(batches), nil
}

// dataSection is the data store: the methods the proxy serves, its refusals,
// and the streams.
func dataSection(t *transcript, d data.Database) {
	ctx := context.Background()
	t1 := time.Unix(1704164645, 600000000).UTC()
	t2 := time.Unix(1704164700, 0).UTC()
	t0 := time.Unix(1704000000, 0).UTC()

	t.emit("d.ping", nil, d.Ping())
	t.emit("d.init", nil, d.Init())
	t.emit("d.purge", nil, d.Purge())

	items := []data.Item{
		{ItemId: "i1", Categories: []string{"a", "b"}, Timestamp: t1, Labels: []any{"x", "y"}, Comment: "first"},
		{ItemId: "i2", IsHidden: true, Categories: []string{}, Timestamp: t2, Labels: map[string]any{"k": "v", "n": 2.5}, Comment: "second"},
	}
	t.emit("d.batch_insert_items", nil, d.BatchInsertItems(ctx, items))
	item, err := d.GetItem(ctx, "i1")
	t.emit("d.get_item_i1", itemJSON(item), err)
	_, err = d.GetItem(ctx, "missing")
	t.emit("d.get_item_missing", nil, err)
	batch, err := d.BatchGetItems(ctx, []string{"i1", "i2", "missing"}, data.GetOptions{SkipHidden: true})
	t.emit("d.batch_get_items", itemsJSON(batch), err)
	count, err := d.CountItems(ctx)
	t.emit("d.count_items", count, err)
	cursor, got, err := d.GetItems(ctx, "", 10, nil)
	t.emit("d.get_items", map[string]any{"cursor": cursor, "items": itemsJSON(got)}, err)
	err = d.ModifyItem(ctx, "i1", data.ItemPatch{IsHidden: ptr(true), Categories: []string{"z"}, Comment: ptr("changed")})
	t.emit("d.modify_item_i1", nil, err)
	item, err = d.GetItem(ctx, "i1")
	t.emit("d.get_item_i1_modified", itemJSON(item), err)
	err = d.ModifyItem(ctx, "missing", data.ItemPatch{Comment: ptr("x")})
	t.emit("d.modify_item_missing", nil, err)

	users := []data.User{
		{UserId: "u1", Labels: map[string]any{"tier": "gold"}, Comment: "vip"},
		{UserId: "u2"},
	}
	t.emit("d.batch_insert_users", nil, d.BatchInsertUsers(ctx, users))
	user, err := d.GetUser(ctx, "u1")
	t.emit("d.get_user_u1", userJSON(user), err)
	_, err = d.GetUser(ctx, "missing")
	t.emit("d.get_user_missing", nil, err)
	count, err = d.CountUsers(ctx)
	t.emit("d.count_users", count, err)
	cursor, gotUsers, err := d.GetUsers(ctx, "", 10)
	t.emit("d.get_users", map[string]any{"cursor": cursor, "users": usersJSON(gotUsers)}, err)
	err = d.ModifyUser(ctx, "u1", data.UserPatch{Comment: ptr("changed")})
	t.emit("d.modify_user_u1", nil, err)

	feedback := []data.Feedback{
		{FeedbackKey: data.FeedbackKey{FeedbackType: "like", UserId: "u1", ItemId: "i1"}, Value: 1.5, Timestamp: t1, Comment: "f1"},
		{FeedbackKey: data.FeedbackKey{FeedbackType: "read", UserId: "u1", ItemId: "i2"}, Value: 0.5, Timestamp: t2, Labels: []any{"seen"}},
		{FeedbackKey: data.FeedbackKey{FeedbackType: "like", UserId: "u2", ItemId: "i1"}, Value: 2.25, Timestamp: t0},
	}
	err = d.BatchInsertFeedback(ctx, feedback, true, true, false)
	t.emit("d.batch_insert_feedback", nil, err)
	itemFeedback, err := d.GetItemFeedback(ctx, "i1")
	t.emit("d.get_item_feedback_i1", feedbackListJSON(itemFeedback), err)
	userFeedback, err := d.GetUserFeedback(ctx, "u1", nil)
	t.emit("d.get_user_feedback_u1", feedbackListJSON(userFeedback), err)
	pair, err := d.GetUserItemFeedback(ctx, "u1", "i1")
	t.emit("d.get_user_item_feedback", feedbackListJSON(pair), err)
	count, err = d.CountFeedback(ctx)
	t.emit("d.count_feedback", count, err)
	cursor, gotFeedback, err := d.GetFeedback(ctx, "", 10, nil, nil)
	t.emit("d.get_feedback", map[string]any{"cursor": cursor, "feedback": feedbackListJSON(gotFeedback)}, err)

	userBatches, userErrs := d.GetUserStream(ctx, 1)
	streamedUsers, err := collectUsers(userBatches, userErrs)
	t.emit("d.get_user_stream", streamedUsers, err)
	itemBatches, itemErrs := d.GetItemStream(ctx, 1, nil)
	streamedItems, err := collectItems(itemBatches, itemErrs)
	t.emit("d.get_item_stream", streamedItems, err)
	feedbackBatches, feedbackErrs := d.GetFeedbackStream(ctx, 2)
	streamedFeedback, err := collectFeedback(feedbackBatches, feedbackErrs)
	t.emit("d.get_feedback_stream", streamedFeedback, err)

	t3 := time.Unix(1704168000, 0).UTC()
	searchable := data.Item{ItemId: "i3", Categories: []string{"a"}, Timestamp: t3, Labels: []any{"news"}, Comment: "searchable notes"}
	t.emit("d.batch_insert_item_i3", nil, d.BatchInsertItems(ctx, []data.Item{searchable}))
	found, err := d.SearchItems(ctx, "searchable", 10)
	t.emit("d.search_items_searchable", scoredItemsJSON(found), err)
	found, err = d.SearchItems(ctx, "changed", 10)
	t.emit("d.search_items_changed", scoredItemsJSON(found), err)
	found, err = d.SearchItems(ctx, "nothing", 10)
	t.emit("d.search_items_none", scoredItemsJSON(found), err)
	latest, err := d.GetLatestItems(ctx, 10, nil, nil)
	t.emit("d.get_latest_items", itemsJSON(latest), err)
	latest, err = d.GetLatestItems(ctx, 10, []string{"a"}, nil)
	t.emit("d.get_latest_items_category", itemsJSON(latest), err)
	latest, err = d.GetLatestItems(ctx, 10, nil, &t3)
	t.emit("d.get_latest_items_after", itemsJSON(latest), err)

	deleted, err := d.DeleteUserItemFeedback(ctx, "u2", "i1")
	t.emit("d.delete_user_item_feedback", deleted, err)
	t.emit("d.delete_item_i2", nil, d.DeleteItem(ctx, "i2"))
	_, err = d.GetItem(ctx, "i2")
	t.emit("d.get_item_i2_deleted", nil, err)
	t.emit("d.delete_user_u2", nil, d.DeleteUser(ctx, "u2"))
	count, err = d.CountFeedback(ctx)
	t.emit("d.count_feedback_after_delete", count, err)
}

// cacheSection is the cache store: values, scores, time series, and the
// refusals of the proxy client.
func cacheSection(t *transcript, c cache.Database) {
	ctx := context.Background()
	t1 := time.Unix(1704164645, 600000000).UTC()
	t2 := time.Unix(1704164700, 0).UTC()
	t3 := time.Unix(1704168000, 0).UTC()

	t.emit("c.ping", nil, c.Ping())
	t.emit("c.init", nil, c.Init())
	t.emit("c.purge", nil, c.Purge())
	t.emit("c.scan", nil, c.Scan(func(string) error { return nil }))

	t.emit("c.set", nil, c.Set(ctx,
		cache.String("a", "1"),
		cache.String("b", "x"),
		cache.Integer("n", 7),
		cache.Time("t", t1),
	))
	value, err := returnJSON(c.Get(ctx, "a"))
	t.emit("c.get_a", value, err)
	value, err = returnJSON(c.Get(ctx, "missing"))
	t.emit("c.get_missing", value, err)
	value, err = integerJSON(c.Get(ctx, "n"))
	t.emit("c.get_n", value, err)
	value, err = timeReturnJSON(c.Get(ctx, "t"))
	t.emit("c.get_t", value, err)
	t.emit("c.delete_b", nil, c.Delete(ctx, "b"))
	value, err = returnJSON(c.Get(ctx, "b"))
	t.emit("c.get_b_deleted", value, err)

	documents := []cache.Score{
		{Id: "x", Score: 1.5, Categories: []string{"g"}, Timestamp: t1},
		{Id: "y", Score: 2.25, Timestamp: t2},
		{Id: "z", Score: 0.5, IsHidden: true, Categories: []string{"g"}, Timestamp: t2},
	}
	t.emit("c.add_scores", nil, c.AddScores(ctx, "rec", "pop", documents))
	scores, err := c.SearchScores(ctx, "rec", "pop", nil, 0, 10)
	t.emit("c.search_scores", scoresJSON(scores), err)
	t.emit("c.update_scores", nil, c.UpdateScores(ctx, []string{"rec"}, ptr("pop"), "x", cache.ScorePatch{Score: ptr(3.75)}))
	scores, err = c.SearchScores(ctx, "rec", "pop", nil, 0, 10)
	t.emit("c.search_scores_updated", scoresJSON(scores), err)
	t.emit("c.delete_scores", nil, c.DeleteScores(ctx, []string{"rec"}, cache.ScoreCondition{Id: ptr("z")}))

	var scanned []any
	err = c.ScanScores(ctx, func(collection, id, subset string, timestamp time.Time) error {
		scanned = append(scanned, map[string]any{
			"collection": collection,
			"id":         id,
			"subset":     subset,
			"timestamp":  timeJSON(timestamp),
		})
		return nil
	})
	t.emit("c.scan_scores", nonNil(scanned), err)

	points := []cache.TimeSeriesPoint{
		{Name: "ts", Timestamp: t1, Value: 1.5},
		{Name: "ts", Timestamp: t2, Value: 2.25},
		{Name: "ts", Timestamp: t3, Value: 4.5},
	}
	t.emit("c.add_time_series", nil, c.AddTimeSeriesPoints(ctx, points))
	series, err := c.GetTimeSeriesPoints(ctx, "ts", t1, t3, time.Hour)
	t.emit("c.get_time_series", timeSeriesJSON(series), err)
}

func returnJSON(r *cache.ReturnValue) (any, error) {
	value, err := r.String()
	if err != nil {
		return nil, err
	}
	return map[string]any{"value": value, "exists": r.Exists()}, nil
}

func integerJSON(r *cache.ReturnValue) (any, error) {
	return r.Integer()
}

func timeReturnJSON(r *cache.ReturnValue) (any, error) {
	value, err := r.Time()
	if err != nil {
		return nil, err
	}
	return timeJSON(value), nil
}

func scoresJSON(scores []cache.Score) []any {
	out := make([]any, 0, len(scores))
	for _, s := range scores {
		out = append(out, map[string]any{
			"Id":         s.Id,
			"Score":      s.Score,
			"IsHidden":   s.IsHidden,
			"Categories": nonNil(s.Categories),
			"Timestamp":  timeJSON(s.Timestamp),
		})
	}
	return nonNil(out)
}

func timeSeriesJSON(points []cache.TimeSeriesPoint) []any {
	out := make([]any, 0, len(points))
	for _, p := range points {
		out = append(out, map[string]any{
			"Name":      p.Name,
			"Timestamp": timeJSON(p.Timestamp),
			"Value":     p.Value,
		})
	}
	return nonNil(out)
}

var distanceNames = map[vectors.Distance]string{
	vectors.Cosine:    "cosine",
	vectors.Euclidean: "euclidean",
	vectors.Dot:       "dot",
}

func vectorJSON(v vectors.Vector) map[string]any {
	return map[string]any{
		"Id":         v.Id,
		"Values":     nonNil(v.Values),
		"IsHidden":   v.IsHidden,
		"Categories": nonNil(v.Categories),
		"Timestamp":  timeJSON(v.Timestamp),
	}
}

// vectorsSection is the vector store, on xvec.
// vectorsSection runs the vector store's steps under names that start with
// prefix: "v" for the xvec store and "q" for the Qdrant store.
func vectorsSection(t *transcript, v vectors.Database, prefix string) {
	name := func(step string) string { return prefix + "." + step }
	ctx := context.Background()
	t1 := time.Unix(1704164645, 600000000).UTC()
	t2 := time.Unix(1704164700, 0).UTC()

	t.emit(name("init"), nil, v.Init())
	t.emit(name("add_collection"), nil, v.AddCollection(ctx, "docs", 2, vectors.Dot, vectors.VectorConfig{}))
	names, err := v.ListCollections(ctx)
	t.emit(name("list_collections"), nonNil(names), err)
	info, err := v.DescribeCollection(ctx, "docs")
	t.emit(name("describe_docs"), collectionJSON(info), err)
	_, err = v.DescribeCollection(ctx, "missing")
	t.emit(name("describe_missing"), nil, err)

	vecs := []vectors.Vector{
		{Id: "a", Values: []float32{1.5, 0.5}, Categories: []string{"c"}, Timestamp: t1},
		{Id: "b", Values: []float32{0.5, 2.5}, Timestamp: t2},
		{Id: "c", Values: []float32{3.5, 1.5}, IsHidden: true, Timestamp: t2},
	}
	t.emit(name("add_vectors"), nil, v.AddVectors(ctx, "docs", vecs))
	count, err := v.CountVectors(ctx, "docs")
	t.emit(name("count_vectors"), count, err)
	got, err := v.GetVectors(ctx, "docs", []string{"a", "c", "missing"})
	out := make([]any, 0, len(got))
	for _, g := range got {
		out = append(out, vectorJSON(g))
	}
	t.emit(name("get_vectors"), nonNil(out), err)

	results, err := v.QueryVectors(ctx, "docs", vectors.Vector{Values: []float32{1.5, 0.5}}, nil, 2)
	scored := make([]any, 0, len(results))
	for _, r := range results {
		scored = append(scored, map[string]any{"Id": r.Id, "Score": r.Score})
	}
	t.emit(name("query_vectors"), nonNil(scored), err)

	t.emit(name("delete_vectors"), nil, v.DeleteVectors(ctx, "docs", t2))
	count, err = v.CountVectors(ctx, "docs")
	t.emit(name("count_vectors_after_delete"), count, err)
	t.emit(name("optimize"), nil, v.Optimize(ctx, "docs"))
	t.emit(name("delete_collection"), nil, v.DeleteCollection(ctx, "docs"))
	names, err = v.ListCollections(ctx)
	t.emit(name("list_collections_after_delete"), nonNil(names), err)
	t.emit(name("close"), nil, v.Close())
}

func collectionJSON(info *vectors.CollectionInfo) map[string]any {
	return map[string]any{
		"Name":      info.Name,
		"Dimension": info.Dimension,
		"Distance":  distanceNames[info.Distance],
		"Type":      string(info.Type),
		"Bits":      info.Bits,
	}
}

func ptr[T any](v T) *T {
	return &v
}

// searchColumns are the search columns of Go's TestSearch
// (storage/data/database_test.go). Search needs them reconciled before it runs.
var searchColumns = []string{"item.ItemId", "item.Categories", "item.Labels.brand", "item.Comment"}

// serveGo starts Go's proxy servers on loopback ports, with a fresh backend of
// each store under dir: the data, cache and xvec stores on one address, and the
// Qdrant store on another, since one gRPC server holds one vector service. It
// returns both addresses and a stop func. The Qdrant store is a live server at
// qdrantURI, so its "docs" collection is deleted first if an earlier run left it.
func serveGo(dir, qdrantURI string) (string, string, func(), error) {
	database, err := data.Open("sqlite://"+filepath.Join(dir, "data.db"), "gorse_")
	if err != nil {
		return "", "", nil, err
	}
	if err := database.Init(); err != nil {
		return "", "", nil, err
	}
	if err := database.Reconcile(config.SearchConfig{Columns: searchColumns}); err != nil {
		return "", "", nil, err
	}
	store, err := cache.Open("sqlite://"+filepath.Join(dir, "cache.db"), "gorse_")
	if err != nil {
		return "", "", nil, err
	}
	if err := store.Init(); err != nil {
		return "", "", nil, err
	}
	index, err := vectors.Open("xvec://"+filepath.Join(dir, "xvec"), "gorse_")
	if err != nil {
		return "", "", nil, err
	}
	if err := index.Init(); err != nil {
		return "", "", nil, err
	}
	live, err := vectors.Open(qdrantURI, "gorse_")
	if err != nil {
		return "", "", nil, err
	}
	if err := live.Init(); err != nil {
		return "", "", nil, err
	}
	if err := purgeAll(live); err != nil {
		return "", "", nil, err
	}
	main := grpc.NewServer()
	protocol.RegisterDataStoreServer(main, data.NewProxyServer(database))
	protocol.RegisterCacheStoreServer(main, cache.NewProxyServer(store))
	protocol.RegisterVectorStoreServer(main, vectors.NewProxyServer(index))
	qdrant := grpc.NewServer()
	protocol.RegisterVectorStoreServer(qdrant, vectors.NewProxyServer(live))
	lis, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return "", "", nil, err
	}
	qdrantLis, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		lis.Close()
		return "", "", nil, err
	}
	go func() {
		if err := main.Serve(lis); err != nil {
			log.Print(err)
		}
	}()
	go func() {
		if err := qdrant.Serve(qdrantLis); err != nil {
			log.Print(err)
		}
	}()
	stop := func() {
		main.Stop()
		qdrant.Stop()
	}
	return lis.Addr().String(), qdrantLis.Addr().String(), stop, nil
}

// purgeAll deletes every collection on the live server, as Go's SetupTest does
// before each vector case. The vector suite leaves its last collection behind,
// and the scenario's list_collections step expects the server to hold "docs"
// alone, so a run that stopped early or a suite run before it must not leak in.
func purgeAll(db vectors.Database) error {
	ctx := context.Background()
	names, err := db.ListCollections(ctx)
	if err != nil {
		return err
	}
	for _, name := range names {
		if err := db.DeleteCollection(ctx, name); err != nil {
			return err
		}
	}
	return nil
}

// runGoClient runs the scenario through Go's proxy clients: the data, cache and
// xvec stores on addr, and the Qdrant store on qdrantAddr.
func runGoClient(addr, qdrantAddr string) (string, error) {
	conn, err := grpc.NewClient(addr, grpc.WithTransportCredentials(insecure.NewCredentials()))
	if err != nil {
		return "", err
	}
	defer conn.Close()
	qdrantConn, err := grpc.NewClient(qdrantAddr, grpc.WithTransportCredentials(insecure.NewCredentials()))
	if err != nil {
		return "", err
	}
	defer qdrantConn.Close()
	t := &transcript{}
	dataSection(t, data.NewProxyClient(conn))
	cacheSection(t, cache.NewProxyClient(conn))
	vectorsSection(t, vectors.NewProxyClient(conn), "v")
	vectorsSection(t, vectors.NewProxyClient(qdrantConn), "q")
	return t.String(), nil
}

// oracle runs the scenario through Go's client and server in this process.
func oracle(qdrantURI string) (string, error) {
	dir, err := os.MkdirTemp("", "proxygolden")
	if err != nil {
		return "", err
	}
	defer os.RemoveAll(dir)
	addr, qdrantAddr, stop, err := serveGo(dir, qdrantURI)
	if err != nil {
		return "", err
	}
	defer stop()
	return runGoClient(addr, qdrantAddr)
}

// checkAgainst compares got with the golden transcript, line by line.
func checkAgainst(got, golden string) error {
	gotLines := strings.Split(got, "\n")
	goldenLines := strings.Split(golden, "\n")
	for i := 0; i < len(gotLines) || i < len(goldenLines); i++ {
		var g, w string
		if i < len(gotLines) {
			g = gotLines[i]
		}
		if i < len(goldenLines) {
			w = goldenLines[i]
		}
		if g != w {
			return fmt.Errorf("line %d differs\n  golden: %s\n  got:    %s", i+1, w, g)
		}
	}
	return nil
}

// checkRustServer runs Go's client against the Rust proxy servers that the
// peer serves, until its stdin closes. The peer reports two addresses: one for
// the data, cache and xvec stores, and one for the Qdrant store.
func checkRustServer(peer, qdrantURI, golden string) error {
	cmd := exec.Command(peer, "serve", qdrantURI)
	cmd.Stderr = os.Stderr
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	if err := cmd.Start(); err != nil {
		return err
	}
	reader := bufio.NewReader(stdout)
	addr, err := readReported(reader, "listening ")
	if err != nil {
		stdin.Close()
		cmd.Wait()
		return err
	}
	qdrantAddr, err := readReported(reader, "listening-qdrant ")
	if err != nil {
		stdin.Close()
		cmd.Wait()
		return err
	}
	got, err := runGoClient(addr, qdrantAddr)
	stdin.Close()
	if waitErr := cmd.Wait(); waitErr != nil && err == nil {
		err = waitErr
	}
	if err != nil {
		return err
	}
	return checkAgainst(got, golden)
}

// readReported reads the peer's next line, which must start with prefix, and
// returns the address after it.
func readReported(reader *bufio.Reader, prefix string) (string, error) {
	line, err := reader.ReadString('\n')
	if err != nil {
		return "", fmt.Errorf("peer did not report its address: %w", err)
	}
	addr, ok := strings.CutPrefix(strings.TrimSpace(line), prefix)
	if !ok {
		return "", fmt.Errorf("peer reported %q, want a line starting %q", line, prefix)
	}
	return addr, nil
}

// checkRustClient serves Go's proxies, and runs the peer's Rust client against
// them.
func checkRustClient(peer, qdrantURI, golden string) error {
	dir, err := os.MkdirTemp("", "proxygolden")
	if err != nil {
		return err
	}
	defer os.RemoveAll(dir)
	addr, qdrantAddr, stop, err := serveGo(dir, qdrantURI)
	if err != nil {
		return err
	}
	defer stop()
	cmd := exec.Command(peer, "client", addr, qdrantAddr)
	cmd.Stderr = os.Stderr
	out, err := cmd.Output()
	if err != nil {
		return err
	}
	return checkAgainst(string(out), golden)
}

func main() {
	out := flag.String("out", "crates/gorse-storage/tests/data/proxy_transcript.txt", "where the oracle writes the transcript")
	golden := flag.String("golden", "crates/gorse-storage/tests/data/proxy_transcript.txt", "the transcript the checks compare with")
	rustServer := flag.String("rust-server", "", "the proxy_peer binary, serving the Rust proxies")
	rustClient := flag.String("rust-client", "", "the proxy_peer binary, running the Rust proxy clients")
	qdrant := flag.String("qdrant", os.Getenv("QDRANT_URI"), "a live Qdrant server, such as qdrant://127.0.0.1:6334 (defaults to $QDRANT_URI)")
	flag.Parse()

	if *qdrant == "" {
		log.Fatal("no Qdrant server: set QDRANT_URI or pass -qdrant (the scenario's vector section runs on a live Qdrant)")
	}
	if *rustServer != "" || *rustClient != "" {
		want, err := os.ReadFile(*golden)
		if err != nil {
			log.Fatal(err)
		}
		if *rustServer != "" {
			if err := checkRustServer(*rustServer, *qdrant, string(want)); err != nil {
				log.Fatalf("Go client against Rust server: %v", err)
			}
			fmt.Println("Go client against Rust server: transcript matches")
		}
		if *rustClient != "" {
			if err := checkRustClient(*rustClient, *qdrant, string(want)); err != nil {
				log.Fatalf("Rust client against Go server: %v", err)
			}
			fmt.Println("Rust client against Go server: transcript matches")
		}
		return
	}

	got, err := oracle(*qdrant)
	if err != nil {
		log.Fatal(err)
	}
	if err := os.WriteFile(*out, []byte(got), 0o644); err != nil {
		log.Fatal(err)
	}
	fmt.Printf("wrote %d steps to %s\n", strings.Count(got, "\n"), *out)
}
