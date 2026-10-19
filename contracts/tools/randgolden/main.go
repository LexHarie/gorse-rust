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

// Command randgolden produces what the Rust port of Go's math/rand and of
// common/util/random.go is checked against:
//
//   - -tables-out: the seeding and ziggurat tables of math/rand (rngCooked,
//     kn, wn, fn, ke, we, fe), read from the Go toolchain's own source files
//     and evaluated with go/constant, written as a Rust source file;
//   - -out: golden outputs of rand.Rand (the method stream of math/rand's own
//     TestRegress, plus long digested streams for many seeds), of math.Exp and
//     math.Log (which NormFloat64 and ExpFloat64 call), and of
//     util.RandomGenerator and util.NewRand.
//
// On amd64, math.Exp has two code paths, with and without FMA, that round
// differently, so the outputs are produced twice: in this process (FMA, which
// the generating machine must have) and in a child process run with
// GODEBUG=cpu.fma=off.
//
// Numbers are written as the hex of their 64-bit pattern: integers two's
// complement, float64 by math.Float64bits, float32 by math.Float32bits. Long
// streams are summarised by FNV-1a 64 over the little-endian bytes of those
// patterns (bytes of Read output are hashed one by one).
package main

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"go/ast"
	"go/constant"
	"go/parser"
	"go/token"
	"log"
	"math"
	"math/rand"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"strconv"
	"strings"

	mapset "github.com/deckarep/golang-set/v2"
	"github.com/gorse-io/gorse/common/util"
	"golang.org/x/sys/cpu"
)

// Golden is the whole output file: the math functions per math.Exp code path,
// and everything else once, since it is checked to be the same on both paths.
type Golden struct {
	GoVersion string           `json:"go_version"`
	Math      map[string]*Math `json:"math"`
	Rand      *Section         `json:"rand"`
}

// Child is what the GODEBUG=cpu.fma=off child process prints.
type Child struct {
	Math *Math    `json:"math"`
	Rand *Section `json:"rand"`
}

// Math holds math.Exp and math.Log outputs.
type Math struct {
	Exp     []MathCase   `json:"exp"`
	Log     []MathCase   `json:"log"`
	MathRnd []MathDigest `json:"random"`
}

// Section holds the outputs of the generators.
type Section struct {
	// InvRn is float64(1.0 / rn) as math/rand's NormFloat64 computes it.
	InvRn   string        `json:"inv_rn"`
	Regress []RegressCase `json:"regress"`
	Streams []Stream      `json:"streams"`
	Mixed   []Mixed       `json:"mixed"`
	Random  []RandomCase  `json:"random_generator"`
	Locked  []Stream      `json:"locked"`
}

// MathCase is one input of math.Exp or math.Log and its output.
type MathCase struct {
	In  string `json:"in"`
	Out string `json:"out"`
}

// MathDigest summarises a function over n inputs drawn by splitmix64.
type MathDigest struct {
	Func   string  `json:"func"`
	Lo     float64 `json:"lo"`
	Hi     float64 `json:"hi"`
	Seed   uint64  `json:"seed"`
	N      int     `json:"n"`
	Digest string  `json:"digest"`
}

// RegressCase is one call of math/rand's TestRegress.
type RegressCase struct {
	Method string `json:"method"`
	Arg    string `json:"arg"`
	Out    string `json:"out"`
}

// Stream is n calls of one method with one argument on a freshly seeded
// generator.
type Stream struct {
	Seed   int64    `json:"seed"`
	Method string   `json:"method"`
	Arg    int64    `json:"arg"`
	N      int      `json:"n"`
	First  []string `json:"first"`
	Digest string   `json:"digest"`
}

// Mixed is a script of calls on one generator, with every output.
type Mixed struct {
	Seed  int64    `json:"seed"`
	Calls []string `json:"calls"`
	Outs  []string `json:"outs"`
}

// RandomCase is a script of util.RandomGenerator calls on one generator.
type RandomCase struct {
	Seed  int64        `json:"seed"`
	Calls []RandomCall `json:"calls"`
}

