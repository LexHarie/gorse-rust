// Command xvecsparsegolden writes the Go scores of sparse dot-product queries
// for the Rust xvec port's regression test (O-87). The Rust test is
// crates/gorse-storage/tests/data/xvec_sparse_golden.json's reader, in
// src/vectors/xvec.rs (sparse_dot_matches_go_bits).
//
// Usage: go run ./contracts/tools/xvecsparsegolden -out <file>
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"math"
	"math/rand"
	"os"
	"sort"
	"time"

	"github.com/gorse-io/gorse/storage/vectors"
)

type doc struct {
	ID      string    `json:"id"`
	Indices []uint32  `json:"indices"`
	Values  []float32 `json:"values"`
}

type score struct {
	Doc  string `json:"doc"`
	Bits uint32 `json:"bits"`
}

type query struct {
	Indices []uint32  `json:"indices"`
	Values  []float32 `json:"values"`
	Scores  []score   `json:"scores"`
}

// sparse draws a canonical sparse vector: 1 to dims distinct indices, sorted,
// with values in [0, 1).
func sparse(rng *rand.Rand, dims int) ([]uint32, []float32) {
	n := 1 + rng.Intn(dims)
	seen := map[uint32]bool{}
	var indices []uint32
	for len(indices) < n {
		i := uint32(rng.Intn(dims))
		if !seen[i] {
			seen[i] = true
			indices = append(indices, i)
		}
	}
	sort.Slice(indices, func(a, b int) bool { return indices[a] < indices[b] })
	values := make([]float32, len(indices))
	for i := range values {
		values[i] = float32(rng.Float64())
	}
	return indices, values
}

func main() {
	out := flag.String("out", "", "the file to write")
	flag.Parse()
	if *out == "" {
		log.Fatal("-out is required")
	}
	ctx := context.Background()
	rng := rand.New(rand.NewSource(87))
	dir, err := os.MkdirTemp("", "xvecsparsegolden")
	if err != nil {
		log.Fatal(err)
	}
	defer os.RemoveAll(dir)
	db, err := vectors.Open("xvec://"+dir, "gorse_")
	if err != nil {
		log.Fatal(err)
	}
	if err := db.Init(); err != nil {
		log.Fatal(err)
	}
	if err := db.AddCollection(ctx, "sp", 0, vectors.Dot, vectors.VectorConfig{}); err != nil {
		log.Fatal(err)
	}
	var docs []doc
	var rows []vectors.Vector
	for i := 0; i < 60; i++ {
		indices, values := sparse(rng, 12)
		id := fmt.Sprintf("d%02d", i)
		docs = append(docs, doc{ID: id, Indices: indices, Values: values})
		rows = append(rows, vectors.Vector{Id: id, Indices: indices, Values: values, Timestamp: time.Unix(1, 0)})
	}
	if err := db.AddVectors(ctx, "sp", rows); err != nil {
		log.Fatal(err)
	}
	var queries []query
	for q := 0; q < 20; q++ {
		indices, values := sparse(rng, 12)
		scored, err := db.QueryVectors(ctx, "sp", vectors.Vector{Indices: indices, Values: values}, nil, 1000)
		if err != nil {
			log.Fatal(err)
		}
		entry := query{Indices: indices, Values: values}
		for _, s := range scored {
			entry.Scores = append(entry.Scores, score{Doc: s.Id, Bits: math.Float32bits(s.Score)})
		}
		queries = append(queries, entry)
	}
	f, err := os.Create(*out)
	if err != nil {
		log.Fatal(err)
	}
	defer f.Close()
	if err := json.NewEncoder(f).Encode(map[string]any{"docs": docs, "queries": queries}); err != nil {
		log.Fatal(err)
	}
}
