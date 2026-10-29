// Copyright 2026 gorse Project Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Command recommendgolden writes what the Go recommendation pipeline does on
// generated inputs, for the Rust port (WP-3-7):
//
//   - Each case seeds the rows of one user's recommendation run: items,
//     feedback, the vectors of the item-to-item and user-to-user collections,
//     the cached scores of the non-personalised, collaborative and ranker lists,
//     and their digests. The rows are written to SQLite and xvec stores in a
//     temporary directory, as the Go test suite opens them.
//   - Each case builds a logics.Recommender with a seeded config, runs it three
//     times, and records what each run gives: the scores, the digest, the
//     exclude set, the cold-start flag and the feedback order. A case is kept
//     only when three independent builds of its stores agree (stableCase).
//
// The Rust side is conformance/tests/recommend_oracle.rs. It reads the rows
// and the runs, builds the same stores, and compares its outputs with these.
// Usage:
//
//	go run ./contracts/tools/recommendgolden -out cases.json
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"math/rand"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"time"

	"github.com/gorse-io/gorse/common/expression"
	"github.com/gorse-io/gorse/config"
	"github.com/gorse-io/gorse/logics"
	"github.com/gorse-io/gorse/storage/cache"
	"github.com/gorse-io/gorse/storage/data"
	"github.com/gorse-io/gorse/storage/vectors"
)

// caseFile is the file the generator writes.
type caseFile struct {
	Seed  int64      `json:"seed"`
	Cases []testCase `json:"cases"`
}

// testCase is one generated run: the rows, the configuration, the runs and
// what Go gave for each run.
type testCase struct {
	Name       string       `json:"name"`
	UserID     string       `json:"user_id"`
	Online     bool         `json:"online"`
	Categories []string     `json:"categories"`
	Config     caseConfig   `json:"config"`
	Items      []caseItem   `json:"items"`
	Feedback   []caseFeed   `json:"feedback"`
	Vectors    []caseVecs   `json:"vectors"`
	Scores     []caseScores `json:"scores"`
	Digests    []caseDigest `json:"digests"`
	Runs       []caseRun    `json:"runs"`
	Expected   []outcome    `json:"expected"`
}

// caseConfig is the part of the recommend configuration the runs read.
type caseConfig struct {
	CacheSize      int         `json:"cache_size"`
	ContextSize    int         `json:"context_size"`
	ItemTTL        uint        `json:"item_ttl"`
	Positive       []string    `json:"positive"`
	Negative       []string    `json:"negative"`
	EnableReplace  bool        `json:"enable_replacement"`
	RankerType     string      `json:"ranker_type"`
	RankerNames    []string    `json:"ranker_recommenders"`
	FallbackNames  []string    `json:"fallback_recommenders"`
	ItemToItem     []caseNamed `json:"item_to_item"`
	UserToUser     []caseNamed `json:"user_to_user"`
	AvailableNames []string    `json:"available_names"`
}

// caseNamed is an item-to-item or user-to-user recommender: its name and its
// type.
type caseNamed struct {
	Name string `json:"name"`
	Type string `json:"type"`
}

// caseItem is one item row.
type caseItem struct {
	ID         string   `json:"id"`
	Categories []string `json:"categories"`
	Timestamp  int64    `json:"timestamp"`
	Hidden     bool     `json:"hidden"`
}

// caseFeed is one feedback row, with its time in Unix seconds.
type caseFeed struct {
	Type      string  `json:"type"`
	User      string  `json:"user"`
	Item      string  `json:"item"`
	Value     float64 `json:"value"`
	Timestamp int64   `json:"timestamp"`
}

// caseVecs is one vector collection, named as the Go package names it.
type caseVecs struct {
	Name     string    `json:"name"`
	Dense    bool      `json:"dense"`
	Distance string    `json:"distance"`
	Dim      int       `json:"dim"`
	Vectors  []caseVec `json:"vectors"`
}

// caseVec is one vector. A dense vector has values; a sparse one has indices
// and values.
type caseVec struct {
	ID         string    `json:"id"`
	Values     []float32 `json:"values"`
	Indices    []uint32  `json:"indices"`
	Categories []string  `json:"categories"`
	Hidden     bool      `json:"hidden"`
}

// caseScores is one cached score list: the collection, the subset and the
// scores.
type caseScores struct {
	Collection string      `json:"collection"`
	Subset     string      `json:"subset"`
	Items      []caseScore `json:"items"`
}

// caseScore is one cached score.
type caseScore struct {
	ID         string   `json:"id"`
	Score      float64  `json:"score"`
	Categories []string `json:"categories"`
}