// RandomCall is one util.RandomGenerator call and its output.
type RandomCall struct {
	Method  string    `json:"method"`
	Args    []float64 `json:"args"`
	Exclude [][]int64 `json:"exclude,omitempty"`
	Len     int       `json:"len"`
	First   []string  `json:"first"`
	Digest  string    `json:"digest"`
}

type digest uint64

func newDigest() digest { return 14695981039346656037 }

func (d *digest) u64(v uint64) {
	for k := 0; k < 8; k++ {
		*d ^= digest((v >> (8 * k)) & 0xff)
		*d *= 1099511628211
	}
}

func (d *digest) bytes(b []byte) {
	for _, c := range b {
		*d ^= digest(c)
		*d *= 1099511628211
	}
}

func (d digest) String() string { return fmt.Sprintf("%016x", uint64(d)) }

func hx(v uint64) string { return fmt.Sprintf("%016x", v) }

// splitmix64 is the input generator shared with the Rust test.
type splitmix64 uint64

func (s *splitmix64) next() uint64 {
	*s += 0x9e3779b97f4a7c15
	z := uint64(*s)
	z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9
	z = (z ^ (z >> 27)) * 0x94d049bb133111eb
	return z ^ (z >> 31)
}

// uniform maps the top 53 bits of next() to [lo, hi).
func (s *splitmix64) uniform(lo, hi float64) float64 {
	return lo + float64(s.next()>>11)*(1.0/(1<<53))*(hi-lo)
}

func expCases() []float64 {
	return []float64{
		0, math.Copysign(0, -1), 1, -1, 0.5, -0.5, 2, 10, -10, 100, -100,
		709.7, 709.78, 709.782712893384, 709.7827128933841, 709.79, 710, 1000,
		-708.39, -708.4, -709, -709.1, -744, -745, -745.13, -745.2, -746, -1075,
		-1e300, 1e300, math.MaxFloat64, -math.MaxFloat64, math.SmallestNonzeroFloat64,
		-math.SmallestNonzeroFloat64, 1e-300, -1e-300, math.Inf(1), math.Inf(-1), math.NaN(),
		-0.5 * 3.442619855899 * 3.442619855899, -7.69711747013104972, 0.6931471805599453,
		-0.6931471805599453, 1.0 / 16, 88.72283905206835, -103.97207708399179,
	}
}

func logCases() []float64 {
	cases := []float64{
		0, math.Copysign(0, -1), 1, 2, 0.5, 0.25, 3, 10, 0.1, -1, -0.5, -math.MaxFloat64,
		math.MaxFloat64, math.SmallestNonzeroFloat64, 2.2250738585072014e-308,
		2.225073858507201e-308, 1e-310, math.Inf(1), math.Inf(-1), math.NaN(),
		math.Sqrt2 / 2, math.Sqrt2, 1 - 1e-16, 1.0 / (1 << 63), math.E, 1e300, 1e-300,
		math.Nextafter(math.Sqrt2/2, 0), math.Nextafter(math.Sqrt2/2, 1),
	}
	for k := -1074; k <= 1023; k += 37 {
		cases = append(cases, math.Ldexp(math.Sqrt2/2, k), math.Ldexp(1, k), math.Ldexp(0.75, k))
	}
	return cases
}

