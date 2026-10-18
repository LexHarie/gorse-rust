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

//go:debug randseednop=0

// Command anngolden writes golden data produced by common/ann and
// github.com/chewxy/math32 for the Rust port's tests
// (crates/gorse-common/src/ann.rs):
//
//	go run ./contracts/tools/anngolden -out crates/gorse-common/tests/data/ann_golden.json
//
// It needs an amd64 machine: math32.Log is assembly there (the Rust math32
// module follows it, with its own golden data).
//
//   - levels: the layer HNSW.insert draws for a uniform u, evaluated with the
//     expression of hnsw.go, and NewHNSW's level factor;
//   - indexes: Bruteforce and HNSW indexes built from the vectors in the file,
//     with what Add returned and what every search returned. An HNSW draws its
//     layers from the global math/rand source; the program seeds it
//     (rand.Seed, enabled by the go:debug line above) and records the values
//     the inserts drew, so the Rust side replays them without a port of
//     math/rand. The index itself is recorded as its Marshal output with the
//     upper-layer entries sorted by key ("canonical"): Go writes them in
//     sync.Map order, which changes from run to run;
//   - unmarshal: HNSW.Unmarshal of Go's blob for the "hnsw small" index,
//     truncated or patched, with Go's error, panic or the searches afterwards.
//
// gob assigns type ids process-wide in first-use order, so every index and
// unmarshal case runs in a fresh child process, as a fresh Rust encoder
// numbers types.
package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"math"
	"math/rand"
	"os"
	"os/exec"
	"slices"
	"strconv"
	"strings"

	"github.com/chewxy/math32"
	"github.com/gorse-io/gorse/common/ann"
	"github.com/gorse-io/gorse/common/encoding"
	"github.com/gorse-io/gorse/common/heap"
	"github.com/samber/lo"
)

// rng is splitmix64; it draws the vectors, never the HNSW layers.
type rng struct{ state uint64 }

func (r *rng) next() uint64 {
	r.state += 0x9e3779b97f4a7c15
	z := r.state
	z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9
	z = (z ^ (z >> 27)) * 0x94d049bb133111eb
	return z ^ (z >> 31)
}

// float returns a multiple of 2^-23 in [-1, 1).
func (r *rng) float() float32 {
	return float32(int32(r.next()>>40)-(1<<23)) / (1 << 23)
}

func f32(v float32) string { return fmt.Sprintf("%08x", math.Float32bits(v)) }

func f32s(v []float32) []string {
	d := []string{}
	for _, x := range v {
		d = append(d, f32(x))
	}
	return d
}

// l2 is the squared Euclidean distance as a plain loop, which Go (amd64,
// GOAMD64=v1: no fused multiply-add) and Rust round the same way.
func l2(a, b []float32) float32 {
	var s float32
	for i := range a {
		d := a[i] - b[i]
		s += d * d
	}
	return s
}

// jaccard is the distance of common/ann/ann_test.go, between sorted sets.
func jaccard(a, b []int) float32 {
	var i, j, intersection int
	for i < len(a) && j < len(b) {
		if a[i] == b[j] {
			intersection++
			i++
			j++
		} else if a[i] < b[j] {
			i++
		} else {
			j++
		}
	}
	if len(a)+len(b)-intersection == 0 {
		return 1
	}
	return 1 - float32(intersection)/float32(len(a)+len(b)-intersection)
}

// levelFactor is NewHNSW's.
var levelFactor = 1.0 / math32.Log(48)

// level is the expression of HNSW.insert (hnsw.go:131) for the uniform u.
func level(u float32) int {
	return int(math32.Floor(-math32.Log(u) * levelFactor))
}

// Search is one search and what Go returned: "index:bits" pairs, an error, or
// a panic.
type Search struct {
	Kind   string   `json:"kind"`
	Query  int      `json:"query"`
	Q      int      `json:"q"`
	K      int      `json:"k"`
	Prune0 bool     `json:"prune0"`
	Result []string `json:"result"`
	Error  string   `json:"error,omitempty"`
	Panic  string   `json:"panic,omitempty"`
}

