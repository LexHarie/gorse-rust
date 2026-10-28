// Command vectorxread is the Go side of the vector cross-read: the Go vector
// stores write a fixed dataset, or read and assert the one the Rust stores wrote,
// on every backend whose URI is set. conformance/tests/vector_cross_read.rs runs it
// in both directions.
//
// Usage (from the repository root): go run ./contracts/tools/vectorxread -mode write -prefix xg
//
//	go run ./contracts/tools/vectorxread -mode check -prefix xr
//
// Every backend is required. An unset URI, or a server that does not answer, is a
// failure, and so is any read that does not match the dataset.
package main

import (
	"context"
	"flag"
	"fmt"
	"log"
	"math"
	"os"
	"slices"
	"time"

	"github.com/gorse-io/gorse/storage/vectors"
)

// The timestamps of the dataset, in milliseconds. The Rust side writes the same.
const (
	tsA int64 = 1800000000123
	tsB int64 = 1800000001456
	tsC int64 = 1800000002789
)

// backend is one store the cross-read covers. Weaviate has no sparse vectors: Go's
// TestSparse skips it.
type backend struct {
	env    string
	name   string
	sparse bool
}

var backends = []backend{
	{env: "QDRANT_URI", name: "Qdrant", sparse: true},
	{env: "MILVUS_URI", name: "Milvus", sparse: true},
	{env: "WEAVIATE_URI", name: "Weaviate", sparse: false},
}

func main() {
	mode := flag.String("mode", "", "write the dataset under -prefix, or check it")
	prefix := flag.String("prefix", "", "the collection name prefix")
	flag.Parse()
	if *mode != "write" && *mode != "check" {
		log.Fatalf("-mode must be write or check, not %q", *mode)
	}
	if *prefix == "" {
		log.Fatal("-prefix is required")
	}
	ctx := context.Background()
	for _, b := range backends {
		uri := os.Getenv(b.env)
		if uri == "" {
			log.Fatalf("%s is not set: the vector cross-read needs a live %s server; start it from storage/docker-compose.yml and set %s", b.env, b.name, b.env)
		}
		db, err := vectors.Open(uri, "gorse_")
		if err != nil {
			log.Fatalf("cannot open %s at %s: %v", b.name, uri, err)
		}
		if err := db.Init(); err != nil {
			log.Fatalf("cannot initialise %s at %s: %v", b.name, uri, err)
		}
		if _, err := db.ListCollections(ctx); err != nil {
			log.Fatalf("cannot reach %s at %s: %v; is the server from storage/docker-compose.yml running?", b.name, uri, err)
		}
		switch *mode {
		case "write":
			write(ctx, db, *prefix, b.sparse)
		case "check":
			check(ctx, db, *prefix, b.sparse)
			cleanup(ctx, db, *prefix)
		}
		if err := db.Close(); err != nil {
			log.Fatalf("cannot close %s: %v", b.name, err)
		}
		fmt.Printf("vectorxread %s %s %s: ok\n", *mode, *prefix, b.name)
	}
}

func denseVectors() []vectors.Vector {
	return []vectors.Vector{
		{Id: "a", Values: []float32{1, 0, 0, 0}, Categories: []string{"cat-a", "common"}, Timestamp: time.UnixMilli(tsA).UTC()},
		{Id: "b", Values: []float32{0.9, 0.1, 0, 0}, Categories: []string{"cat-b", "common"}, Timestamp: time.UnixMilli(tsB).UTC()},
		{Id: "c", Values: []float32{0, 1, 0, 0}, Categories: []string{"common"}, IsHidden: true, Timestamp: time.UnixMilli(tsC).UTC()},
	}
}

func sparseVectors() []vectors.Vector {
	return []vectors.Vector{
		{Id: "s1", Indices: []uint32{1, 100}, Values: []float32{1, 2}, Categories: []string{"common"}, Timestamp: time.UnixMilli(tsA).UTC()},
		{Id: "s2", Indices: []uint32{2, 5}, Values: []float32{0.5, 0.5}, Categories: []string{"common"}, IsHidden: true, Timestamp: time.UnixMilli(tsB).UTC()},
	}
}

// must stops the run with the error, if there is one.
func must(err error) {
	if err != nil {
		log.Fatal(err)
	}
}

// expect stops the run with the message, if the condition does not hold.
func expect(cond bool, format string, args ...any) {
	if !cond {
		log.Fatalf(format, args...)
	}
}

// closeTo asserts two FP32 value lists agree to 1e-2, the precision of FP16 storage.
func closeTo(expected, actual []float32) {
	expect(len(expected) == len(actual), "values %v, want %v", actual, expected)
	for i := range expected {
		expect(math.Abs(float64(expected[i]-actual[i])) < 1e-2, "values %v, want %v", actual, expected)
	}
}

// unit scales a vector to length 1.
func unit(values []float32) []float32 {
	var sum float64
	for _, v := range values {
		sum += float64(v) * float64(v)
	}
	norm := math.Sqrt(sum)
	out := make([]float32, len(values))
	for i, v := range values {
		out[i] = float32(float64(v) / norm)
	}
	return out
}

// direction asserts dense values agree by direction. Cosine stores may normalise a
// vector (Qdrant does), so the lengths of the values are not compared.
func direction(expected, actual []float32) {
	closeTo(unit(expected), unit(actual))
}