func mathSection(s *Math) {
	for _, x := range expCases() {
		s.Exp = append(s.Exp, MathCase{hx(math.Float64bits(x)), hx(math.Float64bits(math.Exp(x)))})
	}
	for _, x := range logCases() {
		s.Log = append(s.Log, MathCase{hx(math.Float64bits(x)), hx(math.Float64bits(math.Log(x)))})
	}
	ranges := []struct {
		fn       string
		lo, hi   float64
		seed     uint64
		n        int
		evaluate func(float64) float64
	}{
		{"exp", -750, 712, 1, 300000, math.Exp},
		{"exp", -6, 0, 2, 300000, math.Exp},
		{"exp", -8, 0, 3, 300000, math.Exp},
		{"exp", -1, 1, 4, 300000, math.Exp},
		{"log", 0, 1, 5, 300000, math.Log},
		{"log", 0, 1e-5, 6, 100000, math.Log},
		{"log", 1, 1e300, 7, 100000, math.Log},
	}
	for _, r := range ranges {
		g := splitmix64(r.seed)
		d := newDigest()
		for i := 0; i < r.n; i++ {
			d.u64(math.Float64bits(r.evaluate(g.uniform(r.lo, r.hi))))
		}
		s.MathRnd = append(s.MathRnd, MathDigest{r.fn, r.lo, r.hi, r.seed, r.n, d.String()})
	}
	// Random bit patterns of positive finite doubles, subnormals included.
	g := splitmix64(8)
	d := newDigest()
	for i := 0; i < 100000; i++ {
		bits := g.next() & 0x7fefffffffffffff
		d.u64(math.Float64bits(math.Log(math.Float64frombits(bits))))
	}
	s.MathRnd = append(s.MathRnd, MathDigest{"log_bits", 0, 0, 8, 100000, d.String()})
	// Sqrt2/2 and its two neighbours either side at every normal exponent:
	// Log's argument reduction branches exactly at Sqrt2/2.
	d = newDigest()
	n := 0
	for e := uint64(1); e < 0x7ff; e++ {
		for delta := int64(-2); delta <= 2; delta++ {
			bits := e<<52 | uint64(int64(math.Float64bits(math.Sqrt2/2)&(1<<52-1))+delta)
			d.u64(math.Float64bits(math.Log(math.Float64frombits(bits))))
			n++
		}
	}
	s.MathRnd = append(s.MathRnd, MathDigest{"log_half_sqrt2", 0, 0, 0, n, d.String()})
}

// encode returns the hex of the 64-bit pattern of a scalar output.
func encode(v any) string {
	switch x := v.(type) {
	case int64:
		return hx(uint64(x))
	case int:
		return hx(uint64(x))
	case int32:
		return hx(uint64(int64(x)))
	case uint32:
		return hx(uint64(x))
	case uint64:
		return hx(x)
	case float64:
		return hx(math.Float64bits(x))
	case float32:
		return hx(uint64(math.Float32bits(x)))
	case []int:
		parts := make([]string, len(x))
		for i, e := range x {
			parts[i] = strconv.Itoa(e)
		}
		return strings.Join(parts, ",")
	case []byte:
		return hex.EncodeToString(x)
	}
	panic(fmt.Sprintf("unexpected type %T", v))
}

// regress replays math/rand's TestRegress (regress_test.go) and records every
// output instead of comparing with its table.
func regress(s *Section) {
	var int32s = []int32{1, 10, 32, 1 << 20, 1<<20 + 1, 1000000000, 1 << 30, 1<<31 - 2, 1<<31 - 1}
	var int64s = []int64{1, 10, 32, 1 << 20, 1<<20 + 1, 1000000000, 1 << 30, 1<<31 - 2, 1<<31 - 1, 1000000000000000000, 1 << 60, 1<<63 - 2, 1<<63 - 1}
	var permSizes = []int{0, 1, 5, 8, 9, 10, 16}
	var readBufferSizes = []int{1, 7, 8, 9, 10}
	r := rand.New(rand.NewSource(0))
	rv := reflect.ValueOf(r)
	for i := 0; i < rv.NumMethod(); i++ {
		m := rv.Type().Method(i)
		mv := rv.Method(i)
		mt := mv.Type()
		if mt.NumOut() == 0 {
			continue
		}
		r.Seed(0)
		for repeat := 0; repeat < 20; repeat++ {
			var args []reflect.Value
			arg := ""
			if mt.NumIn() == 1 {
				var x any
				switch mt.In(0).Kind() {
				case reflect.Int:
					if m.Name == "Perm" {
						x = permSizes[repeat%len(permSizes)]
						break
					}
					x = int(int64s[repeat%len(int64s)])
				case reflect.Int32:
					x = int32s[repeat%len(int32s)]
				case reflect.Int64:
					x = int64s[repeat%len(int64s)]
				case reflect.Slice:
					x = make([]byte, readBufferSizes[repeat%len(readBufferSizes)])
				default:
					log.Fatalf("unexpected argument type for r.%s", m.Name)
				}
				if b, ok := x.([]byte); ok {
					arg = strconv.Itoa(len(b))
				} else {
					arg = fmt.Sprint(x)
				}
				args = append(args, reflect.ValueOf(x))
			}
			out := mv.Call(args)[0].Interface()
			if m.Name == "Read" {
				out = args[0].Interface().([]byte)
			}
			s.Regress = append(s.Regress, RegressCase{m.Name, arg, encode(out)})
		}
	}
}

