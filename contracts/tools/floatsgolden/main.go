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

// Command floatsgolden records what common/floats computes, bit for bit, at
// each kernel level of an amd64 CPU, for the Rust port's tests.
//
// Every function runs on pseudo-random inputs of many lengths (so that the
// SIMD kernels' tails are exercised) drawn from a splitmix64 sequence that the
// Rust test reproduces. For each function and length the output bits are
// hashed with SHA-256 and the first 8 bytes are kept.
//
// The kernel level is chosen the way an operator would, with GODEBUG, in a
// child process per level:
//
//	avx512  (no GODEBUG)            AVX-512 kernels, F16C conversions
//	avx     GODEBUG=cpu.avx512f=off AVX kernels, F16C conversions
//	scalar  GODEBUG=cpu.avx=off     Go loops and x448/float16
//
// A level the CPU does not provide is an error, never silently replaced.
package main

import (
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"os"
	"os/exec"

	"github.com/gorse-io/gorse/common/floats"
	"github.com/klauspost/cpuid/v2"
	"golang.org/x/sys/cpu"
)

// rng is splitmix64. With specials set, its floats mix NaNs of random
// payloads and signs, quiet and signaling, infinities and zeros in with the
// ordinary values, so that which operand's NaN a result carries is checked.
type rng struct {
	state    uint64
	specials bool
}

func (r *rng) next() uint64 {
	r.state += 0x9e3779b97f4a7c15
	z := r.state
	z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9
	z = (z ^ (z >> 27)) * 0x94d049bb133111eb
	return z ^ (z >> 31)
}

// float returns a multiple of 2^-23 in [-1, 1), or with specials set, one
// time in two a special value.
func (r *rng) float() float32 {
	if r.specials {
		if k := r.next(); k%2 == 0 {
			sign := uint32(k>>8) & 0x80000000
			payload := uint32(k>>16) & 0x3fffff
			switch (k >> 1) % 4 {
			case 0: // quiet NaN
				return math.Float32frombits(sign | 0x7fc00000 | payload)
			case 1: // signaling NaN
				return math.Float32frombits(sign | 0x7f800000 | payload | 1)
			case 2:
				return math.Float32frombits(sign | 0x7f800000)
			default:
				return math.Float32frombits(sign)
			}
		}
	}
	return float32(int32(r.next()>>40)-(1<<23)) / (1 << 23)
}

func (r *rng) floats(n int) []float32 {
	v := make([]float32, n)
	for i := range v {
		v[i] = r.float()
	}
	return v
}

// wide returns values spread over the float16 range and beyond: subnormal,
// normal and overflowing magnitudes.
func (r *rng) wide(n int) []float32 {
	v := make([]float32, n)
	for i := range v {
		e := int(r.next()%46) - 28
		v[i] = float32(math.Ldexp(float64(r.float()), e))
	}
	return v
}

func (r *rng) halves(n int) []uint16 {
	v := make([]uint16, n)
	for i := range v {
		v[i] = uint16(r.next())
	}
	return v
}

// Lengths cover every tail of the 8- and 16-wide kernels and a few long
// vectors.
var lengths = func() []int {
	var l []int
	for n := 1; n <= 40; n++ {
		l = append(l, n)
	}
	return append(l, 63, 64, 65, 127, 128, 129, 255, 256, 257, 1000, 1023)
}()

type hasher struct{ b []byte }

func (h *hasher) f32(v ...float32) {
	for _, x := range v {
		h.b = binary.LittleEndian.AppendUint32(h.b, math.Float32bits(x))
	}
}

func (h *hasher) u16(v ...uint16) {
	for _, x := range v {
		h.b = binary.LittleEndian.AppendUint16(h.b, x)
	}
}

func (h *hasher) sum() string {
	s := sha256.Sum256(h.b)
	return hex.EncodeToString(s[:8])
}

// Result is the output of one function at one level: the hash per length, in
// the order of Lengths (or Shapes for mm), and whether it panics on empty
// input.
type Result struct {
	Hashes      []string `json:"hashes"`
	EmptyPanics bool     `json:"empty_panics"`
}

type fn struct {
	name string
	// run computes the function for length n with inputs from r.
	run func(r *rng, n int, h *hasher)
	// empty calls the function with empty slices.
	empty func()
}

// seed derives the input sequence of one function and length.
func seed(index, n int) *rng { return &rng{state: uint64(index)<<32 | uint64(n)} }

