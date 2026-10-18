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

// Command heapgolden writes golden data produced by common/heap for the Rust
// port's tests (crates/gorse-common/src/heap.rs):
//
//	go run ./contracts/tools/heapgolden -out crates/gorse-common/tests/data/heap_golden.json
//
// Each case records its inputs together with what Go did with them, so the
// Rust side replays the inputs and compares, without having to reproduce how
// they were drawn:
//
//   - priority_queue: a sequence of Push, Pop and Peek on a PriorityQueue, with
//     the result of every operation and the heap array (Elems) after it; then
//     Values, Reverse, Clone followed by pushes of values already present, and
//     Marshal of the final queue;
//   - top_k: pushes into a TopKFilter, then PopAll (or PopAllValues), for both
//     instantiations Gorse uses ([int32, float32] and [string, float64]);
//   - unmarshal: hand-made byte strings given to PriorityQueue.Unmarshal, with
//     the resulting queue, or the error (or panic) Go reports.
//
// Floats are written as their IEEE bits in hex so that NaN payloads, infinities
// and the sign of zero survive the JSON round trip.
package main

import (
	"bytes"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"math/rand/v2"
	"os"
	"slices"
	"strconv"

	"github.com/gorse-io/gorse/common/heap"
)

// Pair is one element: [value, weight bits]. The value is a number for
// int32 elements and a string for string elements.
type Pair [2]any

// Op is one operation on a PriorityQueue and what Go returned.
type Op struct {
	Op     string `json:"op"`
	Value  *int32 `json:"value,omitempty"`
	Weight string `json:"weight,omitempty"`
	Elems  []Pair `json:"elems"`
}

// PQCase is a sequence of operations on one PriorityQueue.
type PQCase struct {
	Name string `json:"name"`
	Desc bool   `json:"desc"`
	Ops  []Op   `json:"ops"`
	// Values is Values() after the operations.
	Values []int32 `json:"values"`
	// Reverse is Elems() of Reverse() after the operations.
	Reverse []Pair `json:"reverse"`
	// ClonePushes are pushed into Clone() of the final queue, starting with
	// up to three values the queue holds, given new weights; CloneElems is
	// Elems() of the clone afterwards.
	ClonePushes []Pair `json:"clone_pushes"`
	CloneElems  []Pair `json:"clone_elems"`
	// Marshal is the output of Marshal() of the final queue.
	Marshal string `json:"marshal"`
}

// TopKCase is a sequence of pushes into one TopKFilter.
type TopKCase struct {
	Name   string `json:"name"`
	Kind   string `json:"kind"`
	K      int    `json:"k"`
	Pushes []Pair `json:"pushes"`
	// Method is "pop_all" or "pop_all_values"; Result holds pairs for the
	// first and bare values for the second.
	Method string `json:"method"`
	Result []any  `json:"result"`
}

// UnmarshalCase is one input to PriorityQueue.Unmarshal.
type UnmarshalCase struct {
	Name  string  `json:"name"`
	Hex   string  `json:"hex"`
	Error string  `json:"error,omitempty"`
	Desc  bool    `json:"desc"`
	Elems []Pair  `json:"elems"`
	Set   []int32 `json:"lookup"`
	// Consumed is the number of input bytes Unmarshal read.
	Consumed int `json:"consumed"`
}

// Golden is the whole file.
type Golden struct {
	PriorityQueue []PQCase        `json:"priority_queue"`
	TopK          []TopKCase      `json:"top_k"`
	Unmarshal     []UnmarshalCase `json:"unmarshal"`
}

func f32(v float32) string { return fmt.Sprintf("%08x", math.Float32bits(v)) }
func f64(v float64) string { return fmt.Sprintf("%016x", math.Float64bits(v)) }

func pqPairs(elems []heap.Elem[int32, float32]) []Pair {
	pairs := make([]Pair, 0, len(elems))
	for _, e := range elems {
		pairs = append(pairs, Pair{e.Value, f32(e.Weight)})
	}
	return pairs
}

// pqOp is an operation to run: push (value, weight), pop or peek.
type pqOp struct {
	op     string
	value  int32
	weight float32
}