var seeds = []int64{
	0, 1, -1, 42, 89482311, 2147483647, -2147483647, 2147483646, 4294967294,
	1 << 40, -(1 << 40), math.MaxInt64, math.MinInt64, 1234567890123456789, 1760668980123456789,
}

type method struct {
	name string
	args []int64
	n    int
	call func(r *rand.Rand, arg int64) any
}

var methods = []method{
	{"Int63", []int64{0}, 5000, func(r *rand.Rand, _ int64) any { return r.Int63() }},
	{"Uint64", []int64{0}, 5000, func(r *rand.Rand, _ int64) any { return r.Uint64() }},
	{"Uint32", []int64{0}, 5000, func(r *rand.Rand, _ int64) any { return r.Uint32() }},
	{"Int31", []int64{0}, 5000, func(r *rand.Rand, _ int64) any { return r.Int31() }},
	{"Int", []int64{0}, 5000, func(r *rand.Rand, _ int64) any { return r.Int() }},
	{"Int63n", []int64{1, 2, 3, 7, 10, 1000, 1 << 31, 1<<31 + 1, 1 << 62, 1<<62 + 1, 1<<63 - 1, 6148914691236517205}, 2000,
		func(r *rand.Rand, n int64) any { return r.Int63n(n) }},
	{"Int31n", []int64{1, 2, 3, 7, 10, 1000, 1 << 30, 1<<30 + 1, 1<<31 - 1, 1431655765}, 2000,
		func(r *rand.Rand, n int64) any { return r.Int31n(int32(n)) }},
	{"Intn", []int64{1, 10, 1000, 1<<31 - 1, 1 << 31, 1<<40 + 7, 1<<63 - 1}, 2000,
		func(r *rand.Rand, n int64) any { return r.Intn(int(n)) }},
	{"Float64", []int64{0}, 20000, func(r *rand.Rand, _ int64) any { return r.Float64() }},
	{"Float32", []int64{0}, 20000, func(r *rand.Rand, _ int64) any { return r.Float32() }},
	{"NormFloat64", []int64{0}, 300000, func(r *rand.Rand, _ int64) any { return r.NormFloat64() }},
	{"ExpFloat64", []int64{0}, 300000, func(r *rand.Rand, _ int64) any { return r.ExpFloat64() }},
	{"Perm", []int64{0, 1, 2, 5, 10, 100}, 50, func(r *rand.Rand, n int64) any { return r.Perm(int(n)) }},
	{"Shuffle", []int64{0, 1, 2, 5, 10, 100}, 50, func(r *rand.Rand, n int64) any {
		a := make([]int, n)
		for i := range a {
			a[i] = i
		}
		r.Shuffle(len(a), func(i, j int) { a[i], a[j] = a[j], a[i] })
		return a
	}},
	{"Read", []int64{0, 1, 3, 7, 8, 9, 15, 16, 100}, 20, func(r *rand.Rand, n int64) any {
		b := make([]byte, n)
		_, _ = r.Read(b)
		return b
	}},
}

// hashOut adds one output to a digest: numbers as their 64-bit pattern,
// slices element by element.
func hashOut(d *digest, v any) {
	switch x := v.(type) {
	case []int:
		for _, e := range x {
			d.u64(uint64(e))
		}
	case []byte:
		d.bytes(x)
	default:
		u, err := strconv.ParseUint(encode(v), 16, 64)
		if err != nil {
			panic(err)
		}
		d.u64(u)
	}
}