// caseDigest is one digest: the parts of its key and its value.
type caseDigest struct {
	Parts []string `json:"parts"`
	Value string   `json:"value"`
}

// caseRun is one call: either Recommend ("recommend") or RecommendSequential
// ("sequential") with the limit and the names.
type caseRun struct {
	Op    string   `json:"op"`
	Limit int      `json:"limit"`
	Names []string `json:"names"`
}

// outcome is what one run gave. Error is the error text, empty when the run
// succeeded. A nil slice of results is recorded as Nil, and the results are
// then empty.
type outcome struct {
	Error         string   `json:"error"`
	Nil           bool     `json:"nil"`
	Results       []result `json:"results"`
	Digest        string   `json:"digest"`
	Exclude       []string `json:"exclude"`
	ColdStart     bool     `json:"cold_start"`
	FeedbackOrder []string `json:"feedback_order"`
}

// result is one recommended item.
type result struct {
	ID         string   `json:"id"`
	Score      float64  `json:"score"`
	Categories []string `json:"categories"`
}

var (
	categoryNames = []string{"cat-a", "cat-b"}
	feedbackTypes = []string{"click", "read", "dislike"}
)

func main() {
	out := flag.String("out", "", "the file to write the cases to")
	seed := flag.Int64("seed", 20261028, "the seed of the generated cases")
	count := flag.Int("cases", 40, "the number of cases")
	flag.Parse()
	if *out == "" {
		log.Fatal("-out is required")
	}

	rng := rand.New(rand.NewSource(*seed))
	now := time.Now()
	file := caseFile{Seed: *seed}
	for i := 0; i < *count; i++ {
		c := stableCase(rng, i, now)
		file.Cases = append(file.Cases, c)
	}

	f, err := os.Create(*out)
	if err != nil {
		log.Fatal(err)
	}
	defer f.Close()
	encoder := json.NewEncoder(f)
	if err := encoder.Encode(file); err != nil {
		log.Fatal(err)
	}
}

// maxAttempts bounds how many cases a generator draws for one index.
const maxAttempts = 10

// stableCase draws a case whose outcomes do not change between builds of its
// stores. Go's dense vector collections use HNSW, which is approximate and
// builds its graph with random levels, so a neighbour can be missed in one
// build and found in the next (PORTING_NOTES.md O-30). The case is built three
// times, and it is kept only when all three builds give the same outcomes. A
// case that never settles fails the generator, so the comparison is never
// made on an unstable reference.
func stableCase(rng *rand.Rand, index int, now time.Time) testCase {
	for attempt := 0; attempt < maxAttempts; attempt++ {
		c := generate(rng, index, now)
		first, err := run(&c)
		if err != nil {
			log.Fatalf("case %s: %v", c.Name, err)
		}
		stable := true
		for build := 0; build < 2 && stable; build++ {
			again, err := run(&c)
			if err != nil {
				log.Fatalf("case %s: %v", c.Name, err)
			}
			stable = reflect.DeepEqual(first, again)
		}
		if stable {
			c.Expected = first
			return c
		}
		log.Printf("case %02d attempt %d: the outcomes change between builds; drawing another case", index, attempt+1)
	}
	log.Fatalf("case %02d: no stable case in %d attempts", index, maxAttempts)
	return testCase{}
}