func runPQ(name string, desc bool, ops []pqOp, clonePushes []heap.Elem[int32, float32]) PQCase {
	pq := heap.NewPriorityQueue(desc)
	c := PQCase{Name: name, Desc: desc, Ops: []Op{}}
	for _, o := range ops {
		rec := Op{Op: o.op}
		switch o.op {
		case "push":
			pq.Push(o.value, o.weight)
			rec.Value = new(o.value)
			rec.Weight = f32(o.weight)
		case "pop":
			v, w := pq.Pop()
			rec.Value = new(v)
			rec.Weight = f32(w)
		case "peek":
			v, w := pq.Peek()
			rec.Value = new(v)
			rec.Weight = f32(w)
		default:
			panic(o.op)
		}
		rec.Elems = pqPairs(pq.Elems())
		c.Ops = append(c.Ops, rec)
	}
	c.Values = pq.Values()
	c.Reverse = pqPairs(pq.Reverse().Elems())
	cl := pq.Clone()
	var held []heap.Elem[int32, float32]
	for i, e := range pq.Elems() {
		if i < 3 {
			held = append(held, heap.Elem[int32, float32]{Value: e.Value, Weight: e.Weight + 1})
		}
	}
	clonePushes = append(held, clonePushes...)
	for _, e := range clonePushes {
		cl.Push(e.Value, e.Weight)
	}
	c.ClonePushes = pqPairs(clonePushes)
	c.CloneElems = pqPairs(cl.Elems())
	var buf bytes.Buffer
	if err := pq.Marshal(&buf); err != nil {
		panic(err)
	}
	c.Marshal = hex.EncodeToString(buf.Bytes())
	return c
}

// randomOps draws n operations. Values come from [0, values), so some repeat,
// including values already popped, which Go then ignores; weights come from
// the given set, so that weights tie.
func randomOps(rng *rand.Rand, n int, values int32, weights []float32) []pqOp {
	var ops []pqOp
	size := 0
	for range n {
		r := rng.IntN(10)
		switch {
		case r < 7 || size == 0:
			ops = append(ops, pqOp{op: "push", value: rng.Int32N(values), weight: weights[rng.IntN(len(weights))]})
			// Duplicates are ignored by Go; an upper bound is enough here.
			size++
		case r < 9:
			ops = append(ops, pqOp{op: "pop"})
			size--
		default:
			ops = append(ops, pqOp{op: "peek"})
		}
	}
	return ops
}

// fixOps drops pops and peeks that would hit an empty queue (Go panics there),
// using the real queue to know its length.
func fixOps(desc bool, ops []pqOp) []pqOp {
	pq := heap.NewPriorityQueue(desc)
	var kept []pqOp
	for _, o := range ops {
		switch o.op {
		case "push":
			pq.Push(o.value, o.weight)
		case "pop":
			if pq.Len() == 0 {
				continue
			}
			pq.Pop()
		case "peek":
			if pq.Len() == 0 {
				continue
			}
		}
		kept = append(kept, o)
	}
	return kept
}

func priorityQueueCases() []PQCase {
	rng := rand.New(rand.NewPCG(1, 2))
	var cases []PQCase

	// The sequence of pq_test.go.
	var testOps []pqOp
	for _, e := range []int32{5, 3, 7, 8, 6, 2, 9} {
		testOps = append(testOps, pqOp{op: "push", value: e, weight: float32(e)})
	}
	for range 3 {
		testOps = append(testOps, pqOp{op: "peek"}, pqOp{op: "pop"})
	}
	for _, desc := range []bool{false, true} {
		cases = append(cases, runPQ(fmt.Sprintf("pq_test sequence desc=%v", desc), desc, testOps,
			[]heap.Elem[int32, float32]{{Value: 5, Weight: 1}, {Value: 100, Weight: 4}}))
	}

	// The empty queue.
	cases = append(cases, runPQ("empty", false, nil, nil))
	cases = append(cases, runPQ("empty desc", true, nil, nil))

	// Many ties.
	ties := []float32{0, 1, 2, 3, 4}
	special := []float32{
		float32(math.Inf(1)), float32(math.Inf(-1)), float32(math.Copysign(0, -1)), 0,
		math.MaxFloat32, -math.MaxFloat32, math.SmallestNonzeroFloat32, 1.5, -2.25,
	}
	for i := range 6 {
		desc := i%2 == 1
		var weights []float32
		var label string
		switch i / 2 {
		case 0:
			weights, label = ties, "ties"
		case 1:
			weights, label = special, "special weights"
		default:
			for range 40 {
				weights = append(weights, rng.Float32()*10-5)
			}
			label = "random weights"
		}
		ops := fixOps(desc, randomOps(rng, 160, 160, weights))
		var clonePushes []heap.Elem[int32, float32]
		for range 5 {
			clonePushes = append(clonePushes, heap.Elem[int32, float32]{Value: rng.Int32N(160), Weight: weights[rng.IntN(len(weights))]})
		}
		cases = append(cases, runPQ(fmt.Sprintf("%s desc=%v", label, desc), desc, ops, clonePushes))
	}

	// The pattern of ann.Bruteforce: a descending queue kept at k elements by
	// popping after each push.
	for _, desc := range []bool{true, false} {
		var ops []pqOp
		pq := heap.NewPriorityQueue(desc)
		for i := range int32(200) {
			w := ties[rng.IntN(len(ties))] + float32(rng.IntN(3))*0.5
			ops = append(ops, pqOp{op: "push", value: i % 150, weight: w})
			pq.Push(i%150, w)
			if pq.Len() > 10 {
				ops = append(ops, pqOp{op: "pop"})
				pq.Pop()
			}
		}
		cases = append(cases, runPQ(fmt.Sprintf("bounded at 10 desc=%v", desc), desc, ops, nil))
	}
	return cases
}