func stream(seed int64, m method, arg int64, r *rand.Rand) Stream {
	st := Stream{Seed: seed, Method: m.name, Arg: arg, N: m.n}
	d := newDigest()
	for i := 0; i < m.n; i++ {
		out := m.call(r, arg)
		if i < 4 {
			st.First = append(st.First, encode(out))
		}
		hashOut(&d, out)
	}
	st.Digest = d.String()
	return st
}

func streams(s *Section) {
	for _, seed := range seeds {
		for _, m := range methods {
			for _, arg := range m.args {
				s.Streams = append(s.Streams, stream(seed, m, arg, rand.New(rand.NewSource(seed))))
			}
		}
	}
}

// mixed interleaves Read, other methods and Seed on one generator: Read keeps
// leftover bytes across calls until Seed resets them.
func mixed(s *Section) {
	script := []string{"Read 3", "Int63", "Read 5", "Read 9", "Float64", "Read 1", "Seed 7", "Read 2",
		"Read 6", "NormFloat64", "Read 0", "Read 13", "Uint64", "Seed -3", "Int31n 5", "Read 4"}
	for _, seed := range []int64{0, 5, -11} {
		mx := Mixed{Seed: seed, Calls: script}
		r := rand.New(rand.NewSource(seed))
		for _, call := range script {
			mx.Outs = append(mx.Outs, mixedCall(r, call))
		}
		s.Mixed = append(s.Mixed, mx)
	}
}

func mixedCall(r *rand.Rand, call string) string {
	fields := strings.Fields(call)
	arg := int64(0)
	if len(fields) > 1 {
		var err error
		if arg, err = strconv.ParseInt(fields[1], 10, 64); err != nil {
			panic(err)
		}
	}
	switch fields[0] {
	case "Read":
		b := make([]byte, arg)
		_, _ = r.Read(b)
		return encode(b)
	case "Int63":
		return encode(r.Int63())
	case "Float64":
		return encode(r.Float64())
	case "NormFloat64":
		return encode(r.NormFloat64())
	case "Uint64":
		return encode(r.Uint64())
	case "Int31n":
		return encode(r.Int31n(int32(arg)))
	case "Seed":
		r.Seed(arg)
		return ""
	}
	panic(call)
}

func floatsOut(c *RandomCall, v []float32) {
	d := newDigest()
	for i, x := range v {
		if i < 8 {
			c.First = append(c.First, encode(x))
		}
		d.u64(uint64(math.Float32bits(x)))
	}
	c.Len = len(v)
	c.Digest = d.String()
}

func matrixOut(c *RandomCall, m [][]float32) {
	var flat []float32
	for _, row := range m {
		if len(row) != int(c.Args[1]) {
			panic("ragged matrix")
		}
		flat = append(flat, row...)
	}
	floatsOut(c, flat)
	c.Len = len(m)
}

func intsOut[T int | int32](c *RandomCall, v []T) {
	d := newDigest()
	for i, x := range v {
		if i < 64 {
			c.First = append(c.First, encode(int64(x)))
		}
		d.u64(uint64(int64(x)))
	}
	c.Len = len(v)
	c.Digest = d.String()
}