func pairs(s []lo.Tuple2[int, float32]) []string {
	d := []string{}
	for _, p := range s {
		d = append(d, strconv.Itoa(p.A)+":"+f32(p.B))
	}
	return d
}

func catch(f func()) (msg string) {
	defer func() {
		if p := recover(); p != nil {
			msg = fmt.Sprint(p)
		}
	}()
	f()
	return ""
}

type index[T any] interface {
	SearchIndex(q, k int, prune0 bool) ([]lo.Tuple2[int, float32], error)
	SearchVector(q T, k int, prune0 bool) []lo.Tuple2[int, float32]
}

// Search parameters: the full suite for the indexes, a smaller one after
// Unmarshal.
var (
	allKs, fewKs       = []int{0, 1, 7, 100}, []int{1, 7, 100}
	allPrunes, noPrune = []bool{false, true}, []bool{false}
)

// searches searches for every query (Query is its position in the case's
// queries) and every stored index plus one past the end.
func searches[T any](idx index[T], n int, queries []T, ks []int, prunes []bool) []Search {
	var out []Search
	for i, q := range queries {
		for _, k := range ks {
			for _, prune0 := range prunes {
				s := Search{Kind: "vector", Query: i, K: k, Prune0: prune0}
				s.Panic = catch(func() { s.Result = pairs(idx.SearchVector(q, k, prune0)) })
				out = append(out, s)
			}
		}
	}
	for _, q := range lo.Uniq([]int{0, n / 2, n - 1, n}) {
		if q < 0 {
			continue
		}
		for _, k := range ks[len(ks)-3:] {
			for _, prune0 := range prunes {
				s := Search{Kind: "index", Q: q, K: k, Prune0: prune0}
				s.Panic = catch(func() {
					r, err := idx.SearchIndex(q, k, prune0)
					s.Result = pairs(r)
					if err != nil {
						s.Error = err.Error()
					}
				})
				out = append(out, s)
			}
		}
	}
	return out
}

// IndexCase is one index: its vectors, the uniforms its inserts drew, what Add
// returned (or the add that panicked), the index and its searches.
type IndexCase struct {
	Name     string   `json:"name"`
	Index    string   `json:"index"`
	Elem     string   `json:"elem"`
	Seed     int64    `json:"seed,omitempty"`
	Vectors  any      `json:"vectors"`
	Queries  any      `json:"queries"`
	Uniforms []string `json:"uniforms,omitempty"`
	Adds     []int    `json:"adds"`
	AddPanic string   `json:"add_panic,omitempty"`
	Layers   int      `json:"layers,omitempty"`
	BlobHash string   `json:"blob_sha256,omitempty"`
	// Blob is Go's own blob in hex, for small indexes. Its upper-layer entries
	// are in sync.Map order, so it changes from run to run; BlobHash is of the
	// canonical blob, with the entries sorted, and does not.
	Blob string `json:"blob,omitempty"`
	// Canonical is the canonical blob in hex, for the unmarshal cases; the
	// file holds only its hash.
	Canonical string   `json:"canonical,omitempty"`
	Searches  []Search `json:"searches"`
	// Limits are maxConnection, maxConnection0, ef and efConstruction, written
	// into the blob of an index holding the first vector, which is then
	// unmarshalled and given the others.
	Limits []int64 `json:"limits,omitempty"`
	// EfSearches are searches on the index after Marshal, ef set to 1 in the
	// blob, and Unmarshal: efSearch is then max(1, k), not max(100, k).
	EfSearches []Search `json:"ef_searches,omitempty"`
}