// generate builds one seeded case. The timestamps are relative to now, so a
// run's cut-off by the item TTL is taken at the same moment for both builds.
func generate(rng *rand.Rand, index int, now time.Time) testCase {
	c := testCase{Name: fmt.Sprintf("case-%02d", index), UserID: "user-0", Online: rng.Intn(2) == 0}
	c.Categories = pickCategories(rng)

	ago := func(maxDays int) int64 {
		return now.Add(-time.Duration(rng.Intn(maxDays*24*3600)) * time.Second).Unix()
	}
	numItems := 8 + rng.Intn(7)
	itemIDs := make([]string, numItems)
	for i := range itemIDs {
		itemIDs[i] = fmt.Sprintf("item-%d", i)
		item := caseItem{ID: itemIDs[i], Timestamp: ago(60), Hidden: rng.Float64() < 0.15}
		for _, cat := range categoryNames {
			if rng.Float64() < 0.4 {
				item.Categories = append(item.Categories, cat)
			}
		}
		c.Items = append(c.Items, item)
	}

	for u := 0; u < 4; u++ {
		user := fmt.Sprintf("user-%d", u)
		for _, item := range itemIDs {
			switch p := rng.Float64(); {
			case p < 0.3:
				c.Feedback = append(c.Feedback, caseFeed{Type: "click", User: user, Item: item, Timestamp: ago(30)})
			case p < 0.4:
				c.Feedback = append(c.Feedback, caseFeed{Type: "read", User: user, Item: item, Timestamp: ago(30)})
			case p < 0.48:
				c.Feedback = append(c.Feedback, caseFeed{Type: "dislike", User: user, Item: item, Timestamp: ago(30)})
			}
		}
	}

	c.Config = caseConfig{
		CacheSize:     []int{3, 5, 8}[rng.Intn(3)],
		ContextSize:   []int{1, 2, 4}[rng.Intn(3)],
		ItemTTL:       []uint{0, 30}[rng.Intn(2)],
		Positive:      []string{"click"},
		EnableReplace: rng.Intn(2) == 0,
		RankerType:    "none",
	}
	if rng.Intn(2) == 0 {
		c.Config.Positive = []string{"click", "read"}
	}
	if rng.Intn(2) == 0 {
		c.Config.Negative = []string{"dislike"}
	}
	if rng.Intn(10) < 3 {
		c.Config.RankerType = "fm"
	}

	// The item-to-item recommenders each have one kind, and so do the
	// user-to-user ones. Each keeps its own collection, named as Go does.
	for _, kind := range []struct{ name, kind string }{{"emb", "embedding"}, {"tags", "tags"}, {"users", "users"}, {"auto", "auto"}} {
		if kind.kind == "embedding" || rng.Float64() < 0.8 {
			c.Config.ItemToItem = append(c.Config.ItemToItem, caseNamed{Name: kind.name, Type: kind.kind})
			c.Vectors = append(c.Vectors, vectorsFor(rng, vectors.ItemToItemCollection(kind.name), kind.kind == "embedding", itemIDs, c.Items))
		}
	}
	userIDs := []string{"user-0", "user-1", "user-2", "user-3"}
	for _, kind := range []struct{ name, kind string }{{"uemb", "embedding"}, {"uitems", "items"}} {
		if kind.kind == "embedding" || rng.Float64() < 0.8 {
			c.Config.UserToUser = append(c.Config.UserToUser, caseNamed{Name: kind.name, Type: kind.kind})
			c.Vectors = append(c.Vectors, vectorsFor(rng, vectors.UserToUserCollection(kind.name), kind.kind == "embedding", userIDs, nil))
		}
	}

	c.Config.AvailableNames = []string{"latest", "collaborative", "non-personalized/pop"}
	for _, rec := range c.Config.ItemToItem {
		c.Config.AvailableNames = append(c.Config.AvailableNames, "item-to-item/"+rec.Name)
	}
	for _, rec := range c.Config.UserToUser {
		c.Config.AvailableNames = append(c.Config.AvailableNames, "user-to-user/"+rec.Name)
	}
	c.Config.RankerNames = pickNames(rng, c.Config.AvailableNames)
	c.Config.FallbackNames = pickNames(rng, c.Config.AvailableNames)

	// The cached lists: the ranker's, the non-personalised one and the
	// collaborative one. Each has a digest, which is sometimes missing, so a
	// run that reads it fails.
	subsetOf := func(all []string) []caseScore {
		var scores []caseScore
		for i, id := range all {
			if rng.Float64() < 0.8 {
				s := caseScore{ID: id, Score: rng.Float64() * float64(10+i)}
				if rng.Intn(2) == 0 {
					s.Categories = []string{""}
				}
				if rng.Intn(3) == 0 {
					s.Categories = append(s.Categories, categoryNames[rng.Intn(2)])
				}
				scores = append(scores, s)
			}
		}
		return scores
	}
	c.Scores = []caseScores{
		{Collection: cache.Recommend, Subset: c.UserID, Items: subsetOf(itemIDs)},
		{Collection: cache.NonPersonalized, Subset: "pop", Items: subsetOf(itemIDs)},
		{Collection: cache.CollaborativeFiltering, Subset: c.UserID, Items: subsetOf(itemIDs)},
	}
	c.Digests = []caseDigest{
		{Parts: []string{cache.NonPersonalizedDigest, "pop"}, Value: fmt.Sprintf("pop-%d", index)},
		{Parts: []string{cache.CollaborativeFilteringDigest, c.UserID}, Value: fmt.Sprintf("cf-%d", index)},
	}
	if rng.Intn(4) == 0 {
		c.Digests = c.Digests[1:]
	}

	// Three runs on one recommender: Recommend, RecommendSequential over a seeded
	// list of names (sometimes with an unknown one), and Recommend again.
	limits := []int{0, 2, 3, 5}
	c.Runs = []caseRun{
		{Op: "recommend", Limit: limits[rng.Intn(len(limits))]},
		{Op: "sequential", Limit: limits[rng.Intn(len(limits))], Names: pickNames(rng, c.Config.AvailableNames)},
		{Op: "recommend", Limit: limits[rng.Intn(len(limits))]},
	}
	if rng.Intn(10) == 0 {
		c.Runs[1].Names = append(c.Runs[1].Names, "unknown/recommender")
	}
	return c
}