func randomGenerator(s *Section) {
	type spec struct {
		method  string
		args    []float64
		exclude [][]int64
	}
	excl04 := [][]int64{{0, 1, 2, 3, 4}}
	script := []spec{
		{"NormalMatrix", []float64{1, 1000, 1, 2}, nil},
		{"UniformMatrix", []float64{1, 1000, 1, 2}, nil},
	}
	for i := 1; i <= 10; i++ {
		script = append(script, spec{"Sample", []float64{0, 10, float64(i)}, excl04})
	}
	for i := 1; i <= 10; i++ {
		script = append(script, spec{"SampleInt32", []float64{0, 10, float64(i)}, excl04})
	}
	script = append(script,
		spec{"UniformVector", []float64{0, 0, 1}, nil},
		spec{"UniformVector", []float64{1, 0, 1}, nil},
		spec{"UniformVector", []float64{1000, -1, 1}, nil},
		spec{"UniformVector", []float64{1000, -0.01, 0.01}, nil},
		spec{"NormalVector", []float64{0, 0, 1}, nil},
		spec{"NormalVector", []float64{10, 0, 1}, nil},
		spec{"NormalVector", []float64{1000, 0, 0.01}, nil},
		spec{"NormalVector", []float64{100000, 1, 2}, nil},
		spec{"NormalVector64", []float64{100000, 0, 1}, nil},
		spec{"NormalVector64", []float64{10, -3, 0.5}, nil},
		spec{"NormalMatrix", []float64{50, 16, 0, 0.01}, nil},
		spec{"NormalMatrix", []float64{0, 16, 0, 0.01}, nil},
		spec{"UniformMatrix", []float64{20, 7, -0.5, 0.5}, nil},
		spec{"Sample", []float64{0, 100, 10}, nil},
		spec{"Sample", []float64{5, 50, 30}, [][]int64{{6, 7, 100, -1}}},
		spec{"Sample", []float64{5, 50, 38}, [][]int64{{6, 7, 100, -1}}},
		spec{"Sample", []float64{5, 50, 39}, [][]int64{{6, 7, 100, -1}}},
		// Go counts the out-of-range exclusions too, so these enumerate.
		spec{"Sample", []float64{5, 50, 41}, [][]int64{{6, 7, 100, -1}}},
		spec{"Sample", []float64{5, 50, 42}, [][]int64{{6, 7, 100, -1}}},
		spec{"Sample", []float64{0, 10, 10}, nil},
		spec{"Sample", []float64{0, 10, 0}, nil},
		spec{"Sample", []float64{-5, 5, 3}, nil},
		spec{"Sample", []float64{0, 1000000, 100}, [][]int64{{1, 2, 3}, {3, 4, 5}, {999999}}},
		spec{"Sample", []float64{0, 1000, 999}, [][]int64{{10}, {}}},
		spec{"SampleInt32", []float64{0, 100, 10}, nil},
		spec{"SampleInt32", []float64{5, 50, 30}, [][]int64{{6, 7, 100, -1}}},
		spec{"SampleInt32", []float64{5, 50, 38}, [][]int64{{6, 7, 100, -1}}},
		spec{"SampleInt32", []float64{5, 50, 41}, [][]int64{{6, 7, 100, -1}}},
		spec{"SampleInt32", []float64{5, 50, 42}, [][]int64{{6, 7, 100, -1}}},
		spec{"SampleInt32", []float64{0, 1000000, 100}, [][]int64{{1, 2, 3}, {3, 4, 5}, {999999}}},
		spec{"SampleInt32", []float64{-5, 5, 3}, nil},
		spec{"SampleInt32", []float64{0, 2147483647, 50}, nil},
		spec{"Int63", nil, nil},
		spec{"Intn", []float64{17}, nil},
		spec{"Int31n", []float64{1000}, nil},
	)
	for _, seed := range []int64{0, 1, 42, -7} {
		rc := RandomCase{Seed: seed}
		rng := util.NewRandomGenerator(seed)
		for _, sp := range script {
			c := RandomCall{Method: sp.method, Args: sp.args, Exclude: sp.exclude}
			a := sp.args
			switch sp.method {
			case "UniformVector":
				floatsOut(&c, rng.UniformVector(int(a[0]), float32(a[1]), float32(a[2])))
			case "NormalVector":
				floatsOut(&c, rng.NormalVector(int(a[0]), float32(a[1]), float32(a[2])))
			case "NormalVector64":
				v := rng.NormalVector64(int(a[0]), a[1], a[2])
				d := newDigest()
				for i, x := range v {
					if i < 8 {
						c.First = append(c.First, encode(x))
					}
					d.u64(math.Float64bits(x))
				}
				c.Len = len(v)
				c.Digest = d.String()
			case "NormalMatrix":
				matrixOut(&c, rng.NormalMatrix(int(a[0]), int(a[1]), float32(a[2]), float32(a[3])))
			case "UniformMatrix":
				matrixOut(&c, rng.UniformMatrix(int(a[0]), int(a[1]), float32(a[2]), float32(a[3])))
			case "Sample":
				var sets []mapset.Set[int]
				for _, e := range sp.exclude {
					set := mapset.NewSet[int]()
					for _, v := range e {
						set.Add(int(v))
					}
					sets = append(sets, set)
				}
				intsOut(&c, rng.Sample(int(a[0]), int(a[1]), int(a[2]), sets...))
			case "SampleInt32":
				var sets []mapset.Set[int32]
				for _, e := range sp.exclude {
					set := mapset.NewSet[int32]()
					for _, v := range e {
						set.Add(int32(v))
					}
					sets = append(sets, set)
				}
				intsOut(&c, rng.SampleInt32(int32(a[0]), int32(a[1]), int(a[2]), sets...))
			case "Int63":
				c.First = []string{encode(rng.Int63())}
				c.Len = 1
			case "Intn":
				c.First = []string{encode(rng.Intn(int(a[0])))}
				c.Len = 1
			case "Int31n":
				c.First = []string{encode(rng.Int31n(int32(a[0])))}
				c.Len = 1
			}
			rc.Calls = append(rc.Calls, c)
		}
		s.Random = append(s.Random, rc)
	}
}

