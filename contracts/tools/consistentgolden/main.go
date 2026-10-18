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

// Command consistentgolden writes golden data produced by
// github.com/lafikl/consistent, the hash ring the workers split users with
// (contracts/grpc.md), for the Rust port's tests
// (crates/gorse-common/src/consistent.rs):
//
//	go run ./contracts/tools/consistentgolden -out crates/gorse-common/tests/data/consistent_golden.json
//
// The file holds:
//
//   - hashes: the ring hash (first 8 bytes of BLAKE2b-512, little-endian) of
//     strings of many lengths, computed with the ring's own blake2b package;
//   - keys: user ids to place on the rings;
//   - rings: sequences of Add and Remove, with the ring after each (its sorted
//     points and point-to-host map, read by reflection since they are not
//     exported), then the owner Get gives every key;
//   - loads: sequences of GetLeast, Inc, Done, UpdateLoad, MaxLoad, GetLoads,
//     Add and Remove, with each result and the total load after each (MaxLoad
//     and GetLeast change it).
//
// Hashes and points are decimal strings: JSON numbers cannot hold every
// uint64.
package main

import (
	"bytes"
	"crypto/md5"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"math/rand/v2"
	"os"
	"reflect"
	"slices"
	"strconv"
	"strings"
	"unsafe"

	"github.com/lafikl/consistent"
	blake2b "github.com/minio/blake2b-simd"
)

// Hash is one string and its ring hash.
type Hash struct {
	Key  string `json:"key"`
	Hash string `json:"hash"`
}

// RingOp is one Add or Remove and the ring afterwards.
type RingOp struct {
	Op   string `json:"op"`
	Host string `json:"host"`
	// Result is what Remove returned.
	Result *bool `json:"result,omitempty"`
	// SortedSet is the sorted list of points; Points maps each point to its
	// host, sorted by point.
	SortedSet []string    `json:"sorted_set"`
	Points    [][2]string `json:"points"`
	Hosts     []string    `json:"hosts"`
}

// RingCase is a sequence of ring operations, then Get of every key.
type RingCase struct {
	Name string   `json:"name"`
	Ops  []RingOp `json:"ops"`
	// Owners holds, for each key, the index in Hosts of the host Get
	// returned, or -1 when Get failed, with the error in GetError.
	Hosts    []string `json:"hosts"`
	Owners   []int    `json:"owners"`
	GetError string   `json:"get_error,omitempty"`
}

// LoadOp is one operation of the bounded-load API and what Go did.
type LoadOp struct {
	Op   string `json:"op"`
	Host string `json:"host,omitempty"`
	Key  string `json:"key,omitempty"`
	Load *int64 `json:"load,omitempty"`
	// Result is the host from Get or GetLeast (possibly ""), the number from
	// MaxLoad (decimal), the bool from Remove, or "error: <text>" /
	// "panic: <text>"; absent for calls that return nothing.
	Result    *string          `json:"result,omitempty"`
	Loads     map[string]int64 `json:"loads"`
	TotalLoad int64            `json:"total_load"`
}

// LoadCase is a sequence of bounded-load operations.
type LoadCase struct {
	Name string   `json:"name"`
	Ops  []LoadOp `json:"ops"`
}

// InjectedCase starts from a ring state written into the unexported fields,
// one that only a 64-bit hash collision can produce, then runs operations
// as LoadCase does. SortedSet and Points are the ring afterwards.
type InjectedCase struct {
	Name      string           `json:"name"`
	Init      Ring             `json:"init"`
	Ops       []LoadOp         `json:"ops"`
	SortedSet []string         `json:"sorted_set"`
	Points    [][2]string      `json:"points"`
	Loads     map[string]int64 `json:"loads"`
}

// Ring is the state of a ring: its fields, as RingOp writes them.
type Ring struct {
	SortedSet []string         `json:"sorted_set"`
	Points    [][2]string      `json:"points"`
	Loads     map[string]int64 `json:"loads"`
	TotalLoad int64            `json:"total_load"`
}

// Golden is the whole file.
type Golden struct {
	Hashes   []Hash         `json:"hashes"`
	Keys     []string       `json:"keys"`
	Rings    []RingCase     `json:"rings"`
	Loads    []LoadCase     `json:"loads"`
	Injected []InjectedCase `json:"injected"`
}

func ringHash(key string) uint64 {
	out := blake2b.Sum512([]byte(key))
	return binary.LittleEndian.Uint64(out[:])
}