func functions() []fn {
	return []fn{
		{"dot", func(r *rng, n int, h *hasher) {
			h.f32(floats.Dot(r.floats(n), r.floats(n)))
		}, func() { floats.Dot(nil, nil) }},
		{"euclidean", func(r *rng, n int, h *hasher) {
			h.f32(floats.Euclidean(r.floats(n), r.floats(n)))
		}, func() { floats.Euclidean(nil, nil) }},
		{"mul_const_add_to", func(r *rng, n int, h *hasher) {
			a, b, c := r.floats(n), r.floats(n), r.float()
			dst := make([]float32, n)
			floats.MulConstAddTo(a, c, b, dst)
			h.f32(dst...)
		}, func() { floats.MulConstAddTo(nil, 1, nil, nil) }},
		{"mul_const_add", func(r *rng, n int, h *hasher) {
			a, dst, c := r.floats(n), r.floats(n), r.float()
			floats.MulConstAdd(a, c, dst)
			h.f32(dst...)
		}, func() { floats.MulConstAdd(nil, 1, nil) }},
		{"mul_const_to", func(r *rng, n int, h *hasher) {
			a, c := r.floats(n), r.float()
			dst := make([]float32, n)
			floats.MulConstTo(a, c, dst)
			h.f32(dst...)
		}, func() { floats.MulConstTo(nil, 1, nil) }},
		{"mul_const", func(r *rng, n int, h *hasher) {
			dst, c := r.floats(n), r.float()
			floats.MulConst(dst, c)
			h.f32(dst...)
		}, func() { floats.MulConst(nil, 1) }},
		{"add_const", func(r *rng, n int, h *hasher) {
			dst, c := r.floats(n), r.float()
			floats.AddConst(dst, c)
			h.f32(dst...)
		}, func() { floats.AddConst(nil, 1) }},
		{"sub", func(r *rng, n int, h *hasher) {
			dst, s := r.floats(n), r.floats(n)
			floats.Sub(dst, s)
			h.f32(dst...)
		}, func() { floats.Sub(nil, nil) }},
		{"sub_to", func(r *rng, n int, h *hasher) {
			a, b := r.floats(n), r.floats(n)
			dst := make([]float32, n)
			floats.SubTo(a, b, dst)
			h.f32(dst...)
		}, func() { floats.SubTo(nil, nil, nil) }},
		{"mul_to", func(r *rng, n int, h *hasher) {
			a, b := r.floats(n), r.floats(n)
			dst := make([]float32, n)
			floats.MulTo(a, b, dst)
			h.f32(dst...)
		}, func() { floats.MulTo(nil, nil, nil) }},
		{"div_to", func(r *rng, n int, h *hasher) {
			a, b := r.floats(n), r.floats(n)
			dst := make([]float32, n)
			floats.DivTo(a, b, dst)
			h.f32(dst...)
		}, func() { floats.DivTo(nil, nil, nil) }},
		{"sqrt_to", func(r *rng, n int, h *hasher) {
			a := r.floats(n)
			for i := range a {
				a[i] = float32(math.Abs(float64(a[i])))
			}
			dst := make([]float32, n)
			floats.SqrtTo(a, dst)
			h.f32(dst...)
		}, func() { floats.SqrtTo(nil, nil) }},
		{"from_float32", func(r *rng, n int, h *hasher) {
			h.u16(floats.FromFloat32(r.wide(n))...)
		}, func() { floats.FromFloat32(nil) }},
		{"to_float32", func(r *rng, n int, h *hasher) {
			h.f32(floats.ToFloat32(r.halves(n))...)
		}, func() { floats.ToFloat32(nil) }},
		{"add", func(r *rng, n int, h *hasher) {
			dst, s := r.floats(n), r.floats(n)
			floats.Add(dst, s)
			h.f32(dst...)
		}, func() { floats.Add(nil, nil) }},
		{"div", func(r *rng, n int, h *hasher) {
			dst, s := r.floats(n), r.floats(n)
			floats.Div(dst, s)
			h.f32(dst...)
		}, func() { floats.Div(nil, nil) }},
		{"mul_add_to", func(r *rng, n int, h *hasher) {
			a, b, c := r.floats(n), r.floats(n), r.floats(n)
			floats.MulAddTo(a, b, c)
			h.f32(c...)
		}, func() { floats.MulAddTo(nil, nil, nil) }},
		{"add_to", func(r *rng, n int, h *hasher) {
			a, b := r.floats(n), r.floats(n)
			dst := make([]float32, n)
			floats.AddTo(a, b, dst)
			h.f32(dst...)
		}, func() { floats.AddTo(nil, nil, nil) }},
		{"sqrt", func(r *rng, n int, h *hasher) {
			a := r.floats(n)
			for i := range a {
				a[i] = float32(math.Abs(float64(a[i])))
			}
			floats.Sqrt(a)
			h.f32(a...)
		}, func() { floats.Sqrt(nil) }},
	}
}