func write(ctx context.Context, db vectors.Database, prefix string, sparse bool) {
	dense, sq, sp := prefix+"_dense", prefix+"_sq", prefix+"_sparse"
	for _, name := range []string{dense, sq, sp} {
		_ = db.DeleteCollection(ctx, name)
	}
	must(db.AddCollection(ctx, dense, 4, vectors.Cosine, vectors.VectorConfig{}))
	must(db.AddVectors(ctx, dense, denseVectors()))
	must(db.AddCollection(ctx, sq, 4, vectors.Cosine, vectors.VectorConfig{Type: vectors.QuantizationSQ}))
	must(db.AddVectors(ctx, sq, denseVectors()))
	if sparse {
		must(db.AddCollection(ctx, sp, 0, vectors.Dot, vectors.VectorConfig{}))
		must(db.AddVectors(ctx, sp, sparseVectors()))
	}
}

func check(ctx context.Context, db vectors.Database, prefix string, sparse bool) {
	expected := denseVectors()
	dense := prefix + "_dense"
	// Weaviate's describe reports no dimension, as Go's does not.
	dimension := 4
	if !sparse {
		dimension = 0
	}

	info, err := db.DescribeCollection(ctx, dense)
	must(err)
	expect(info.Dimension == dimension, "dense dimension %d, want %d", info.Dimension, dimension)
	expect(info.Distance == vectors.Cosine, "dense distance %v, want cosine", info.Distance)
	expect(info.VectorConfig == (vectors.VectorConfig{}), "dense config %+v, want none", info.VectorConfig)
	count, err := db.CountVectors(ctx, dense)
	must(err)
	expect(count == 3, "dense count %d, want 3", count)
	got, err := db.GetVectors(ctx, dense, []string{"a", "b", "c", "zz"})
	must(err)
	expect(len(got) == 3, "dense get returned %d vectors, want 3", len(got))
	for i := range got {
		expect(got[i].Id == expected[i].Id, "dense id %s, want %s", got[i].Id, expected[i].Id)
		direction(expected[i].Values, got[i].Values)
		expect(slices.Equal(got[i].Categories, expected[i].Categories), "categories of %s: %v", got[i].Id, got[i].Categories)
		expect(got[i].IsHidden == expected[i].IsHidden, "hidden of %s: %v", got[i].Id, got[i].IsHidden)
		expect(got[i].Timestamp.UnixMilli() == expected[i].Timestamp.UnixMilli(), "timestamp of %s: %d", got[i].Id, got[i].Timestamp.UnixMilli())
	}
	hits, err := db.QueryVectors(ctx, dense, vectors.Vector{Values: []float32{1, 0, 0, 0}}, []string{"common"}, 10)
	must(err)
	expect(len(hits) == 2 && hits[0].Id == "a" && hits[1].Id == "b", "query hits %v, want [a b]", hits)
	expect(hits[0].Score >= hits[1].Score, "scores %v", hits)

	sq := prefix + "_sq"
	info, err = db.DescribeCollection(ctx, sq)
	must(err)
	expect(info.Type == vectors.QuantizationSQ, "sq config %v, want SQ", info.Type)
	expect(info.Dimension == dimension, "sq dimension %d, want %d", info.Dimension, dimension)
	expect(info.Distance == vectors.Cosine, "sq distance %v, want cosine", info.Distance)
	got, err = db.GetVectors(ctx, sq, []string{"a", "b", "c"})
	must(err)
	expect(len(got) == 3, "sq get returned %d vectors, want 3", len(got))
	direction(expected[1].Values, got[1].Values)

	if !sparse {
		return
	}
	sp := prefix + "_sparse"
	info, err = db.DescribeCollection(ctx, sp)
	must(err)
	expect(info.Dimension == 0, "sparse dimension %d, want 0", info.Dimension)
	expect(info.Distance == vectors.Dot, "sparse distance %v, want dot", info.Distance)
	expect(info.VectorConfig == (vectors.VectorConfig{}), "sparse config %+v, want none", info.VectorConfig)
	count, err = db.CountVectors(ctx, sp)
	must(err)
	expect(count == 2, "sparse count %d, want 2", count)
	got, err = db.GetVectors(ctx, sp, []string{"s1", "s2"})
	must(err)
	expect(len(got) == 2, "sparse get returned %d vectors, want 2", len(got))
	expect(slices.Equal(got[0].Indices, []uint32{1, 100}), "indices of s1: %v", got[0].Indices)
	closeTo([]float32{1, 2}, got[0].Values)
	expect(slices.Equal(got[0].Categories, []string{"common"}), "categories of s1: %v", got[0].Categories)
	expect(!got[0].IsHidden, "s1 is hidden")
	expect(got[0].Timestamp.UnixMilli() == tsA, "timestamp of s1: %d", got[0].Timestamp.UnixMilli())
	expect(got[1].IsHidden, "s2 is not hidden")
	hits, err = db.QueryVectors(ctx, sp, vectors.Vector{Indices: []uint32{1}, Values: []float32{1}}, []string{"common"}, 10)
	must(err)
	expect(len(hits) == 1 && hits[0].Id == "s1", "sparse query hits %v, want [s1]", hits)
}

func cleanup(ctx context.Context, db vectors.Database, prefix string) {
	for _, name := range []string{prefix + "_dense", prefix + "_sq", prefix + "_sparse"} {
		_ = db.DeleteCollection(ctx, name)
	}
}