// field reads an unexported field of the ring.
func field(c *consistent.Consistent, name string) reflect.Value {
	return reflect.ValueOf(c).Elem().FieldByName(name)
}

func sortedSet(c *consistent.Consistent) []string {
	v := field(c, "sortedSet")
	out := make([]string, v.Len())
	for i := range v.Len() {
		out[i] = strconv.FormatUint(v.Index(i).Uint(), 10)
	}
	return out
}

func points(c *consistent.Consistent) [][2]string {
	v := field(c, "hosts")
	var keys []uint64
	hosts := map[uint64]string{}
	for it := v.MapRange(); it.Next(); {
		keys = append(keys, it.Key().Uint())
		hosts[it.Key().Uint()] = it.Value().String()
	}
	slices.Sort(keys)
	out := make([][2]string, 0, len(keys))
	for _, k := range keys {
		out = append(out, [2]string{strconv.FormatUint(k, 10), hosts[k]})
	}
	return out
}

// setField writes an unexported field of the ring.
func setField(c *consistent.Consistent, name string, v any) {
	f := field(c, name)
	reflect.NewAt(f.Type(), unsafe.Pointer(f.UnsafeAddr())).Elem().Set(reflect.ValueOf(v))
}

func totalLoad(c *consistent.Consistent) int64 {
	return field(c, "totalLoad").Int()
}

func sortedHosts(c *consistent.Consistent) []string {
	hosts := c.Hosts()
	slices.Sort(hosts)
	if hosts == nil {
		hosts = []string{}
	}
	return hosts
}

// uuid is how a worker names itself: md5(hostname + http host + port), hex.
func uuid(hostname, host string, port int) string {
	h := md5.New()
	h.Write([]byte(hostname))
	h.Write([]byte(host))
	h.Write([]byte(strconv.Itoa(port)))
	return hex.EncodeToString(h.Sum(nil))
}

func hashCases() []Hash {
	keys := []string{"", "a", "abc", "127.0.0.1:8000", "127.0.0.1:80000", "127.0.0.1:80009",
		"92.0.0.1:80001", "用户-1", "\x00", "emoji 😀"}
	for _, n := range []int{63, 64, 65, 127, 128, 129, 255, 256, 257, 1000} {
		keys = append(keys, strings.Repeat("x", n))
	}
	out := make([]Hash, 0, len(keys))
	for _, k := range keys {
		out = append(out, Hash{Key: k, Hash: strconv.FormatUint(ringHash(k), 10)})
	}
	return out
}

func makeKeys(rng *rand.Rand, workers []string) []string {
	keys := []string{"", "0", "1", "user", "用户", "😀"}
	for i := range 1000 {
		keys = append(keys, strconv.Itoa(i))
	}
	const alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-"
	for range 1500 {
		b := make([]byte, 1+rng.IntN(24))
		for i := range b {
			b[i] = alphabet[rng.IntN(len(alphabet))]
		}
		keys = append(keys, string(b))
	}
	for i := range 200 {
		keys = append(keys, uuid(fmt.Sprintf("user-%d", i), "", 0))
	}
	// Keys equal to ring points' preimages hash exactly onto a point.
	for _, w := range workers {
		keys = append(keys, w)
		for i := range 10 {
			keys = append(keys, fmt.Sprintf("%s%d", w, i))
		}
	}
	return keys
}

func runRing(name string, keys []string, ops [][2]string) RingCase {
	c := consistent.New()
	rc := RingCase{Name: name, Ops: []RingOp{}}
	for _, op := range ops {
		rec := RingOp{Op: op[0], Host: op[1]}
		switch op[0] {
		case "add":
			c.Add(op[1])
		case "remove":
			rec.Result = new(c.Remove(op[1]))
		default:
			panic(op[0])
		}
		rec.SortedSet = sortedSet(c)
		rec.Points = points(c)
		rec.Hosts = sortedHosts(c)
		rc.Ops = append(rc.Ops, rec)
	}
	rc.Hosts = sortedHosts(c)
	rc.Owners = make([]int, 0, len(keys))
	for _, k := range keys {
		host, err := c.Get(k)
		if err != nil {
			if rc.GetError != "" && rc.GetError != err.Error() {
				panic("two different Get errors")
			}
			rc.GetError = err.Error()
			rc.Owners = append(rc.Owners, -1)
			continue
		}
		i := slices.Index(rc.Hosts, host)
		if i < 0 {
			panic(fmt.Sprintf("Get returned %q, not a host", host))
		}
		rc.Owners = append(rc.Owners, i)
	}
	return rc
}