// locked covers util.NewRand, whose source does not implement Source64.
func locked(s *Section) {
	for _, seed := range []int64{0, 1, -1, 42} {
		for _, m := range methods {
			for _, arg := range m.args {
				st := stream(seed, m, arg, util.NewRand(seed))
				s.Locked = append(s.Locked, st)
			}
		}
	}
}

func section() *Child {
	m := &Math{}
	mathSection(m)
	const rn = 3.442619855899
	s := &Section{InvRn: hx(math.Float64bits(1.0 / rn))}
	regress(s)
	streams(s)
	mixed(s)
	randomGenerator(s)
	locked(s)
	return &Child{Math: m, Rand: s}
}

// tables reads the math/rand tables from the toolchain's source files.
func tables(out string) {
	dir := filepath.Join(runtime.GOROOT(), "src", "math", "rand")
	type table struct{ file, name, kind string }
	list := []table{
		{"rng.go", "rngCooked", "i64"},
		{"normal.go", "kn", "u32"},
		{"normal.go", "wn", "f32"},
		{"normal.go", "fn", "f32"},
		{"exp.go", "ke", "u32"},
		{"exp.go", "we", "f32"},
		{"exp.go", "fe", "f32"},
	}
	rustName := map[string]string{"rngCooked": "RNG_COOKED", "kn": "KN", "wn": "WN", "fn": "FN", "ke": "KE", "we": "WE", "fe": "FE"}
	var b bytes.Buffer
	fmt.Fprintf(&b, "// Ports: math/rand rng.go (rngCooked), normal.go (kn, wn, fn), exp.go (ke, we, fe) @ %s\n", runtime.Version())
	fmt.Fprintf(&b, "// Code generated by contracts/tools/randgolden -tables-out; DO NOT EDIT.\n\n")
	fmt.Fprintf(&b, "//! Seeding and ziggurat tables of Go's `math/rand`, evaluated from the Go\n")
	fmt.Fprintf(&b, "//! source with `go/constant`; floats are their exact `float32` bits.\n\n")
	fset := token.NewFileSet()
	for _, t := range list {
		f, err := parser.ParseFile(fset, filepath.Join(dir, t.file), nil, 0)
		if err != nil {
			log.Fatal(err)
		}
		var values []constant.Value
		ast.Inspect(f, func(n ast.Node) bool {
			vs, ok := n.(*ast.ValueSpec)
			if !ok || len(vs.Names) != 1 || vs.Names[0].Name != t.name {
				return true
			}
			lit := vs.Values[0].(*ast.CompositeLit)
			for _, e := range lit.Elts {
				values = append(values, eval(e))
			}
			return false
		})
		if len(values) == 0 {
			log.Fatalf("table %s not found in %s", t.name, t.file)
		}
		rust := map[string]string{"i64": "i64", "u32": "u32", "f32": "f32"}[t.kind]
		fmt.Fprintf(&b, "/// `%s` from `%s`.\npub(super) const %s: [%s; %d] = [\n", t.name, t.file, rustName[t.name], rust, len(values))
		for _, v := range values {
			switch t.kind {
			case "i64":
				x, exact := constant.Int64Val(v)
				if !exact {
					log.Fatalf("%s: %v is not an int64", t.name, v)
				}
				fmt.Fprintf(&b, "    %d,\n", x)
			case "u32":
				x, exact := constant.Uint64Val(v)
				if !exact || x > math.MaxUint32 {
					log.Fatalf("%s: %v is not a uint32", t.name, v)
				}
				fmt.Fprintf(&b, "    0x%08x,\n", x)
			case "f32":
				x, _ := constant.Float32Val(v)
				fmt.Fprintf(&b, "    f32::from_bits(0x%08x),\n", math.Float32bits(x))
			}
		}
		fmt.Fprintf(&b, "];\n\n")
	}
	if err := os.WriteFile(out, bytes.TrimRight(b.Bytes(), "\n"), 0o644); err != nil {
		log.Fatal(err)
	}
	if f, err := os.OpenFile(out, os.O_APPEND|os.O_WRONLY, 0o644); err == nil {
		_, _ = f.WriteString("\n")
		_ = f.Close()
	}
}

