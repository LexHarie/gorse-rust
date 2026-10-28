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

// Command item2itemgolden writes what the Go item-to-item and user-to-user
// recommenders do on generated inputs, for the Rust port (WP-3-5):
//
//   - Each case builds the recommender with logics.NewItemToItem or
//     logics.NewUserToUser on an xvec store in a temporary directory, adds the
//     rows with their labels in the form dataset.AddItem gives them (label
//     strings become dataset.ID, numeric lists become FP16 bits), and cleans.
//   - The output holds the vectors the store has for each row, and the
//     neighbours that logics.QueryItemToItem or logics.QueryUserToUser gives for
//     each row. Floats are written as their bits.
//
// The Rust side is conformance/tests/item_to_item_oracle.rs. Usage:
//
//	go run ./contracts/tools/item2itemgolden -out cases.json
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"log"
	"math"
	"math/rand"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/gorse-io/gorse/common/floats"
	"github.com/gorse-io/gorse/config"
	"github.com/gorse-io/gorse/dataset"
	"github.com/gorse-io/gorse/logics"
	"github.com/gorse-io/gorse/storage"
	"github.com/gorse-io/gorse/storage/cache"
	"github.com/gorse-io/gorse/storage/data"
	"github.com/gorse-io/gorse/storage/vectors"
)

// The recommender kinds, one per Go constructor: item-to-item and
// user-to-user, each with its types.
var kinds = []string{
	"item-embedding", "item-tags", "item-users", "item-auto",
	"user-embedding", "user-tags", "user-items", "user-auto",
}

// The number of cases per kind, and the rows and neighbours per case.
const (
	casesPerKind = 5
	rowsPerCase  = 30
	neighbours   = 5
)

// The column expressions each kind is built with. The kinds without a column
// take none, as Go's configs leave them.
var columns = map[string][]string{
	"item-embedding": {"item.Labels.embedding", "item.Labels.embedding[1:]", "item.Labels.description"},
	"item-tags":      {"item.Labels.tags", "item.Labels"},
	"item-users":     {""},
	"item-auto":      {""},
	"user-embedding": {"user.Labels.embedding", "user.Labels.embedding[:3]", "user.Labels.description"},
	"user-tags":      {"user.Labels.tags", "user.Labels"},
	"user-items":     {""},
	"user-auto":      {""},
}

// genRow is a row before it is written: its labels are the Go values the
// recommender receives.
type genRow struct {
	ID         string
	Hidden     bool
	Categories []string
	Labels     any
	Feedback   []int32
}

// jsonRow is the row as the Rust side reads it. Labels are tagged, see
// encodeLabels.
type jsonRow struct {
	ID         string   `json:"id"`
	Hidden     bool     `json:"hidden"`
	Categories []string `json:"categories"`
	Labels     any      `json:"labels"`
	Feedback   []int32  `json:"feedback"`
}

// oracleCase is one case: its inputs, then the outputs Go gives for them.
type oracleCase struct {
	Name      string         `json:"name"`
	Kind      string         `json:"kind"`
	Column    string         `json:"column"`
	TagsIDF   []uint32       `json:"tags_idf,omitempty"`
	UsersIDF  []uint32       `json:"users_idf,omitempty"`
	ItemsIDF  []uint32       `json:"items_idf,omitempty"`
	Rows      []jsonRow      `json:"rows"`
	Vectors   []vectorOut    `json:"vectors"`
	Neighbors []neighboursOf `json:"neighbors"`
	// Missing is set when no row wrote a vector, so the collection was never
	// created and Go's GetVectors fails with storage.ErrNotFound.
	Missing bool `json:"missing,omitempty"`
}

// vectorOut is a vector the store holds: its values as FP32 bits, and its
// indices.
type vectorOut struct {
	ID         string   `json:"id"`
	Values     []uint32 `json:"values"`
	Indices    []uint32 `json:"indices"`
	Categories []string `json:"categories"`
}

// neighboursOf is the neighbours of one row.
type neighboursOf struct {
	ID     string     `json:"id"`
	Scores []scoreOut `json:"scores"`
}

// scoreOut is a neighbour and its score as FP64 bits.
type scoreOut struct {
	ID    string `json:"id"`
	Score uint64 `json:"score"`
}

// encodeLabels writes a processed label as a tagged JSON value, so that the
// Rust side rebuilds the same type: "nil", "bool", "f64", "id", "ids", "fp16",
// "any", "string" or "map".
func encodeLabels(v any) map[string]any {
	switch typed := v.(type) {
	case nil:
		return map[string]any{"t": "nil"}
	case bool:
		return map[string]any{"t": "bool", "v": typed}
	case float64:
		return map[string]any{"t": "f64", "v": typed}
	case string:
		return map[string]any{"t": "string", "v": typed}
	case dataset.ID:
		return map[string]any{"t": "id", "v": int32(typed)}
	case []dataset.ID:
		ids := make([]int32, len(typed))
		for i, id := range typed {
			ids[i] = int32(id)
		}
		return map[string]any{"t": "ids", "v": ids}
	case []uint16:
		return map[string]any{"t": "fp16", "v": typed}
	case []any:
		items := make([]map[string]any, len(typed))
		for i, item := range typed {
			items[i] = encodeLabels(item)
		}
		return map[string]any{"t": "any", "v": items}
	case map[string]any:
		entries := make(map[string]map[string]any, len(typed))
		for name, value := range typed {
			entries[name] = encodeLabels(value)
		}
		return map[string]any{"t": "map", "v": entries}
	default:
		panic(fmt.Sprintf("no encoding for %T", v))
	}
}