func adds(hosts ...string) [][2]string {
	var ops [][2]string
	for _, h := range hosts {
		ops = append(ops, [2]string{"add", h})
	}
	return ops
}

func ringCases(keys, workers []string) []RingCase {
	ops := func(parts ...[][2]string) [][2]string { return slices.Concat(parts...) }
	remove := func(h string) [][2]string { return [][2]string{{"remove", h}} }
	reversed := slices.Clone(workers[:10])
	slices.Reverse(reversed)
	return []RingCase{
		runRing("empty", keys, nil),
		runRing("one worker", keys, adds(workers[0])),
		runRing("two workers", keys, adds(workers[:2]...)),
		runRing("three workers", keys, adds(workers[:3]...)),
		runRing("five workers", keys, adds(workers[:5]...)),
		runRing("ten workers", keys, adds(workers[:10]...)),
		runRing("ten workers added in reverse", keys, adds(reversed...)),
		runRing("library test hosts", keys, adds("127.0.0.1:8000", "92.0.0.1:8000")),
		runRing("duplicate add is ignored", keys, adds(workers[0], workers[1], workers[0])),
		runRing("remove", keys, ops(adds(workers[:4]...), remove(workers[1]))),
		runRing("remove unknown host", keys, ops(adds(workers[:2]...), remove("not a worker"))),
		runRing("remove then add back", keys, ops(adds(workers[:3]...), remove(workers[0]), adds(workers[0]))),
		runRing("remove every host", keys, ops(adds(workers[:2]...), remove(workers[0]), remove(workers[1]))),
		runRing("empty host name", keys, adds("", workers[0])),
	}
}

type loadRunner struct {
	c   *consistent.Consistent
	out LoadCase
}

func (r *loadRunner) record(op LoadOp) {
	op.Loads = r.c.GetLoads()
	op.TotalLoad = totalLoad(r.c)
	r.out.Ops = append(r.out.Ops, op)
}

func (r *loadRunner) do(op LoadOp) (result string) {
	defer func() {
		if p := recover(); p != nil {
			op.Result = new(fmt.Sprintf("panic: %v", p))
			r.record(op)
			result = *op.Result
		}
	}()
	switch op.Op {
	case "add":
		r.c.Add(op.Host)
	case "remove":
		op.Result = new(strconv.FormatBool(r.c.Remove(op.Host)))
	case "get":
		host, err := r.c.Get(op.Key)
		if err != nil {
			op.Result = new("error: " + err.Error())
		} else {
			op.Result = new(host)
		}
	case "get_least":
		host, err := r.c.GetLeast(op.Key)
		if err != nil {
			op.Result = new("error: " + err.Error())
		} else {
			op.Result = new(host)
		}
	case "inc":
		r.c.Inc(op.Host)
	case "done":
		r.c.Done(op.Host)
	case "update_load":
		r.c.UpdateLoad(op.Host, *op.Load)
	case "max_load":
		op.Result = new(strconv.FormatInt(r.c.MaxLoad(), 10))
	case "loads":
	default:
		panic(op.Op)
	}
	r.record(op)
	if op.Result == nil {
		return ""
	}
	return *op.Result
}

func newRunner(name string, hosts ...string) *loadRunner {
	r := &loadRunner{c: consistent.New(), out: LoadCase{Name: name, Ops: []LoadOp{}}}
	for _, h := range hosts {
		r.do(LoadOp{Op: "add", Host: h})
	}
	return r
}