type spec struct {
	name, index, elem string
	n, dim            int
	coords            int // > 0: integer coordinates in [0, coords)
	seed              int64
	minLayers         int
	minLayer1         int
	skip              bool // a vector drawn two or more levels above the top
	efSearches        bool
	limits            []int64 // maxConnection, maxConnection0, ef, efConstruction
	nan               bool
	full              bool // record the blobs in full
}

func specs() []spec {
	return []spec{
		{name: "bruteforce empty", index: "bruteforce", elem: "f32", n: 0, dim: 3, seed: 1},
		{name: "bruteforce f32 grid", index: "bruteforce", elem: "f32", n: 60, dim: 3, coords: 4, seed: 2},
		{name: "bruteforce f32 random", index: "bruteforce", elem: "f32", n: 200, dim: 8, seed: 3},
		{name: "bruteforce int", index: "bruteforce", elem: "int", n: 80, dim: 40, seed: 4},
		{name: "bruteforce nan", index: "bruteforce", elem: "f32", n: 5, dim: 2, seed: 5, nan: true},
		{name: "hnsw empty", index: "hnsw", elem: "f32", n: 0, dim: 3, seed: 1},
		{name: "hnsw one", index: "hnsw", elem: "f32", n: 1, dim: 3, seed: 1},
		{name: "hnsw two", index: "hnsw", elem: "f32", n: 2, dim: 3, seed: 1},
		{name: "hnsw small", index: "hnsw", elem: "f32", n: 40, dim: 2, seed: 1, minLayers: 2, minLayer1: 3, skip: true, full: true},
		{name: "hnsw f32 grid", index: "hnsw", elem: "f32", n: 300, dim: 3, coords: 6, seed: 1, minLayers: 2},
		{name: "hnsw f32 random", index: "hnsw", elem: "f32", n: 600, dim: 8, seed: 1, minLayers: 3, skip: true},
		{name: "hnsw sparse", index: "hnsw", elem: "f32", n: 300, dim: 4, seed: 1, minLayers: 2, efSearches: true,
			limits: []int64{4, 8, 0, 3}},
		{name: "hnsw int", index: "hnsw", elem: "int", n: 200, dim: 60, seed: 1, minLayers: 2},
		{name: "hnsw nan", index: "hnsw", elem: "f32", n: 5, dim: 2, seed: 1, nan: true},
	}
}

// chooseSeed returns the first seed from s.seed on whose uniforms an index of
// s.n vectors gets at least s.minLayers upper layers and s.minLayer1 vectors
// in the first one, and with s.skip a vector drawn two or more levels above
// the top layer (which adds only one layer), so that those code paths run.
func chooseSeed(s spec) int64 {
	for seed := s.seed; ; seed++ {
		rand.Seed(seed)
		top, layer1, skipped := 0, 0, false
		for i := 1; i < s.n; i++ {
			l := level(rand.Float32())
			if l >= 1 {
				layer1++
			}
			if l >= top+2 {
				skipped = true
			}
			if l > top {
				top++
			}
		}
		if top >= s.minLayers && layer1 >= s.minLayer1 && (skipped || !s.skip) {
			return seed
		}
	}
}

func floatVectors(r *rng, s spec, n int) [][]float32 {
	v := make([][]float32, n)
	for i := range v {
		v[i] = make([]float32, s.dim)
		for j := range v[i] {
			if s.coords > 0 {
				v[i][j] = float32(r.next() % uint64(s.coords))
			} else {
				v[i][j] = r.float()
			}
		}
	}
	if s.nan && n > 3 {
		v[3][1] = float32(math.NaN())
	}
	return v
}

// intVectors are sorted subsets of [0, s.dim).
func intVectors(r *rng, s spec, n int) [][]int {
	v := make([][]int, n)
	for i := range v {
		v[i] = []int{}
		for j := 0; j < s.dim; j++ {
			if r.next()%4 == 0 {
				v[i] = append(v[i], j)
			}
		}
	}
	return v
}