// pickCategories is nil half the time, and one category otherwise.
func pickCategories(rng *rand.Rand) []string {
	switch rng.Intn(4) {
	case 0, 1:
		return nil
	default:
		return []string{categoryNames[rng.Intn(2)]}
	}
}

// pickNames is a seeded, non-empty subset of names, in a seeded order.
func pickNames(rng *rand.Rand, names []string) []string {
	perm := rng.Perm(len(names))
	n := 1 + rng.Intn(len(names))
	picked := make([]string, 0, n)
	for _, i := range perm[:n] {
		picked = append(picked, names[i])
	}
	return picked
}

// vectorsFor builds one collection. A dense collection holds three values per
// row. A sparse one holds one to three indices of a six-dimensional space,
// with positive values, so a dot product can be zero.
func vectorsFor(rng *rand.Rand, name string, dense bool, ids []string, items []caseItem) caseVecs {
	v := caseVecs{Name: name, Dense: dense, Distance: "dot"}
	if dense {
		v.Distance, v.Dim = "euclidean", 3
	}
	for i, id := range ids {
		if rng.Float64() < 0.1 {
			continue
		}
		row := caseVec{ID: id}
		if items != nil {
			row.Hidden = items[i].Hidden
			row.Categories = items[i].Categories
		}
		if dense {
			for d := 0; d < 3; d++ {
				row.Values = append(row.Values, float32(rng.Float64()*2-1))
			}
		} else {
			seen := map[uint32]bool{}
			for n := 1 + rng.Intn(3); len(row.Indices) < n; {
				idx := uint32(rng.Intn(6))
				if !seen[idx] {
					seen[idx] = true
					row.Indices = append(row.Indices, idx)
				}
			}
			sort.Slice(row.Indices, func(a, b int) bool { return row.Indices[a] < row.Indices[b] })
			for range row.Indices {
				row.Values = append(row.Values, float32(0.1+rng.Float64()*1.9))
			}
		}
		v.Vectors = append(v.Vectors, row)
	}
	return v
}