// Shapes are the (m, n, k) of the matrix products; leading dimensions are
// the packed ones, as common/nn passes them.
var Shapes = [][3]int{{1, 1, 1}, {2, 4, 3}, {3, 8, 5}, {4, 8, 16}, {5, 9, 7}, {7, 17, 33}, {8, 16, 8}, {13, 10, 5}, {16, 31, 12}}

func mm(transA, transB bool) fn {
	name := fmt.Sprintf("mm_%v_%v", transA, transB)
	return fn{name, func(r *rng, s int, h *hasher) {
		m, n, k := Shapes[s][0], Shapes[s][1], Shapes[s][2]
		a, b, c := r.floats(m*k), r.floats(k*n), r.floats(m*n)
		lda, ldb := k, n
		if transA {
			lda = m
		}
		if transB {
			ldb = k
		}
		floats.MM(transA, transB, m, n, k, a, lda, b, ldb, c, n)
		h.f32(c...)
	}, func() { floats.MM(transA, transB, 0, 0, 0, nil, 0, nil, 0, nil, 0) }}
}

func panics(f func()) (p bool) {
	defer func() {
		if recover() != nil {
			p = true
		}
	}()
	f()
	return false
}

// runLevel computes every function on ordinary inputs, and again, under the
// name "specials/<name>", on inputs with special values (seeds offset by 1000).
func runLevel() map[string]Result {
	out := map[string]Result{}
	for _, specials := range []bool{false, true} {
		prefix, offset := "", 0
		if specials {
			prefix, offset = "specials/", 1000
		}
		for i, f := range functions() {
			var res Result
			for _, n := range lengths {
				var h hasher
				r := seed(offset+i, n)
				r.specials = specials
				f.run(r, n, &h)
				res.Hashes = append(res.Hashes, h.sum())
			}
			res.EmptyPanics = panics(f.empty)
			out[prefix+f.name] = res
		}
		for j, t := range [][2]bool{{false, false}, {false, true}, {true, false}, {true, true}} {
			f := mm(t[0], t[1])
			var res Result
			for s := range Shapes {
				var h hasher
				r := seed(offset+100+j, s)
				r.specials = specials
				f.run(r, s, &h)
				res.Hashes = append(res.Hashes, h.sum())
			}
			res.EmptyPanics = panics(f.empty)
			out[prefix+f.name] = res
		}
	}
	return out
}

// level reports the kernel level in effect, as common/floats chooses it.
func level() string {
	switch {
	case cpu.X86.HasAVX && cpu.X86.HasFMA && cpu.X86.HasAVX512F:
		return "avx512"
	case cpu.X86.HasAVX:
		return "avx"
	default:
		return "scalar"
	}
}

// Golden is the file the Rust test reads.
type Golden struct {
	Lengths []int                        `json:"lengths"`
	Shapes  [][3]int                     `json:"shapes"`
	F16C    map[string]bool              `json:"f16c"`
	Levels  map[string]map[string]Result `json:"levels"`
}

func main() {
	out := flag.String("out", "", "output JSON file")
	child := flag.String("level", "", "internal: compute the level in effect and check it is this one")
	flag.Parse()
	if *child != "" {
		if got := level(); got != *child {
			fmt.Fprintf(os.Stderr, "level %s requested but %s in effect\n", *child, got)
			os.Exit(1)
		}
		data, err := json.Marshal(map[string]any{
			"f16c":    cpu.X86.HasAVX && cpuid.CPU.Supports(cpuid.F16C),
			"results": runLevel(),
		})
		if err != nil {
			panic(err)
		}
		_, _ = os.Stdout.Write(data)
		return
	}
	self, err := os.Executable()
	if err != nil {
		panic(err)
	}
	golden := Golden{Lengths: lengths, Shapes: Shapes, F16C: map[string]bool{}, Levels: map[string]map[string]Result{}}
	for _, l := range []struct{ name, godebug string }{
		{"avx512", ""}, {"avx", "cpu.avx512f=off"}, {"scalar", "cpu.avx=off"},
	} {
		cmd := exec.Command(self, "-level", l.name)
		cmd.Env = append(os.Environ(), "GODEBUG="+l.godebug)
		cmd.Stderr = os.Stderr
		data, err := cmd.Output()
		if err != nil {
			panic(fmt.Sprintf("level %s: %v", l.name, err))
		}
		var res struct {
			F16C    bool              `json:"f16c"`
			Results map[string]Result `json:"results"`
		}
		if err = json.Unmarshal(data, &res); err != nil {
			panic(err)
		}
		golden.F16C[l.name] = res.F16C
		golden.Levels[l.name] = res.Results
	}
	data, err := json.MarshalIndent(golden, "", " ")
	if err != nil {
		panic(err)
	}
	if err = os.WriteFile(*out, append(data, '\n'), 0o644); err != nil {
		panic(err)
	}
}
