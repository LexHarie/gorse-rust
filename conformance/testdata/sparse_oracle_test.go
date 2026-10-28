package logics

// The Go side of conformance/tests/sparse_oracle.rs (WP-3-4). It is not part of
// the Go tree: the test runs `go test -overlay` to add it to package logics as
// sparse_oracle_conformance_test.go, so the unexported sparse pruner and
// appendSparseVector are the Go ones, and the tree is not changed. The go tool
// ignores testdata directories, so `go build ./...` and `go vet ./...` skip it.
//
// The test writes the cases as JSON to the file named by SPARSE_ORACLE_OUT.

import (
	"encoding/json"
	"math"
	"math/rand"
	"os"
	"testing"

	"github.com/gorse-io/gorse/storage/vectors"
)

type oracleAdd struct {
	Index uint32 `json:"index"`
	Value uint32 `json:"value"`
}

type oracleCase struct {
	Kind    string      `json:"kind"`
	MaxNNZ  int         `json:"max_nnz,omitempty"`
	Adds    []oracleAdd `json:"adds,omitempty"`
	InIdx   []uint32    `json:"in_indices,omitempty"`
	InVal   []uint32    `json:"in_values,omitempty"`
	IDs     []int32     `json:"ids,omitempty"`
	IDF     []uint32    `json:"idf,omitempty"`
	Offset  uint32      `json:"offset,omitempty"`
	Indices []uint32    `json:"indices"`
	Values  []uint32    `json:"values"`
}

// Values include NaN and infinities, and repeat so that ties occur.
var oracleValues = []float32{0, 1, 2, 3, 0.5, -1, -0.5, float32(math.NaN()), float32(math.Inf(1)), float32(math.Inf(-1)), 1, 2}
var oracleIDF = []float32{1, 2, 3, 4, 0.5, 0, -1, 1, 2, float32(math.NaN()), float32(math.Inf(1)), 3}

func oracleBits(values []float32) []uint32 {
	out := make([]uint32, len(values))
	for i, v := range values {
		out[i] = math.Float32bits(v)
	}
	return out
}

func TestSparseOracle(t *testing.T) {
	path := os.Getenv("SPARSE_ORACLE_OUT")
	if path == "" {
		t.Fatal("SPARSE_ORACLE_OUT is not set; run this through conformance/tests/sparse_oracle.rs")
	}
	r := rand.New(rand.NewSource(20261028))
	var cases []oracleCase
	for range 400 {
		maxNNZ := r.Intn(7)
		c := oracleCase{Kind: "pruner", MaxNNZ: maxNNZ}
		for range r.Intn(40) {
			c.Adds = append(c.Adds, oracleAdd{
				Index: uint32(r.Intn(10)),
				Value: math.Float32bits(oracleValues[r.Intn(len(oracleValues))]),
			})
		}
		p := newSparseVectorPruner(maxNNZ)
		for _, a := range c.Adds {
			p.Add(a.Index, math.Float32frombits(a.Value))
		}
		indices, values := p.Result()
		c.Indices = indices
		c.Values = oracleBits(values)
		cases = append(cases, c)
	}
	// Enough distinct ids to reach the pruner's limit of 1024.
	for range 20 {
		c := oracleCase{Kind: "append"}
		var vec vectors.Vector
		for range r.Intn(8) {
			idx := uint32(r.Intn(1500))
			val := oracleValues[r.Intn(len(oracleValues))]
			c.InIdx = append(c.InIdx, idx)
			c.InVal = append(c.InVal, math.Float32bits(val))
			vec.Indices = append(vec.Indices, idx)
			vec.Values = append(vec.Values, val)
		}
		idf := make([]float32, r.Intn(2000))
		for i := range idf {
			idf[i] = oracleIDF[r.Intn(len(oracleIDF))]
		}
		ids := make([]int32, r.Intn(4000))
		for i := range ids {
			ids[i] = int32(r.Intn(len(idf)+4) - 2)
		}
		c.IDs = ids
		c.IDF = oracleBits(idf)
		c.Offset = uint32(r.Intn(1200))
		out := appendSparseVector(vec, ids, idf, c.Offset)
		c.Indices = out.Indices
		c.Values = oracleBits(out.Values)
		cases = append(cases, c)
	}
	data, err := json.Marshal(cases)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, data, 0o644); err != nil {
		t.Fatal(err)
	}
}