func loadCases(rng *rand.Rand, workers []string) []LoadCase {
	var cases []LoadCase

	// TestGetLeast.
	r := newRunner("TestGetLeast", "127.0.0.1:8000", "92.0.0.1:8000")
	for range 100 {
		host := r.do(LoadOp{Op: "get_least", Key: "92.0.0.1:80001"})
		r.do(LoadOp{Op: "inc", Host: host})
	}
	r.do(LoadOp{Op: "max_load"})
	cases = append(cases, r.out)

	// TestIncDone.
	r = newRunner("TestIncDone", "127.0.0.1:8000", "92.0.0.1:8000")
	host := r.do(LoadOp{Op: "get_least", Key: "92.0.0.1:80001"})
	r.do(LoadOp{Op: "inc", Host: host})
	r.do(LoadOp{Op: "done", Host: host})
	cases = append(cases, r.out)

	// The calls that change the total load as a side effect.
	r = newRunner("side effects on the total load", workers[0], workers[1])
	r.do(LoadOp{Op: "max_load"}) // total 0 becomes 1
	r.do(LoadOp{Op: "done", Host: workers[0]})
	r.do(LoadOp{Op: "done", Host: workers[0]}) // total -1
	r.do(LoadOp{Op: "get_least", Key: "k"})    // total -1 becomes 0
	r.do(LoadOp{Op: "inc", Host: workers[1]})
	r.do(LoadOp{Op: "inc", Host: "unknown"})
	r.do(LoadOp{Op: "done", Host: "unknown"})
	r.do(LoadOp{Op: "update_load", Host: "unknown", Load: new(int64(5))})
	r.do(LoadOp{Op: "remove", Host: workers[1]}) // its load stays in the total
	r.do(LoadOp{Op: "max_load"})
	cases = append(cases, r.out)

	// An empty ring.
	r = newRunner("empty ring")
	r.do(LoadOp{Op: "get_least", Key: "k"})
	r.do(LoadOp{Op: "max_load"})
	cases = append(cases, r.out)

	// Random sequences over a few hosts, unknown hosts included.
	for n := range 4 {
		hosts := append(slices.Clone(workers[:2+n]), "unknown")
		r = newRunner(fmt.Sprintf("random %d", n), workers[:2+n]...)
		last := hosts[0]
		for range 400 {
			switch x := rng.IntN(100); {
			case x < 35:
				res := r.do(LoadOp{Op: "get_least", Key: strconv.Itoa(rng.IntN(50))})
				if !strings.HasPrefix(res, "error: ") {
					last = res
				}
			case x < 65:
				r.do(LoadOp{Op: "inc", Host: last})
			case x < 80:
				r.do(LoadOp{Op: "done", Host: hosts[rng.IntN(len(hosts))]})
			case x < 86:
				r.do(LoadOp{Op: "update_load", Host: hosts[rng.IntN(len(hosts))], Load: new(int64(rng.IntN(30) - 5))})
			case x < 94:
				r.do(LoadOp{Op: "max_load"})
			case x < 97:
				r.do(LoadOp{Op: "remove", Host: hosts[rng.IntN(len(hosts)-1)]})
			default:
				r.do(LoadOp{Op: "add", Host: hosts[rng.IntN(len(hosts)-1)]})
			}
		}
		r.do(LoadOp{Op: "loads"})
		cases = append(cases, r.out)
	}
	return cases
}

// encode writes the file with one entry per line, so that a regenerated file
// diffs entry by entry.
func encode(g Golden) (*bytes.Buffer, error) {
	var buf bytes.Buffer
	sections := []struct {
		name  string
		items []any
	}{
		{"hashes", toAny(g.Hashes)},
		{"keys", []any{g.Keys}},
		{"rings", toAny(g.Rings)},
		{"loads", toAny(g.Loads)},
		{"injected", toAny(g.Injected)},
	}
	buf.WriteString("{\n")
	for si, sec := range sections {
		if sec.name == "keys" {
			line, err := json.Marshal(g.Keys)
			if err != nil {
				return nil, err
			}
			fmt.Fprintf(&buf, "  %q: %s", sec.name, line)
		} else {
			fmt.Fprintf(&buf, "  %q: [\n", sec.name)
			for i, item := range sec.items {
				line, err := json.Marshal(item)
				if err != nil {
					return nil, err
				}
				buf.WriteString("    ")
				buf.Write(line)
				if i < len(sec.items)-1 {
					buf.WriteByte(',')
				}
				buf.WriteByte('\n')
			}
			buf.WriteString("  ]")
		}
		if si < len(sections)-1 {
			buf.WriteByte(',')
		}
		buf.WriteByte('\n')
	}
	buf.WriteString("}\n")
	if !json.Valid(buf.Bytes()) {
		return nil, fmt.Errorf("invalid JSON written")
	}
	return &buf, nil
}

func toAny[T any](s []T) []any {
	out := make([]any, len(s))
	for i, v := range s {
		out[i] = v
	}
	return out
}