// genValue draws a processed label of the kinds dataset.AddItem produces. A
// list of strings is not drawn, since processLabels has already made it IDs.
func genValue(r *rand.Rand, depth int) any {
	switch r.Intn(8) {
	case 0:
		return nil
	case 1:
		return r.Intn(2) == 0
	case 2:
		return float64(r.Intn(9)) / 4
	case 3:
		return dataset.ID(1 + r.Intn(40))
	case 4:
		ids := make([]dataset.ID, 1+r.Intn(3))
		for i := range ids {
			ids[i] = dataset.ID(1 + r.Intn(40))
		}
		return ids
	case 5:
		return floats.FromFloat32(genFloats(r, 3))
	case 6:
		items := make([]any, 1+r.Intn(3))
		for i := range items {
			switch r.Intn(3) {
			case 0:
				items[i] = nil
			case 1:
				items[i] = float64(r.Intn(9)) / 4
			default:
				items[i] = "text" + strconv.Itoa(r.Intn(3))
			}
		}
		return items
	default:
		if depth >= 2 {
			return dataset.ID(1 + r.Intn(40))
		}
		entries := make(map[string]any)
		for i := 0; i < 1+r.Intn(2); i++ {
			entries[[]string{"a", "b", "tags"}[r.Intn(3)]] = genValue(r, depth+1)
		}
		return entries
	}
}

// genFloats draws n values in [-1, 1).
func genFloats(r *rand.Rand, n int) []float32 {
	values := make([]float32, n)
	for i := range values {
		values[i] = float32(2*r.Float64() - 1)
	}
	return values
}

// genLabels draws the labels of a row: usually a map that has an embedding and
// tags, sometimes a single value.
func genLabels(r *rand.Rand) any {
	if r.Intn(5) == 0 {
		return genValue(r, 0)
	}
	labels := make(map[string]any)
	if r.Intn(10) < 8 {
		labels["embedding"] = floats.FromFloat32(genFloats(r, 4))
	}
	if r.Intn(10) < 8 {
		ids := make([]dataset.ID, 1+r.Intn(4))
		for i := range ids {
			ids[i] = dataset.ID(1 + r.Intn(40))
		}
		labels["tags"] = ids
	}
	if r.Intn(3) == 0 {
		labels["description"] = genValue(r, 1)
	}
	return labels
}

// genIDF draws n IDF values in [0.5, 3). They are not integers, so that the
// scores have few ties.
func genIDF(r *rand.Rand, n int) []uint32 {
	idf := make([]uint32, n)
	for i := range idf {
		idf[i] = math.Float32bits(float32(0.5 + 2.5*r.Float64()))
	}
	return idf
}

// genRows draws the rows of a case. An item row is a user's feedback, and a
// user row is an item's feedback.
func genRows(r *rand.Rand, kind string) []genRow {
	rows := make([]genRow, rowsPerCase)
	for i := range rows {
		row := genRow{ID: strconv.Itoa(i), Labels: genLabels(r)}
		row.Hidden = r.Intn(5) == 0
		if r.Intn(3) == 0 {
			row.Categories = []string{"c" + strconv.Itoa(r.Intn(2))}
		}
		// A user row's feedback is over the 40 items, an item row's over the 20 users.
		pool := 20
		if strings.HasPrefix(kind, "user-") {
			pool = 40
		}
		count := r.Intn(pool + 1)
		for _, id := range r.Perm(pool)[:count] {
			row.Feedback = append(row.Feedback, int32(1+id))
		}
		rows[i] = row
	}
	return rows
}

// bitsOf writes the FP32 bits of values.
func bitsOf(values []float32) []uint32 {
	bits := make([]uint32, len(values))
	for i, value := range values {
		bits[i] = math.Float32bits(value)
	}
	return bits
}

// floatsOf reads FP32 values from their bits.
func floatsOf(bits []uint32) []float32 {
	values := make([]float32, len(bits))
	for i, value := range bits {
		values[i] = math.Float32frombits(value)
	}
	return values
}