func runCase(s spec) IndexCase {
	r := &rng{state: uint64(s.seed) * 1000003}
	c := IndexCase{Name: s.name, Index: s.index, Elem: s.elem, Adds: []int{}}
	if s.elem == "f32" {
		vectors := floatVectors(r, s, s.n)
		queries := append([][]float32{}, floatVectors(r, spec{dim: s.dim, coords: s.coords}, 3)...)
		for _, i := range lo.Uniq([]int{0, s.n / 2, s.n - 1}) {
			if i >= 0 && i < s.n {
				queries = append(queries, vectors[i])
			}
		}
		c.Vectors, c.Queries = lo.Map(vectors, func(v []float32, _ int) []string { return f32s(v) }),
			lo.Map(queries, func(v []float32, _ int) []string { return f32s(v) })
		build(&c, s, vectors, queries, l2)
	} else {
		vectors := intVectors(r, s, s.n)
		queries := intVectors(r, s, 3)
		for _, i := range lo.Uniq([]int{0, s.n / 2, s.n - 1}) {
			if i >= 0 && i < s.n {
				queries = append(queries, vectors[i])
			}
		}
		c.Vectors, c.Queries = vectors, queries
		build(&c, s, vectors, queries, jaccard)
	}
	return c
}

func build[T any](c *IndexCase, s spec, vectors, queries []T, dist func(a, b T) float32) {
	if s.index == "bruteforce" {
		bf := ann.NewBruteforce(dist)
		for _, v := range vectors {
			c.Adds = append(c.Adds, bf.Add(v))
		}
		c.Searches = searches[T](bf, len(vectors), queries, allKs, allPrunes)
		return
	}
	c.Seed = s.seed
	if s.minLayers > 0 {
		c.Seed = chooseSeed(s)
	}
	h := ann.NewHNSW(dist)
	if s.limits != nil {
		// NewHNSW's limits are fixed; Unmarshal reads them. The first vector
		// goes in before, so that the index is not empty (X-23).
		first := ann.NewHNSW(dist)
		first.Add(vectors[0])
		var b bytes.Buffer
		if err := first.Marshal(&b); err != nil {
			panic(err)
		}
		data := b.Bytes()
		for i, v := range s.limits {
			binary.LittleEndian.PutUint64(data[4+8*i:], uint64(v))
		}
		if err := h.Unmarshal(bytes.NewReader(data)); err != nil {
			panic(err)
		}
		c.Limits = s.limits
		c.Adds = append(c.Adds, 0)
		vectors = vectors[1:]
	}
	rand.Seed(c.Seed)
	for _, v := range vectors {
		var j int
		if c.AddPanic = catch(func() { j = h.Add(v) }); c.AddPanic != "" {
			break
		}
		c.Adds = append(c.Adds, j)
	}
	// The inserts draw one uniform each, all but the first; an insert that
	// panics on a NaN distance does so before it draws.
	rand.Seed(c.Seed)
	for range max(len(c.Adds)-1, 0) {
		c.Uniforms = append(c.Uniforms, f32(rand.Float32()))
	}
	if c.AddPanic != "" {
		return
	}
	var blob bytes.Buffer
	if err := h.Marshal(&blob); err != nil {
		panic(err)
	}
	p := parseBlob(blob.Bytes(), s.elem)
	c.Layers = p.layers
	sum := sha256.Sum256(p.canonical)
	c.BlobHash = hex.EncodeToString(sum[:])
	if s.full {
		c.Blob = hex.EncodeToString(blob.Bytes())
		c.Canonical = hex.EncodeToString(p.canonical)
	}
	c.Searches = searches[T](h, len(vectors), queries, allKs, allPrunes)
	if s.efSearches {
		patched := slices.Clone(blob.Bytes())
		binary.LittleEndian.PutUint64(patched[20:], 1)
		g := ann.NewHNSW(dist)
		if err := g.Unmarshal(bytes.NewReader(patched)); err != nil {
			panic(err)
		}
		c.EfSearches = searches[T](g, len(vectors), queries, allKs, noPrune)
	}
}