func topKCases() []TopKCase {
	rng := rand.New(rand.NewPCG(3, 4))
	var cases []TopKCase
	nan32 := float32(math.NaN())
	nan64 := math.NaN()
	for _, k := range []int{-1, 0, 1, 3, 10, 50, 200} {
		for _, method := range []string{"pop_all", "pop_all_values"} {
			for _, withNaN := range []bool{false, true} {
				for _, kind := range []string{"int32/float32", "string/float64"} {
					tc := TopKCase{
						Name:   fmt.Sprintf("k=%d %s nan=%v", k, method, withNaN),
						Kind:   kind,
						K:      k,
						Method: method,
					}
					n := 120
					if kind == "int32/float32" {
						f := heap.NewTopKFilter[int32, float32](k)
						for range n {
							v := rng.Int32N(60)
							w := float32(rng.IntN(8)) / 2
							if withNaN && rng.IntN(6) == 0 {
								w = nan32
							}
							f.Push(v, w)
							tc.Pushes = append(tc.Pushes, Pair{v, f32(w)})
						}
						if method == "pop_all" {
							for _, e := range f.PopAll() {
								tc.Result = append(tc.Result, Pair{e.Value, f32(e.Weight)})
							}
						} else {
							for _, v := range f.PopAllValues() {
								tc.Result = append(tc.Result, v)
							}
						}
					} else {
						f := heap.NewTopKFilter[string, float64](k)
						for range n {
							v := strconv.Itoa(rng.IntN(60))
							w := float64(rng.IntN(8)) / 2
							if withNaN && rng.IntN(6) == 0 {
								w = nan64
							}
							f.Push(v, w)
							tc.Pushes = append(tc.Pushes, Pair{v, f64(w)})
						}
						if method == "pop_all" {
							for _, e := range f.PopAll() {
								tc.Result = append(tc.Result, Pair{e.Value, f64(e.Weight)})
							}
						} else {
							for _, v := range f.PopAllValues() {
								tc.Result = append(tc.Result, v)
							}
						}
					}
					if tc.Result == nil {
						tc.Result = []any{}
					}
					cases = append(cases, tc)
				}
			}
		}
	}
	return cases
}

// countingReader counts the bytes read through it.
type countingReader struct {
	r *bytes.Reader
	n int
}

func (c *countingReader) Read(p []byte) (int, error) {
	n, err := c.r.Read(p)
	c.n += n
	return n, err
}

