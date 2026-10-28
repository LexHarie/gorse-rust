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

//go:build nngolden

// Command nngolden records what common/nn computes, bit for bit, at each
// kernel level of an amd64 CPU, for the Rust port's tests. Build and run it
// with run.sh.
//
// Each case builds tensors from a splitmix64 sequence (or literal values),
// runs operations, backward passes, optimizers, Save and Load, and records an
// ordered list of named values: tensors as their shape and the IEEE bits of
// their data (a SHA-256 digest for more than 64 values), panics as their
// message, errors as their text, and plain numbers.
//
// The kernel level is chosen with GODEBUG in a child process per level, as in
// floatsgolden:
//
//	avx512  (no GODEBUG)            AVX-512 kernels
//	avx     GODEBUG=cpu.avx512f=off AVX kernels
//	scalar  GODEBUG=cpu.avx=off     Go loops; math.Exp without FMA
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
	"strings"

	"github.com/gorse-io/gorse/common/nn"
	"golang.org/x/sys/cpu"
)

// rng is splitmix64.
type rng struct{ state uint64 }

func seq(seed uint64) *rng { return &rng{state: seed} }

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

func size(shape []int) int {
	n := 1
	for _, s := range shape {
		n *= s
	}
	return n
}

// gen returns n values of the sequence seed, times scale.
func gen(seed uint64, n int, scale float32) []float32 {
	r := seq(seed)
	v := make([]float32, n)
	for i := range v {
		v[i] = r.float() * scale
	}
	return v
}

// positive returns n values in [0.5, 1.5).
func positive(seed uint64, n int) []float32 {
	r := seq(seed)
	v := make([]float32, n)
	for i := range v {
		v[i] = 0.5 + float32(math.Abs(float64(r.float())))
	}
	return v
}

// labels returns n class indices below classes, as float32.
func labels(seed uint64, n, classes int) []float32 {
	r := seq(seed)
	v := make([]float32, n)
	for i := range v {
		v[i] = float32(r.next() % uint64(classes))
	}
	return v
}

// signs returns n values that are -1 or 1.
func signs(seed uint64, n int) []float32 {
	r := seq(seed)
	v := make([]float32, n)
	for i := range v {
		v[i] = float32(int(r.next()%2)*2 - 1)
	}
	return v
}

// tensor returns a tensor of the given shape with values of gen(seed).
func tensor(seed uint64, shape ...int) *nn.Tensor {
	return nn.NewTensor(gen(seed, size(shape), 1), shape...)
}

// scaled is tensor times scale.
func scaled(seed uint64, scale float32, shape ...int) *nn.Tensor {
	return nn.NewTensor(gen(seed, size(shape), scale), shape...)
}

func lit(data []float32, shape ...int) *nn.Tensor { return nn.NewTensor(data, shape...) }

func bits(b uint32) float32 { return math.Float32frombits(b) }

var nan = float32(math.NaN())
var inf = float32(math.Inf(1))

// setData overwrites a parameter's data in place, as tests write t.data.
func setData(t *nn.Tensor, seed uint64, scale float32) {
	copy(t.Data(), gen(seed, len(t.Data()), scale))
}

// floats encodes values as 8 hex digits of IEEE bits each, or as
// "sha256:<first 16 bytes of the digest of the little-endian bits>:<len>"
// for more than 64 values.
func floats(d []float32) string {
	if len(d) <= 64 {
		var b strings.Builder
		for _, x := range d {
			fmt.Fprintf(&b, "%08x", math.Float32bits(x))
		}
		return b.String()
	}
	buf := make([]byte, 0, 4*len(d))
	for _, x := range d {
		buf = binary.LittleEndian.AppendUint32(buf, math.Float32bits(x))
	}
	s := sha256.Sum256(buf)
	return fmt.Sprintf("sha256:%s:%d", hex.EncodeToString(s[:16]), len(d))
}

func shapeOf(t *nn.Tensor) []int {
	if t.Shape() == nil {
		return []int{}
	}
	return t.Shape()
}

func tensorValue(t *nn.Tensor) any {
	if t == nil {
		return nil
	}
	return map[string]any{"shape": shapeOf(t), "data": floats(t.Data())}
}

// catch runs f and returns {"panic": message} if it panics, nil otherwise.
func catch(f func()) (v any) {
	defer func() {
		if r := recover(); r != nil {
			v = map[string]any{"panic": fmt.Sprint(r)}
		}
	}()
	f()
	return nil
}

// errValue records an error's text. protobuf-go starts its messages with
// "proto:" and a space that is U+0020 or U+00A0 depending on a hash of the
// binary (internal/detrand), to discourage comparing them; the space is
// recorded as U+0020 whichever this binary got.
func errValue(err error) any {
	if err == nil {
		return nil
	}
	return map[string]any{"error": strings.ReplaceAll(err.Error(), "proto:\u00a0", "proto: ")}
}

// rec is the ordered list of named values of one case.
type rec struct{ entries [][2]any }

func (r *rec) add(name string, v any) { r.entries = append(r.entries, [2]any{name, v}) }

func (r *rec) t(name string, t *nn.Tensor) { r.add(name, tensorValue(t)) }

func (r *rec) p(name string, f func()) { r.add(name, catch(f)) }

type testCase struct {
	name string
	run  func(r *rec)
}

func allCases() []testCase {
	var cs []testCase
	cs = append(cs, opCases()...)
	cs = append(cs, graphCases()...)
	cs = append(cs, tensorCases()...)
	cs = append(cs, trainCases()...)
	cs = append(cs, saveLoadCases()...)
	return cs
}

func runLevel() map[string][][2]any {
	out := map[string][][2]any{}
	for _, c := range allCases() {
		if _, dup := out[c.name]; dup {
			panic("duplicate case " + c.name)
		}
		r := &rec{entries: [][2]any{}}
		c.run(r)
		out[c.name] = r.entries
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

func main() {
	out := flag.String("out", "", "output JSON file")
	child := flag.String("level", "", "internal: compute the level in effect and check it is this one")
	flag.Parse()
	if *child != "" {
		if got := level(); got != *child {
			fmt.Fprintf(os.Stderr, "level %s requested but %s in effect\n", *child, got)
			os.Exit(1)
		}
		data, err := json.Marshal(runLevel())
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
	golden := map[string]any{}
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
		var res map[string]any
		if err = json.Unmarshal(data, &res); err != nil {
			panic(err)
		}
		golden[l.name] = res
	}
	data, err := json.MarshalIndent(map[string]any{"levels": golden}, "", " ")
	if err != nil {
		panic(err)
	}
	if err = os.WriteFile(*out, append(data, '\n'), 0o644); err != nil {
		panic(err)
	}
}