// parsed is an HNSW blob taken apart: the canonical form (upper-layer entries
// sorted by key) and the offsets of its fields.
type parsed struct {
	canonical []byte
	layers    int
	// Offsets of: numVectors, the first bottom queue, numLayers, the first
	// layer's count, its first entry's key, and enterPoint; and every field
	// boundary.
	numVectors, bottom, numLayers, layerCount, firstKey, enterPoint int
	boundaries                                                      []int
}

func parseBlob(blob []byte, elem string) parsed {
	r := bytes.NewReader(blob)
	pos := func() int { return len(blob) - r.Len() }
	var p parsed
	mark := func() { p.boundaries = append(p.boundaries, pos()) }
	read := func(v any) {
		if err := binary.Read(r, binary.LittleEndian, v); err != nil {
			panic(err)
		}
		mark()
	}
	mark()
	var lf float32
	var params [4]int64
	read(&lf)
	for i := range params {
		read(&params[i])
	}
	p.numVectors = pos()
	var n int64
	read(&n)
	for range n {
		var err error
		if elem == "f32" {
			var v []float32
			err = encoding.ReadGob(r, &v)
		} else {
			var v []int
			err = encoding.ReadGob(r, &v)
		}
		if err != nil {
			panic(err)
		}
		mark()
	}
	p.bottom = pos()
	for range n {
		if err := heap.NewPriorityQueue(false).Unmarshal(r); err != nil {
			panic(err)
		}
		mark()
	}
	p.numLayers = pos()
	var layers int64
	read(&layers)
	p.layers = int(layers)
	head := slices.Clone(blob[:pos()])
	for i := range layers {
		if i == 0 {
			p.layerCount = pos()
		}
		var count int32
		read(&count)
		head = binary.LittleEndian.AppendUint32(head, uint32(count))
		type entry struct {
			key   int32
			bytes []byte
		}
		var entries []entry
		for j := range count {
			if i == 0 && j == 0 {
				p.firstKey = pos()
			}
			start := pos()
			var key int32
			read(&key)
			if err := heap.NewPriorityQueue(false).Unmarshal(r); err != nil {
				panic(err)
			}
			mark()
			entries = append(entries, entry{key, blob[start:pos()]})
		}
		slices.SortFunc(entries, func(a, b entry) int { return int(a.key - b.key) })
		for _, e := range entries {
			head = append(head, e.bytes...)
		}
	}
	p.enterPoint = pos()
	var ep int32
	read(&ep)
	head = binary.LittleEndian.AppendUint32(head, uint32(ep))
	if r.Len() != 0 {
		panic("trailing bytes")
	}
	p.canonical = head
	return p
}

// UnmarshalCase is HNSW.Unmarshal of the "hnsw small" blob, truncated to
// Truncate bytes or with Patch written at Offset, and what Go did: the error,
// the panic, or the searches on the result. With Add, the vectors are added
// afterwards with the recorded uniforms and the index is marshalled again.
type UnmarshalCase struct {
	Name     string   `json:"name"`
	Base     string   `json:"base"`
	Truncate int      `json:"truncate"`
	Offset   int      `json:"offset"`
	Patch    string   `json:"patch,omitempty"`
	Error    string   `json:"error,omitempty"`
	Panic    string   `json:"panic,omitempty"`
	Deadlock bool     `json:"deadlock,omitempty"`
	Add      any      `json:"add,omitempty"`
	Uniforms []string `json:"uniforms,omitempty"`
	Adds     []int    `json:"adds,omitempty"`
	BlobHash string   `json:"blob_sha256,omitempty"`
	Searches []Search `json:"searches,omitempty"`
}

func le64(v int64) string {
	return hex.EncodeToString(binary.LittleEndian.AppendUint64(nil, uint64(v)))
}
func le32(v int32) string {
	return hex.EncodeToString(binary.LittleEndian.AppendUint32(nil, uint32(v)))
}