func unmarshalCase(name string, data []byte) (c UnmarshalCase) {
	c = UnmarshalCase{Name: name, Hex: hex.EncodeToString(data), Elems: []Pair{}, Set: []int32{}}
	r := &countingReader{r: bytes.NewReader(data)}
	pq := heap.NewPriorityQueue(false)
	defer func() {
		if p := recover(); p != nil {
			c.Error = fmt.Sprintf("panic: %v", p)
			c.Consumed = r.n
		}
	}()
	if err := pq.Unmarshal(r); err != nil {
		c.Error = err.Error()
		c.Consumed = r.n
		return c
	}
	// The receiver is not exported; Marshal shows desc.
	var buf bytes.Buffer
	if err := pq.Marshal(&buf); err != nil {
		panic(err)
	}
	c.Desc = buf.Bytes()[0] != 0
	c.Elems = pqPairs(pq.Elems())
	// The lookup set is not exported; it is exactly the set of values Push
	// now ignores. Probe every value in the input plus one that is not.
	var probes []int32
	for _, e := range pq.Elems() {
		if !slices.Contains(probes, e.Value) {
			probes = append(probes, e.Value)
		}
	}
	slices.Sort(probes)
	for _, v := range append(probes, 1_000_000) {
		before := pq.Len()
		pq.Push(v, 0)
		if pq.Len() == before {
			c.Set = append(c.Set, v)
		}
	}
	c.Consumed = r.n
	return c
}

func le32(v int32) []byte {
	return binary.LittleEndian.AppendUint32(nil, uint32(v))
}

func elem(v int32, w float32) []byte {
	return binary.LittleEndian.AppendUint32(le32(v), math.Float32bits(w))
}

func cat(parts ...[]byte) []byte {
	return slices.Concat(parts...)
}

func unmarshalCases() []UnmarshalCase {
	return []UnmarshalCase{
		unmarshalCase("ascending, two elements", cat([]byte{0}, le32(2), elem(1, 1), elem(2, 2))),
		unmarshalCase("descending, empty", cat([]byte{1}, le32(0))),
		unmarshalCase("desc byte 2 reads as true", cat([]byte{2}, le32(1), elem(7, 3))),
		unmarshalCase("desc byte 0xff reads as true", cat([]byte{0xff}, le32(0))),
		unmarshalCase("not in heap order, kept as is", cat([]byte{0}, le32(3), elem(1, 9), elem(2, 1), elem(3, 5))),
		unmarshalCase("duplicate values", cat([]byte{0}, le32(3), elem(4, 1), elem(4, 2), elem(5, 3))),
		unmarshalCase("trailing bytes are left unread", cat([]byte{1}, le32(1), elem(9, 9), []byte{0xaa, 0xbb})),
		unmarshalCase("no input", nil),
		unmarshalCase("desc only", []byte{0}),
		unmarshalCase("short length", cat([]byte{0}, []byte{1, 0})),
		unmarshalCase("short elements", cat([]byte{0}, le32(2), elem(1, 1), []byte{1, 2, 3})),
		unmarshalCase("negative length", cat([]byte{0}, le32(-1))),
	}
}

// writeJSON writes the file with one case per line, so that a regenerated file
// diffs case by case without being megabytes of indentation.
func writeJSON(path string, g Golden) error {
	var buf bytes.Buffer
	section := func(name string, cases []any, last bool) error {
		fmt.Fprintf(&buf, "  %q: [\n", name)
		for i, c := range cases {
			line, err := json.Marshal(c)
			if err != nil {
				return err
			}
			buf.WriteString("    ")
			buf.Write(line)
			if i < len(cases)-1 {
				buf.WriteByte(',')
			}
			buf.WriteByte('\n')
		}
		buf.WriteString("  ]")
		if !last {
			buf.WriteByte(',')
		}
		buf.WriteByte('\n')
		return nil
	}
	buf.WriteString("{\n")
	if err := section("priority_queue", toAny(g.PriorityQueue), false); err != nil {
		return err
	}
	if err := section("top_k", toAny(g.TopK), false); err != nil {
		return err
	}
	if err := section("unmarshal", toAny(g.Unmarshal), true); err != nil {
		return err
	}
	buf.WriteString("}\n")
	if !json.Valid(buf.Bytes()) {
		return fmt.Errorf("invalid JSON written")
	}
	return os.WriteFile(path, buf.Bytes(), 0o644)
}

func toAny[T any](s []T) []any {
	out := make([]any, len(s))
	for i, v := range s {
		out[i] = v
	}
	return out
}

func main() {
	out := flag.String("out", "", "output JSON file")
	flag.Parse()
	if *out == "" {
		fmt.Fprintln(os.Stderr, "missing -out")
		os.Exit(2)
	}
	g := Golden{
		PriorityQueue: priorityQueueCases(),
		TopK:          topKCases(),
		Unmarshal:     unmarshalCases(),
	}
	if err := writeJSON(*out, g); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
