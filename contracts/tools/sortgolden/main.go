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

// Command sortgolden writes golden data produced by sort.Sort for the Rust
// port's tests (crates/gorse-common/src/gosort.rs):
//
//	go run ./contracts/tools/sortgolden -out crates/gorse-common/tests/data/sort_golden.json
//
// Each case gives the keys of a slice and the comparison direction. Go sorts
// the indexes 0..n-1 by their keys, with Less comparing keys only, and records
// the resulting order. Sort is not stable, so with repeated keys the order of
// equal elements is Go's own, and the Rust side must reproduce it exactly.
//
// heapSort is unexported, and pdqsort reaches it only on inputs that unbalance
// its partitions again and again, which random keys do not produce. So the
// tool also writes sort_heap_golden.json, which records heapSort itself: a
// verbatim copy of Go 1.27's siftDown and heapSort (sort/zsortinterface.go),
// run on the same kinds of slices.
//
// The cases cover every length from 0 to 40, the lengths around pdqsort's
// thresholds (50 and 8), and longer slices, each with several key patterns
// that take different paths through pdqsort: random keys with few or many
// distinct values, sorted, reversed, organ pipe, sawtooth, all equal, and
// nearly sorted.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"math/rand/v2"
	"os"
	"sort"
)

// Case is one golden case.
type Case struct {
	Name       string `json:"name"`
	Descending bool   `json:"descending"`
	Keys       []int  `json:"keys"`
	Order      []int  `json:"order"`
}

// byKey is sort.Interface over a slice of indexes, compared by their keys.
type byKey struct {
	idx        []int
	keys       []int
	descending bool
}

func (s byKey) Len() int { return len(s.idx) }

func (s byKey) Less(i, j int) bool {
	x, y := s.keys[s.idx[i]], s.keys[s.idx[j]]
	if s.descending {
		return x > y
	}
	return x < y
}

func (s byKey) Swap(i, j int) { s.idx[i], s.idx[j] = s.idx[j], s.idx[i] }

func sortOrder(keys []int, descending bool) []int {
	data := byKey{idx: make([]int, len(keys)), keys: keys, descending: descending}
	for i := range data.idx {
		data.idx[i] = i
	}
	sort.Sort(data)
	return data.idx
}

func patterns(r *rand.Rand, n int) map[string][]int {
	p := make(map[string][]int)

	random := func(k int) []int {
		keys := make([]int, n)
		for i := range keys {
			keys[i] = r.IntN(k)
		}
		return keys
	}
	p["random_k2"] = random(2)
	p["random_k3"] = random(3)
	p["random_k10"] = random(10)
	p["random_wide"] = random(1 << 30)

	ascending := make([]int, n)
	for i := range ascending {
		ascending[i] = i
	}
	p["ascending"] = ascending

	descending := make([]int, n)
	for i := range descending {
		descending[i] = n - i
	}
	p["descending"] = descending

	equal := make([]int, n)
	p["all_equal"] = equal

	sawtooth := make([]int, n)
	for i := range sawtooth {
		sawtooth[i] = i % 7
	}
	p["sawtooth"] = sawtooth

	organ := make([]int, n)
	for i := range organ {
		if i < n/2 {
			organ[i] = i
		} else {
			organ[i] = n - i
		}
	}
	p["organ_pipe"] = organ

	nearly := make([]int, n)
	for i := range nearly {
		nearly[i] = i
	}
	for range 3 {
		if n > 1 {
			a, b := r.IntN(n), r.IntN(n)
			nearly[a], nearly[b] = nearly[b], nearly[a]
		}
	}
	p["nearly_sorted"] = nearly

	return p
}

// siftDown and heapSort below are copied verbatim from go1.27.0's
// src/sort/zsortinterface.go. They are unexported in package sort, so the
// tool carries its own copy of the same code.
func siftDown(data sort.Interface, lo, hi, first int) {
	root := lo
	for {
		child := 2*root + 1
		if child >= hi {
			break
		}
		if child+1 < hi && data.Less(first+child, first+child+1) {
			child++
		}
		if !data.Less(first+root, first+child) {
			return
		}
		data.Swap(first+root, first+child)
		root = child
	}
}

func heapSort(data sort.Interface, a, b int) {
	first := a
	lo := 0
	hi := b - a

	// Build heap with greatest element at top.
	for i := (hi - 1) / 2; i >= 0; i-- {
		siftDown(data, i, hi, first)
	}

	// Pop elements, largest first, into end of data.
	for i := hi - 1; i >= 0; i-- {
		data.Swap(first, first+i)
		siftDown(data, lo, i, first)
	}
}

func heapOrder(keys []int, descending bool) []int {
	data := byKey{idx: make([]int, len(keys)), keys: keys, descending: descending}
	for i := range data.idx {
		data.idx[i] = i
	}
	heapSort(data, 0, len(keys))
	return data.idx
}

// writeJSON writes v as indented JSON to path.
func writeJSON(path string, v any) {
	data, err := json.MarshalIndent(v, "", " ")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	if err := os.WriteFile(path, append(data, '\n'), 0o644); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func main() {
	out := flag.String("out", "sort_golden.json", "where to write the sort.Sort golden JSON")
	heapOut := flag.String("heap-out", "sort_heap_golden.json", "where to write the heapSort golden JSON")
	flag.Parse()

	sizes := make([]int, 0, 60)
	for n := 0; n <= 40; n++ {
		sizes = append(sizes, n)
	}
	sizes = append(sizes, 45, 49, 50, 51, 64, 100, 200, 500, 1000)

	r := rand.New(rand.NewPCG(20260928, 17))
	cases := make([]Case, 0)
	for _, n := range sizes {
		// Map iteration order is random, so the names are sorted to keep the
		// file the same from run to run.
		named := patterns(r, n)
		names := make([]string, 0, len(named))
		for name := range named {
			names = append(names, name)
		}
		sort.Strings(names)
		for _, name := range names {
			keys := named[name]
			for _, descending := range []bool{false, true} {
				cases = append(cases, Case{
					Name:       fmt.Sprintf("%s_n%d_desc%t", name, n, descending),
					Descending: descending,
					Keys:       keys,
					Order:      sortOrder(keys, descending),
				})
			}
		}
	}

	writeJSON(*out, map[string]any{"cases": cases})
	fmt.Printf("wrote %d cases to %s\n", len(cases), *out)

	heapCases := make([]Case, 0)
	for _, n := range sizes {
		named := patterns(r, n)
		names := make([]string, 0, len(named))
		for name := range named {
			names = append(names, name)
		}
		sort.Strings(names)
		for _, name := range names {
			keys := named[name]
			for _, descending := range []bool{false, true} {
				heapCases = append(heapCases, Case{
					Name:       fmt.Sprintf("%s_n%d_desc%t", name, n, descending),
					Descending: descending,
					Keys:       keys,
					Order:      heapOrder(keys, descending),
				})
			}
		}
	}
	writeJSON(*heapOut, map[string]any{"cases": heapCases})
	fmt.Printf("wrote %d heapSort cases to %s\n", len(heapCases), *heapOut)
}