// unmarshalSpecs lists the cases on the blob of "hnsw small" (and, for
// "empty", the blob of an index without vectors).
func unmarshalSpecs(small IndexCase, emptyBlob []byte) []UnmarshalCase {
	blob, err := hex.DecodeString(small.Blob)
	if err != nil {
		panic(err)
	}
	canonical, err := hex.DecodeString(small.Canonical)
	if err != nil {
		panic(err)
	}
	p := parseBlob(blob, "f32")
	cases := []UnmarshalCase{
		{Name: "go blob", Base: "blob", Truncate: len(blob)},
		{Name: "canonical blob", Base: "canonical", Truncate: len(canonical)},
		{Name: "empty index", Base: "empty", Truncate: len(emptyBlob)},
		{Name: "empty index, then add", Base: "empty", Truncate: len(emptyBlob), Add: [][]string{f32s([]float32{0.5, 0.25})}},
		{Name: "add after unmarshal", Base: "canonical", Truncate: len(canonical),
			Add: lo.Map([][]float32{{0.1, 0.2}, {-0.3, 0.9}, {0.5, 0.5}}, func(v []float32, _ int) []string { return f32s(v) })},
		{Name: "negative numVectors", Base: "canonical", Truncate: len(canonical), Offset: p.numVectors, Patch: le64(-1)},
		{Name: "huge numVectors", Base: "canonical", Truncate: len(canonical), Offset: p.numVectors, Patch: le64(1 << 62)},
		{Name: "negative numLayers", Base: "canonical", Truncate: len(canonical), Offset: p.numLayers, Patch: le64(-1)},
		{Name: "huge numLayers", Base: "canonical", Truncate: len(canonical), Offset: p.numLayers, Patch: le64(1 << 62)},
		{Name: "negative layer count", Base: "canonical", Truncate: len(canonical), Offset: p.layerCount, Patch: le32(-1)},
		{Name: "enter point out of range", Base: "canonical", Truncate: len(canonical), Offset: p.enterPoint, Patch: le32(1000)},
		{Name: "negative bottom queue length", Base: "canonical", Truncate: len(canonical), Offset: p.bottom + 1, Patch: le32(-2)},
		// Search parameters and connection limits come from the blob.
		{Name: "ef from the blob", Base: "canonical", Truncate: len(canonical), Offset: 4 + 16, Patch: le64(7)},
		{Name: "small limits from the blob", Base: "canonical", Truncate: len(canonical), Offset: 4, Patch: le64(2) + le64(3) + le64(0) + le64(5),
			Add: lo.Map([][]float32{{0.1, 0.2}, {-0.3, 0.9}, {0.5, 0.5}, {0.7, -0.1}}, func(v []float32, _ int) []string { return f32s(v) })},
	}
	// A key already in the layer: the later entry wins.
	if second := p.firstKey + 4; second < p.enterPoint {
		var q bytes.Reader
		q.Reset(canonical[p.firstKey+4:])
		if err := heap.NewPriorityQueue(false).Unmarshal(&q); err != nil {
			panic(err)
		}
		next := p.firstKey + 4 + (len(canonical[p.firstKey+4:]) - q.Len())
		if next < p.enterPoint {
			cases = append(cases, UnmarshalCase{Name: "duplicate key in a layer", Base: "canonical", Truncate: len(canonical),
				Offset: next, Patch: hex.EncodeToString(canonical[p.firstKey : p.firstKey+4])})
		}
	}
	// Every field boundary, one byte short of it, and the empty input.
	seen := map[int]bool{len(canonical): true}
	for _, b := range p.boundaries {
		for _, t := range []int{b - 1, b} {
			if t >= 0 && !seen[t] {
				seen[t] = true
				cases = append(cases, UnmarshalCase{Name: "truncated to " + strconv.Itoa(t), Base: "canonical", Truncate: t})
			}
		}
	}
	return cases
}