func eval(e ast.Expr) constant.Value {
	switch x := e.(type) {
	case *ast.BasicLit:
		return constant.MakeFromLiteral(x.Value, x.Kind, 0)
	case *ast.UnaryExpr:
		return constant.UnaryOp(x.Op, eval(x.X), 0)
	}
	log.Fatalf("unexpected table element %T", e)
	return nil
}

func main() {
	tablesOut := flag.String("tables-out", "", "write the math/rand tables as Rust to this file")
	out := flag.String("out", "", "write the golden JSON to this file")
	child := flag.Bool("section", false, "print one section as JSON (used for the GODEBUG=cpu.fma=off child)")
	flag.Parse()
	if *child {
		if err := json.NewEncoder(os.Stdout).Encode(section()); err != nil {
			log.Fatal(err)
		}
		return
	}
	if *tablesOut != "" {
		tables(*tablesOut)
	}
	if *out == "" {
		return
	}
	if !(cpu.X86.HasAVX && cpu.X86.HasFMA) {
		log.Fatal("generate on an amd64 machine with AVX and FMA, so both math.Exp paths are recorded")
	}
	fma := section()
	cmd := exec.Command(os.Args[0], "-section")
	cmd.Env = append(os.Environ(), "GODEBUG=cpu.fma=off")
	cmd.Stderr = os.Stderr
	stdout, err := cmd.Output()
	if err != nil {
		log.Fatal(err)
	}
	var nofma Child
	if err = json.Unmarshal(stdout, &nofma); err != nil {
		log.Fatal(err)
	}
	if reflect.DeepEqual(fma.Math, nofma.Math) {
		log.Fatal("math.Exp gave the same outputs with and without FMA; is GODEBUG=cpu.fma=off honoured?")
	}
	a, err := json.Marshal(fma.Rand)
	if err != nil {
		log.Fatal(err)
	}
	b, err := json.Marshal(nofma.Rand)
	if err != nil {
		log.Fatal(err)
	}
	if !bytes.Equal(a, b) {
		log.Fatal("the generators gave different outputs with and without FMA; record both in the golden file")
	}
	g := Golden{GoVersion: runtime.Version(), Math: map[string]*Math{"fma": fma.Math, "nofma": nofma.Math}, Rand: fma.Rand}
	data, err := json.MarshalIndent(g, "", " ")
	if err != nil {
		log.Fatal(err)
	}
	if err = os.WriteFile(*out, append(data, '\n'), 0o644); err != nil {
		log.Fatal(err)
	}
}