// injectedCase runs ops on a ring set to init.
func injectedCase(name string, init Ring, ops []LoadOp) InjectedCase {
	c := consistent.New()
	var set []uint64
	for _, p := range init.SortedSet {
		v, err := strconv.ParseUint(p, 10, 64)
		if err != nil {
			panic(err)
		}
		set = append(set, v)
	}
	hosts := map[uint64]string{}
	for _, p := range init.Points {
		v, err := strconv.ParseUint(p[0], 10, 64)
		if err != nil {
			panic(err)
		}
		hosts[v] = p[1]
	}
	loadMap := map[string]*consistent.Host{}
	for h, l := range init.Loads {
		loadMap[h] = &consistent.Host{Name: h, Load: l}
	}
	setField(c, "sortedSet", set)
	setField(c, "hosts", hosts)
	setField(c, "loadMap", loadMap)
	setField(c, "totalLoad", init.TotalLoad)
	r := &loadRunner{c: c, out: LoadCase{Name: name, Ops: []LoadOp{}}}
	for _, op := range ops {
		r.do(op)
	}
	return InjectedCase{
		Name:      name,
		Init:      init,
		Ops:       r.out.Ops,
		SortedSet: sortedSet(c),
		Points:    points(c),
		Loads:     c.GetLoads(),
	}
}

func injectedCases() []InjectedCase {
	// Points are placed around the hash of "probe".
	const probe = "probe"
	h := ringHash(probe)
	if h < 100 || h > math.MaxUint64-100 {
		panic("probe hash too close to the ends")
	}
	at := func(d int) string { return strconv.FormatUint(uint64(int64(h)+int64(d)), 10) }
	get := func(key string) LoadOp { return LoadOp{Op: "get", Key: key} }
	getLeast := func(key string) LoadOp { return LoadOp{Op: "get_least", Key: key} }

	// Three hosts; "heavy" is over its bound. Two points collided, so the
	// sorted set holds 30 points but the point map 29: GetLeast wraps at 29,
	// skipping the last point ("x"), and returns "y".
	var wrap Ring
	wrap.Loads = map[string]int64{"heavy": 100, "x": 0, "y": 0}
	wrap.TotalLoad = 100
	for i := range 30 {
		var p string
		switch {
		case i <= 2:
			p = at(i - 40)
		case i < 28:
			p = at(i - 41) // index 3 repeats index 2
		case i == 28:
			p = at(0)
		default:
			p = at(1)
		}
		wrap.SortedSet = append(wrap.SortedSet, p)
	}
	for i, p := range wrap.SortedSet {
		host := "heavy"
		switch i {
		case 1:
			host = "y"
		case 29:
			host = "x"
		}
		if i == 3 {
			continue // the collided point has one map entry
		}
		wrap.Points = append(wrap.Points, [2]string{p, host})
	}

	// "b" collided with one of "a"'s points and was removed: the point is
	// still in the sorted set but has no host.
	orphan := Ring{
		SortedSet: []string{at(-1), at(0), at(1)},
		Points:    [][2]string{{at(-1), "a"}, {at(1), "a"}},
		Loads:     map[string]int64{"a": 0},
	}

	return []InjectedCase{
		injectedCase("GetLeast wraps at the number of distinct points", wrap, []LoadOp{
			get(probe), getLeast(probe), {Op: "inc", Host: "y"}, getLeast(probe),
			{Op: "update_load", Host: "heavy", Load: new(int64(0))}, getLeast(probe),
			{Op: "remove", Host: "x"}, get(probe),
		}),
		injectedCase("Get of a point with no host", orphan, []LoadOp{
			get(probe), get("a0"), getLeast(probe), {Op: "remove", Host: "a"}, get(probe),
		}),
	}
}

func main() {
	out := flag.String("out", "", "output JSON file")
	flag.Parse()
	if *out == "" {
		fmt.Fprintln(os.Stderr, "missing -out")
		os.Exit(2)
	}
	rng := rand.New(rand.NewPCG(5, 6))
	var workers []string
	for i := range 10 {
		workers = append(workers, uuid(fmt.Sprintf("worker-%d", i), "0.0.0.0", 8089))
	}
	keys := makeKeys(rng, workers)
	g := Golden{
		Hashes:   hashCases(),
		Keys:     keys,
		Rings:    ringCases(keys, workers),
		Loads:    loadCases(rng, workers),
		Injected: injectedCases(),
	}
	buf, err := encode(g)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	if err = os.WriteFile(*out, buf.Bytes(), 0o644); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