func runUnmarshal(c UnmarshalCase, small IndexCase, emptyBlob []byte) UnmarshalCase {
	var base []byte
	var err error
	switch c.Base {
	case "blob":
		base, err = hex.DecodeString(small.Blob)
	case "canonical":
		base, err = hex.DecodeString(small.Canonical)
	case "empty":
		base = emptyBlob
	}
	if err != nil {
		panic(err)
	}
	data := slices.Clone(base[:c.Truncate])
	if c.Patch != "" {
		patch, err := hex.DecodeString(c.Patch)
		if err != nil {
			panic(err)
		}
		copy(data[c.Offset:], patch)
	}
	h := ann.NewHNSW(l2)
	if c.Panic = catch(func() { err = h.Unmarshal(bytes.NewReader(data)) }); c.Panic != "" {
		return c
	}
	if err != nil {
		c.Error = err.Error()
		return c
	}
	if c.Add != nil {
		// Seeded as "hnsw small" plus one, recorded like the indexes.
		rand.Seed(small.Seed + 1)
		var adds []int
		for _, d := range c.Add.([][]string) {
			v := lo.Map(d, func(s string, _ int) float32 {
				b, err := strconv.ParseUint(s, 16, 32)
				if err != nil {
					panic(err)
				}
				return math.Float32frombits(uint32(b))
			})
			adds = append(adds, h.Add(v))
		}
		c.Adds = adds
		rand.Seed(small.Seed + 1)
		for range adds {
			c.Uniforms = append(c.Uniforms, f32(rand.Float32()))
		}
		var blob bytes.Buffer
		if err := h.Marshal(&blob); err != nil {
			panic(err)
		}
		sum := sha256.Sum256(parseBlob(blob.Bytes(), "f32").canonical)
		c.BlobHash = hex.EncodeToString(sum[:])
	}
	vectors := lo.Map(small.Queries.([][]string), func(d []string, _ int) []float32 {
		return lo.Map(d, func(s string, _ int) float32 {
			b, _ := strconv.ParseUint(s, 16, 32)
			return math.Float32frombits(uint32(b))
		})
	})
	c.Searches = searches[[]float32](h, len(small.Adds), vectors, fewKs, noPrune)
	return c
}

// LevelCase is the layer drawn for the uniform U, as a decimal string.
type LevelCase struct {
	U     string `json:"u"`
	Level string `json:"level"`
}

// Golden is the whole file.
type Golden struct {
	LevelFactor string          `json:"level_factor"`
	EmptyBlob   string          `json:"empty_blob"`
	Levels      []LevelCase     `json:"levels"`
	Indexes     []IndexCase     `json:"indexes"`
	Unmarshal   []UnmarshalCase `json:"unmarshal"`
}

func levelCases() []LevelCase {
	inputs := []float32{0, 1e-45, float32(math.Ldexp(1, -63)), float32(math.Ldexp(1, -24)), 0.5, 0.999, math.Float32frombits(0x3f7fffff)}
	for k := 1; k <= 11; k++ {
		x := float32(math.Pow(48, -float64(k)))
		inputs = append(inputs, x, math.Nextafter32(x, 0), math.Nextafter32(x, 1))
	}
	rand.Seed(99)
	for range 200 {
		inputs = append(inputs, rand.Float32())
	}
	return lo.Map(inputs, func(u float32, _ int) LevelCase { return LevelCase{U: f32(u), Level: strconv.Itoa(level(u))} })
}