// run builds the stores of one case in a temporary directory, runs the
// recommender, and returns what each run gave.
func run(c *testCase) ([]outcome, error) {
	dir, err := os.MkdirTemp("", "recommendgolden")
	if err != nil {
		return nil, err
	}
	defer os.RemoveAll(dir)
	ctx := context.Background()

	dataClient, err := data.Open(fmt.Sprintf("sqlite://%s", filepath.Join(dir, "data.db")), "")
	if err != nil {
		return nil, err
	}
	defer dataClient.Close()
	cacheClient, err := cache.Open(fmt.Sprintf("sqlite://%s", filepath.Join(dir, "cache.db")), "")
	if err != nil {
		return nil, err
	}
	defer cacheClient.Close()
	vectorClient, err := vectors.Open(fmt.Sprintf("xvec://%s", filepath.Join(dir, "vectors")), "")
	if err != nil {
		return nil, err
	}
	defer vectorClient.Close()
	for _, init := range []func() error{dataClient.Init, cacheClient.Init, vectorClient.Init} {
		if err := init(); err != nil {
			return nil, err
		}
	}

	items := make([]data.Item, 0, len(c.Items))
	for _, item := range c.Items {
		var categories []string
		if item.Categories != nil {
			categories = item.Categories
		}
		items = append(items, data.Item{
			ItemId:     item.ID,
			IsHidden:   item.Hidden,
			Categories: categories,
			Timestamp:  time.Unix(item.Timestamp, 0),
		})
	}
	if err := dataClient.BatchInsertItems(ctx, items); err != nil {
		return nil, err
	}
	feedback := make([]data.Feedback, 0, len(c.Feedback))
	for _, f := range c.Feedback {
		feedback = append(feedback, data.Feedback{
			FeedbackType: f.Type,
			UserId:       f.User,
			ItemId:       f.Item,
			Value:        f.Value,
			Timestamp:    time.Unix(f.Timestamp, 0),
		})
	}
	if err := dataClient.BatchInsertFeedback(ctx, feedback, true, true, false); err != nil {
		return nil, err
	}

	for _, v := range c.Vectors {
		distance := vectors.Dot
		if v.Distance == "euclidean" {
			distance = vectors.Euclidean
		}
		if err := vectorClient.AddCollection(ctx, v.Name, v.Dim, distance, vectors.VectorConfig{}); err != nil {
			return nil, err
		}
		rows := make([]vectors.Vector, 0, len(v.Vectors))
		for _, row := range v.Vectors {
			rows = append(rows, vectors.Vector{
				Id:         row.ID,
				Values:     row.Values,
				Indices:    row.Indices,
				Categories: row.Categories,
				IsHidden:   row.Hidden,
			})
		}
		if err := vectorClient.AddVectors(ctx, v.Name, rows); err != nil {
			return nil, err
		}
	}

	for _, s := range c.Scores {
		scores := make([]cache.Score, 0, len(s.Items))
		for _, item := range s.Items {
			scores = append(scores, cache.Score{Id: item.ID, Score: item.Score, Categories: item.Categories})
		}
		if len(scores) == 0 {
			continue
		}
		if err := cacheClient.AddScores(ctx, s.Collection, s.Subset, scores); err != nil {
			return nil, err
		}
	}
	for _, d := range c.Digests {
		if err := cacheClient.Set(ctx, cache.String(cache.Key(d.Parts...), d.Value)); err != nil {
			return nil, err
		}
	}

	cfg := config.RecommendConfig{
		CacheSize:   c.Config.CacheSize,
		ContextSize: c.Config.ContextSize,
		DataSource: config.DataSourceConfig{
			ItemTTL: c.Config.ItemTTL,
		},
		Replacement: config.ReplacementConfig{EnableReplacement: c.Config.EnableReplace},
		Ranker:      config.RankerConfig{Type: c.Config.RankerType, Recommenders: c.Config.RankerNames},
		Fallback:    config.FallbackConfig{Recommenders: c.Config.FallbackNames},
	}
	for _, name := range c.Config.Positive {
		cfg.DataSource.PositiveFeedbackTypes = append(cfg.DataSource.PositiveFeedbackTypes, expression.FeedbackTypeExpression{FeedbackType: name})
	}
	for _, name := range c.Config.Negative {
		cfg.DataSource.NegativeFeedbackTypes = append(cfg.DataSource.NegativeFeedbackTypes, expression.FeedbackTypeExpression{FeedbackType: name})
	}
	for _, rec := range c.Config.ItemToItem {
		cfg.ItemToItem = append(cfg.ItemToItem, config.ItemToItemConfig{Name: rec.Name, Type: rec.Type})
	}
	for _, rec := range c.Config.UserToUser {
		cfg.UserToUser = append(cfg.UserToUser, config.UserToUserConfig{Name: rec.Name, Type: rec.Type})
	}

	recommender, err := logics.NewRecommender(cfg, cacheClient, dataClient, vectorClient, c.Online, c.UserID, c.Categories)
	if err != nil {
		return nil, err
	}
	var outcomes []outcome
	for _, r := range c.Runs {
		var scores []cache.Score
		var digest string
		var runErr error
		if r.Op == "recommend" {
			scores, runErr = recommender.Recommend(ctx, r.Limit)
		} else {
			scores, digest, runErr = recommender.RecommendSequential(ctx, nil, r.Limit, r.Names...)
		}
		outcomes = append(outcomes, outcomeOf(recommender, scores, digest, runErr))
	}
	return outcomes, nil
}

// outcomeOf records what a run gave, with the state of the recommender after it.
func outcomeOf(r *logics.Recommender, scores []cache.Score, digest string, err error) outcome {
	o := outcome{Digest: digest, Results: []result{}}
	if err != nil {
		o.Error = err.Error()
	}
	o.Nil = err == nil && scores == nil
	for _, s := range scores {
		categories := s.Categories
		if categories == nil {
			categories = []string{}
		}
		o.Results = append(o.Results, result{ID: s.Id, Score: s.Score, Categories: categories})
	}
	exclude := r.ExcludeSet().ToSlice()
	sort.Strings(exclude)
	o.Exclude = exclude
	o.ColdStart = r.IsColdStart()
	o.FeedbackOrder = []string{}
	for _, f := range r.UserFeedback() {
		o.FeedbackOrder = append(o.FeedbackOrder, f.ItemId)
	}
	return o
}