// runCase builds the recommender of the case on a new store, adds the rows,
// cleans, and reads back what the store holds and what the neighbours are.
func runCase(c *oracleCase, rows []genRow) error {
	dir, err := os.MkdirTemp("", "item2item")
	if err != nil {
		return err
	}
	defer os.RemoveAll(dir)
	ctx := context.Background()
	db, err := vectors.Open(fmt.Sprintf("xvec://%s", filepath.Join(dir, "vectors")), "")
	if err != nil {
		return err
	}
	if err := db.Init(); err != nil {
		return err
	}
	defer db.Close()
	timestamp := time.Now()
	const name = "oracle"

	var ids []string
	for _, row := range rows {
		ids = append(ids, row.ID)
	}
	if strings.HasPrefix(c.Kind, "item-") {
		cfg := config.ItemToItemConfig{Name: name, Type: strings.TrimPrefix(c.Kind, "item-"), Column: c.Column}
		rec, err := logics.NewItemToItem(cfg, timestamp, &logics.ItemToItemOptions{
			Context:      ctx,
			VectorClient: db,
			TagsIDF:      floatsOf(c.TagsIDF),
			UsersIDF:     floatsOf(c.UsersIDF),
		})
		if err != nil {
			return err
		}
		for _, row := range rows {
			item := &data.Item{ItemId: row.ID, IsHidden: row.Hidden, Categories: row.Categories, Labels: row.Labels}
			if err := rec.Add(item, row.Feedback); err != nil {
				return err
			}
		}
		if err := rec.Clean(); err != nil {
			return err
		}
		stored, err := db.GetVectors(ctx, vectors.ItemToItemCollection(name), ids)
		if errors.Is(err, storage.ErrNotFound) {
			c.Missing = true
			return nil
		}
		if err != nil {
			return err
		}
		c.Vectors = vectorsOf(stored)
		for _, id := range ids {
			scores, err := logics.QueryItemToItem(ctx, db, cfg, id, nil, neighbours)
			if err != nil {
				return err
			}
			c.Neighbors = append(c.Neighbors, neighboursOfScores(id, scores))
		}
		return nil
	}
	cfg := config.UserToUserConfig{Name: name, Type: strings.TrimPrefix(c.Kind, "user-"), Column: c.Column}
	rec, err := logics.NewUserToUser(cfg, timestamp, &logics.UserToUserOptions{
		Context:      ctx,
		VectorClient: db,
		TagsIDF:      floatsOf(c.TagsIDF),
		ItemsIDF:     floatsOf(c.ItemsIDF),
	})
	if err != nil {
		return err
	}
	for _, row := range rows {
		user := &data.User{UserId: row.ID, Labels: row.Labels}
		if err := rec.Add(user, row.Feedback); err != nil {
			return err
		}
	}
	if err := rec.Clean(); err != nil {
		return err
	}
	stored, err := db.GetVectors(ctx, vectors.UserToUserCollection(name), ids)
	if errors.Is(err, storage.ErrNotFound) {
		c.Missing = true
		return nil
	}
	if err != nil {
		return err
	}
	c.Vectors = vectorsOf(stored)
	for _, id := range ids {
		scores, err := logics.QueryUserToUser(ctx, db, cfg, id, neighbours)
		if err != nil {
			return err
		}
		c.Neighbors = append(c.Neighbors, neighboursOfScores(id, scores))
	}
	return nil
}

// vectorsOf writes the stored vectors in the order the store returns them.
func vectorsOf(stored []vectors.Vector) []vectorOut {
	out := make([]vectorOut, 0, len(stored))
	for _, vector := range stored {
		out = append(out, vectorOut{
			ID:         vector.Id,
			Values:     bitsOf(vector.Float32Values()),
			Indices:    vector.Indices,
			Categories: vector.Categories,
		})
	}
	return out
}

// neighboursOfScores writes the neighbours of the row id, in the order Go
// returns them.
func neighboursOfScores(id string, scores []cache.Score) neighboursOf {
	out := neighboursOf{ID: id}
	for _, score := range scores {
		out.Scores = append(out.Scores, scoreOut{ID: score.Id, Score: math.Float64bits(score.Score)})
	}
	return out
}

func main() {
	out := flag.String("out", "", "the file to write the cases to")
	flag.Parse()
	if *out == "" {
		log.Fatal("-out is required")
	}
	var cases []oracleCase
	for k, kind := range kinds {
		for n := range casesPerKind {
			r := rand.New(rand.NewSource(int64(20261028 + 100*k + n)))
			c := oracleCase{Name: fmt.Sprintf("%s-%d", kind, n), Kind: kind}
			cols := columns[kind]
			c.Column = cols[r.Intn(len(cols))]
			rows := genRows(r, kind)
			if strings.HasPrefix(kind, "item-") {
				c.TagsIDF = genIDF(r, 41)
				c.UsersIDF = genIDF(r, 21)
			} else {
				c.TagsIDF = genIDF(r, 41)
				c.ItemsIDF = genIDF(r, 41)
			}
			for _, row := range rows {
				c.Rows = append(c.Rows, jsonRow{ID: row.ID, Hidden: row.Hidden, Categories: row.Categories, Labels: encodeLabels(row.Labels), Feedback: row.Feedback})
			}
			if err := runCase(&c, rows); err != nil {
				log.Fatalf("case %s: %v", c.Name, err)
			}
			cases = append(cases, c)
		}
	}
	encoded, err := json.Marshal(cases)
	if err != nil {
		log.Fatal(err)
	}
	if err := os.WriteFile(*out, encoded, 0o644); err != nil {
		log.Fatal(err)
	}
}