// child runs one case in this process and prints its JSON.
func child(kind string, i int) {
	var v any
	switch kind {
	case "index":
		v = runCase(specs()[i])
	case "empty-blob":
		var b bytes.Buffer
		if err := ann.NewHNSW(l2).Marshal(&b); err != nil {
			panic(err)
		}
		v = hex.EncodeToString(b.Bytes())
	case "unmarshal":
		var in struct {
			Case  UnmarshalCase
			Small IndexCase
			Empty string
		}
		if err := json.NewDecoder(os.Stdin).Decode(&in); err != nil {
			panic(err)
		}
		in.Small.Queries = lo.Map(in.Small.Queries.([]any), func(q any, _ int) []string {
			return lo.Map(q.([]any), func(s any, _ int) string { return s.(string) })
		})
		if in.Case.Add != nil {
			in.Case.Add = lo.Map(in.Case.Add.([]any), func(q any, _ int) []string {
				return lo.Map(q.([]any), func(s any, _ int) string { return s.(string) })
			})
		}
		empty, err := hex.DecodeString(in.Empty)
		if err != nil {
			panic(err)
		}
		v = runUnmarshal(in.Case, in.Small, empty)
	}
	if err := json.NewEncoder(os.Stdout).Encode(v); err != nil {
		panic(err)
	}
}

func runChild(stdin io.Reader, args ...string) ([]byte, string, error) {
	self, err := os.Executable()
	if err != nil {
		panic(err)
	}
	cmd := exec.Command(self, args...)
	cmd.Stdin = stdin
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	return out, stderr.String(), err
}

func main() {
	out := flag.String("out", "", "output JSON file")
	kind := flag.String("child", "", "internal: run one case of this kind in this process")
	index := flag.Int("case", -1, "internal: the case to run")
	flag.Parse()
	if *kind != "" {
		child(*kind, *index)
		return
	}
	g := Golden{LevelFactor: f32(levelFactor), Levels: levelCases()}
	var small IndexCase
	for i, s := range specs() {
		data, stderr, err := runChild(nil, "-child", "index", "-case", strconv.Itoa(i))
		if err != nil {
			panic(fmt.Sprintf("%s: %v\n%s", s.name, err, stderr))
		}
		var c IndexCase
		if err := json.Unmarshal(data, &c); err != nil {
			panic(err)
		}
		g.Indexes = append(g.Indexes, c)
		if c.Name == "hnsw small" {
			small = c
		}
	}
	data, stderr, err := runChild(nil, "-child", "empty-blob")
	if err != nil {
		panic(fmt.Sprintf("%v\n%s", err, stderr))
	}
	var emptyHex string
	if err := json.Unmarshal(data, &emptyHex); err != nil {
		panic(err)
	}
	emptyBlob, err := hex.DecodeString(emptyHex)
	if err != nil {
		panic(err)
	}
	g.EmptyBlob = emptyHex
	small.Queries = lo.Map(small.Queries.([]any), func(q any, _ int) []string {
		return lo.Map(q.([]any), func(s any, _ int) string { return s.(string) })
	})
	for _, c := range unmarshalSpecs(small, emptyBlob) {
		in, err := json.Marshal(map[string]any{"Case": c, "Small": small, "Empty": emptyHex})
		if err != nil {
			panic(err)
		}
		data, stderr, err := runChild(bytes.NewReader(in), "-child", "unmarshal")
		if err != nil {
			// sync.Mutex is not reentrant: Go's runtime ends the process.
			if strings.Contains(stderr, "all goroutines are asleep - deadlock!") {
				c.Deadlock = true
				g.Unmarshal = append(g.Unmarshal, c)
				continue
			}
			panic(fmt.Sprintf("%s: %v\n%s", c.Name, err, stderr))
		}
		var r UnmarshalCase
		if err := json.Unmarshal(data, &r); err != nil {
			panic(err)
		}
		g.Unmarshal = append(g.Unmarshal, r)
	}
	// The canonical blob is its hash in the file; Rust derives the bytes from
	// Go's blob.
	for i := range g.Indexes {
		g.Indexes[i].Canonical = ""
	}
	b, err := json.MarshalIndent(g, "", " ")
	if err != nil {
		panic(err)
	}
	if err := os.WriteFile(*out, append(b, '\n'), 0o644); err != nil {
		panic(err)
	}
}
