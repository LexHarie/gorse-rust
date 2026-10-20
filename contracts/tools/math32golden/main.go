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

// Command math32golden records what github.com/chewxy/math32 computes, bit for
// bit, for the Rust port's tests (crates/gorse-common/src/math32.rs):
//
//	go run ./contracts/tools/math32golden -out crates/gorse-common/tests/data/math32_golden.json
//
// Each function runs on a fixed list of edge values plus inputs drawn from a
// splitmix64 sequence, and the file records the inputs and outputs as IEEE
// bits. Every output is recorded twice:
//
//	asm    the default build: on amd64 math32's Exp, Log and Sqrt are assembly
//	noasm  a child process built with -tags noasm, where math32 uses its Go
//	       code for every function (what arm64 runs for Exp and Log)
//
// The file also holds, per variant, a digest of each unary function over all
// 2^32 inputs (see exhaustive), which the Rust port's ignored test
// math32_exhaustive recomputes.
//
// It must run on linux/amd64, the reference platform of the port.
package main

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"math"
	"os"
	"os/exec"
	"runtime"
	"strconv"
	"strings"
	"sync"

	"github.com/chewxy/math32"
)

// rng is splitmix64.
type rng struct{ state uint64 }

func (r *rng) next() uint64 {
	r.state += 0x9e3779b97f4a7c15
	z := r.state
	z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9
	z = (z ^ (z >> 27)) * 0x94d049bb133111eb
	return z ^ (z >> 31)
}

// bits returns any 32-bit pattern: NaNs, infinities, subnormals included.
func (r *rng) bits() float32 { return math.Float32frombits(uint32(r.next())) }

// uniform returns a value in [lo, hi).
func (r *rng) uniform(lo, hi float64) float32 {
	return float32(lo + (hi-lo)*float64(r.next()>>11)/(1<<53))
}

func (r *rng) bitsN(n int) []float32 {
	v := make([]float32, n)
	for i := range v {
		v[i] = r.bits()
	}
	return v
}

func (r *rng) uniformN(n int, lo, hi float64) []float32 {
	v := make([]float32, n)
	for i := range v {
		v[i] = r.uniform(lo, hi)
	}
	return v
}

func f(b uint32) float32 { return math.Float32frombits(b) }

// specials are edge values every unary function is checked on.
func specials() []float32 {
	v := []float32{
		0, f(0x80000000), float32(math.Inf(1)), float32(math.Inf(-1)),
		f(0x7FC00000), f(0x7FE00000), f(0xFFC00000), f(0x7F800001), f(0xFFFFFFFF), f(0x7FC00001),
		1, -1, 0.5, -0.5, 2, -2, 3, -3, 10, -10,
		f(1), f(0x80000001), f(0x007FFFFF), f(0x807FFFFF), f(0x00800000), f(0x80800000),
		f(0x7F7FFFFF), f(0xFF7FFFFF), 1e-40, -1e-40, 1e-30, 1e30, -1e30, 3e38,
		math32.Pi, -math32.Pi, math32.Pi / 2, math32.Pi / 4, 2 * math32.Pi,
		0.625, -0.625, f(0x3F1FFFFF), f(0xBF1FFFFF), 44.014847, -44.014847, 44.01484, f(0x42300F33), f(0x42300F34),
		88, 88.02969, 88.7, 88.72283, 89, 100, -87, -87.33, -88, -103, -104, -150, 709, 710, -745, -746,
		1e9, 1e10, -1e10, 1.4885e9, 1.48852e9, 1.488522e9, 1.4885222e9, 1.4886e9, -1.4886e9,
		float32(1 << 29), float32(1<<29) + 64, float32(1<<29) - 32, float32(1 << 31), float32(1 << 40), float32(1 << 60),
		f(0x3F3504F3), f(0x3F3504F2), f(0x3F3504F4), f(0x3FB504F3), f(0x003504F3), f(0x3EB504F3),
		0.9, 0.999, 1.0000001, 0.99999994, 1e-8, -1e-8, 3.7252903e-09, -3.7252903e-09,
	}
	// The inputs of math32's own tests (all_test.go).
	v = append(v,
		4.9790119248836735e+00, 7.7388724745781045e+00, -2.7688005719200159e-01, -5.0106036182710749e+00,
		9.6362937071984173e+00, 2.9263772392439646e+00, 5.2290834314593066e+00, 2.7279399104360102e+00,
		1.8253080916808550e+00, -8.6859247685756013e+00, -2000, 2000)
	return v
}

func concat(lists ...[]float32) []float32 {
	var out []float32
	for _, l := range lists {
		out = append(out, l...)
	}
	return out
}

func hexOf(v []float32) string {
	b := make([]byte, 0, len(v)*4)
	for _, x := range v {
		u := math.Float32bits(x)
		b = append(b, byte(u>>24), byte(u>>16), byte(u>>8), byte(u))
	}
	return hex.EncodeToString(b)
}

func intsOf(v []int) string {
	s := make([]string, len(v))
	for i, x := range v {
		s[i] = strconv.Itoa(x)
	}
	return strings.Join(s, " ")
}

// Case is one function over its inputs. Outputs are filled in per variant.
type Case struct {
	Fn string `json:"fn"`
	// X holds the first argument as IEEE bits, 8 hex digits per value.
	X string `json:"x"`
	// Y holds the second argument: IEEE bits for pow and copysign, decimal
	// exponents separated by spaces for ldexp.
	Y string `json:"y,omitempty"`
	// Asm and NoAsm hold the result bits per variant; frexp and modf write
	// "<fraction or integer part bits>|<exponents or fractional part bits>".
	Asm   string `json:"asm"`
	NoAsm string `json:"noasm"`

	xs, ys []float32
	es     []int
}

func unary(name string, xs []float32) *Case { return &Case{Fn: name, xs: xs} }

func cases() []*Case {
	r := &rng{state: 20261017}
	sp := specials()
	logBoundary := []float32{}
	for e := uint32(0); e < 255; e++ {
		for _, m := range []uint32{0x3504F2, 0x3504F3, 0x3504F4} {
			logBoundary = append(logBoundary, f(e<<23|m))
		}
	}
	positiveBits := func(n int) []float32 {
		v := r.bitsN(n)
		for i := range v {
			v[i] = f(math.Float32bits(v[i]) &^ (1 << 31))
		}
		return v
	}
	nearby := func(x float32, k int) []float32 {
		var v []float32
		b := math.Float32bits(x)
		for d := -k; d <= k; d++ {
			v = append(v, f(uint32(int64(b)+int64(d))))
		}
		return v
	}
	var cs []*Case
	cs = append(cs,
		unary("abs", concat(sp, r.bitsN(300))),
		unary("floor", concat(sp, r.bitsN(600), r.uniformN(300, -1e4, 1e4), r.uniformN(200, -3, 3))),
		unary("frexp", concat(sp, r.bitsN(600), r.uniformN(200, -1e4, 1e4))),
		unary("modf", concat(sp, r.bitsN(600), r.uniformN(300, -1e4, 1e4))),
		unary("sqrt", concat(sp, r.bitsN(600), positiveBits(600))),
		unary("exp", concat(sp, r.bitsN(600), r.uniformN(1200, -120, 120), r.uniformN(300, -1, 1),
			nearby(88.02969, 40), nearby(-87.33655, 40))),
		unary("log", concat(sp, r.bitsN(600), positiveBits(1200), logBoundary)),
		unary("sin", concat(sp, r.bitsN(600), r.uniformN(1000, -20, 20), r.uniformN(300, 1<<29, 1<<36))),
		unary("cos", concat(sp, r.bitsN(600), r.uniformN(1000, -20, 20), r.uniformN(300, 1<<29, 1<<36))),
		unary("tanh", concat(sp, r.bitsN(600), r.uniformN(1000, -12, 12), nearby(0.625, 20), nearby(-0.625, 20),
			nearby(44.014847, 20))),
	)

	// copysign: pairs of any bits.
	cp := &Case{Fn: "copysign", xs: concat(sp, r.bitsN(300))}
	cp.ys = r.bitsN(len(cp.xs))
	cs = append(cs, cp)

	// ldexp: any fraction with exponents around the float32 range.
	ld := &Case{Fn: "ldexp", xs: concat(sp, r.bitsN(800), r.uniformN(200, -2, 2))}
	for range ld.xs {
		ld.es = append(ld.es, int(r.next()%661)-330)
	}
	for _, e := range []int{0, -1075, 1024, -1024, -150, -149, -126, -23, -22, 128, 256, -(127 + 149), 1 << 40, -(1 << 40)} {
		for _, x := range []float32{1, -1, 0, f(0x80000000), f(1), f(0x00800000), f(0x007FFFFF), f(0x7F7FFFFF), 0.5, float32(math.Inf(1)), f(0x7FC00000)} {
			ld.xs = append(ld.xs, x)
			ld.es = append(ld.es, e)
		}
	}
	cs = append(cs, ld)

	// pow: pairs covering every special case, the integer and fractional
	// paths, and the exponents the Adam optimizer uses.
	pw := &Case{Fn: "pow"}
	add := func(x, y float32) {
		pw.xs = append(pw.xs, x)
		pw.ys = append(pw.ys, y)
	}
	edge := []float32{0, f(0x80000000), float32(math.Inf(1)), float32(math.Inf(-1)), f(0x7FC00000), 1, -1, 0.5, -0.5,
		2, -2, 3, -3, math32.Pi, -math32.Pi, 0.25, f(1), f(0x007FFFFF), f(0x7F7FFFFF), 1e10, 2147483648, 4294967296, 1.5, -1.5}
	for _, x := range edge {
		for _, y := range edge {
			add(x, y)
		}
	}
	for _, y := range specials()[:10] {
		add(10, y)
	}
	for _, v := range specials()[len(specials())-12:] {
		add(10, v)
	}
	for range 800 {
		add(r.uniform(0, 4), r.uniform(-8, 8))
	}
	for range 300 {
		add(r.uniform(-4, 4), float32(int(r.next()%41)-20))
	}
	for range 500 {
		add(r.bits(), r.bits())
	}
	for range 200 {
		add(r.uniform(0, 2), r.uniform(-1, 1))
	}
	for range 100 {
		add(r.uniform(0, 2), r.uniform(2147483648, 1e12))
	}
	for t := 1; t <= 1000; t++ {
		add(0.9, float32(t))
		add(0.999, float32(t))
	}
	cs = append(cs, pw)
	return cs
}

func evaluate(c *Case) string {
	switch c.Fn {
	case "frexp":
		fr := make([]float32, len(c.xs))
		ex := make([]int, len(c.xs))
		for i, x := range c.xs {
			fr[i], ex[i] = math32.Frexp(x)
		}
		return hexOf(fr) + "|" + intsOf(ex)
	case "modf":
		in := make([]float32, len(c.xs))
		fr := make([]float32, len(c.xs))
		for i, x := range c.xs {
			in[i], fr[i] = math32.Modf(x)
		}
		return hexOf(in) + "|" + hexOf(fr)
	}
	out := make([]float32, len(c.xs))
	for i, x := range c.xs {
		switch c.Fn {
		case "abs":
			out[i] = math32.Abs(x)
		case "floor":
			out[i] = math32.Floor(x)
		case "sqrt":
			out[i] = math32.Sqrt(x)
		case "exp":
			out[i] = math32.Exp(x)
		case "log":
			out[i] = math32.Log(x)
		case "sin":
			out[i] = math32.Sin(x)
		case "cos":
			out[i] = math32.Cos(x)
		case "tanh":
			out[i] = math32.Tanh(x)
		case "copysign":
			out[i] = math32.Copysign(x, c.ys[i])
		case "ldexp":
			out[i] = math32.Ldexp(x, c.es[i])
		case "pow":
			out[i] = math32.Pow(x, c.ys[i])
		default:
			log.Fatalf("unknown function %s", c.Fn)
		}
	}
	return hexOf(out)
}

// exhaustiveFns are the unary functions digested over every input. frexp and
// modf contribute two words per input: the fraction (or integer part) bits,
// then the exponent as uint32 (or the fractional part bits). int32 and int are
// Go's own conversions of a float32, which the port reproduces next to
// math32 (Go leaves out-of-range results to the implementation); int
// contributes its low then its high 32 bits.
var exhaustiveFns = []string{"abs", "floor", "frexp", "modf", "sqrt", "exp", "log", "sin", "cos", "tanh", "int32", "int"}

func words(name string, x float32, out []uint32) []uint32 {
	switch name {
	case "abs":
		return append(out, math.Float32bits(math32.Abs(x)))
	case "floor":
		return append(out, math.Float32bits(math32.Floor(x)))
	case "frexp":
		f, e := math32.Frexp(x)
		return append(out, math.Float32bits(f), uint32(e))
	case "modf":
		i, f := math32.Modf(x)
		return append(out, math.Float32bits(i), math.Float32bits(f))
	case "sqrt":
		return append(out, math.Float32bits(math32.Sqrt(x)))
	case "exp":
		return append(out, math.Float32bits(math32.Exp(x)))
	case "log":
		return append(out, math.Float32bits(math32.Log(x)))
	case "sin":
		return append(out, math.Float32bits(math32.Sin(x)))
	case "cos":
		return append(out, math.Float32bits(math32.Cos(x)))
	case "tanh":
		return append(out, math.Float32bits(math32.Tanh(x)))
	case "int32":
		return append(out, uint32(int32(x)))
	case "int":
		v := int(x)
		return append(out, uint32(v), uint32(v>>32))
	}
	log.Fatalf("unknown function %s", name)
	return nil
}

const (
	fnvOffset = 0xcbf29ce484222325
	fnvPrime  = 0x100000001b3
)

// exhaustive digests a function over all 2^32 inputs: FNV-1a (64-bit, one
// step per 32-bit word) over each block of 2^24 consecutive input bit
// patterns, then FNV-1a over the 256 block digests (each as two words, low
// half first).
func exhaustive(name string) string {
	var blocks [256]uint64
	var wg sync.WaitGroup
	next := make(chan int)
	for range runtime.NumCPU() {
		wg.Go(func() {
			buf := make([]uint32, 0, 2)
			for b := range next {
				h := uint64(fnvOffset)
				for i := uint32(0); i < 1<<24; i++ {
					buf = words(name, math.Float32frombits(uint32(b)<<24|i), buf[:0])
					for _, w := range buf {
						h ^= uint64(w)
						h *= fnvPrime
					}
				}
				blocks[b] = h
			}
		})
	}
	for b := range 256 {
		next <- b
	}
	close(next)
	wg.Wait()
	h := uint64(fnvOffset)
	for _, d := range blocks {
		for _, w := range []uint32{uint32(d), uint32(d >> 32)} {
			h ^= uint64(w)
			h *= fnvPrime
		}
	}
	return fmt.Sprintf("%016x", h)
}

// Output is what one build variant computes.
type Output struct {
	Results    []string          `json:"results"`
	Exhaustive map[string]string `json:"exhaustive"`
}

func compute(cs []*Case) Output {
	o := Output{Exhaustive: map[string]string{}}
	for _, c := range cs {
		o.Results = append(o.Results, evaluate(c))
	}
	for _, name := range exhaustiveFns {
		o.Exhaustive[name] = exhaustive(name)
	}
	return o
}

func main() {
	out := flag.String("out", "crates/gorse-common/tests/data/math32_golden.json", "output file")
	child := flag.Bool("child", false, "print this build's outputs as JSON (used for the noasm build)")
	flag.Parse()
	if runtime.GOOS != "linux" || runtime.GOARCH != "amd64" {
		log.Fatalf("run on linux/amd64, not %s/%s", runtime.GOOS, runtime.GOARCH)
	}

	cs := cases()
	if *child {
		if variant != "noasm" {
			log.Fatal("-child must run in the noasm build")
		}
		if err := json.NewEncoder(os.Stdout).Encode(compute(cs)); err != nil {
			log.Fatal(err)
		}
		return
	}
	if variant != "asm" {
		log.Fatal("run the default build, without -tags noasm")
	}
	asm := compute(cs)

	cmd := exec.Command("go", "run", "-tags", "noasm", "./contracts/tools/math32golden", "-child")
	cmd.Stderr = os.Stderr
	stdout, err := cmd.Output()
	if err != nil {
		log.Fatalf("noasm child: %v", err)
	}
	var noasm Output
	if err := json.Unmarshal(stdout, &noasm); err != nil {
		log.Fatal(err)
	}
	if len(noasm.Results) != len(cs) {
		log.Fatalf("noasm child returned %d results for %d cases", len(noasm.Results), len(cs))
	}

	var buf bytes.Buffer
	exh, err := json.Marshal(map[string]map[string]string{"asm": asm.Exhaustive, "noasm": noasm.Exhaustive})
	if err != nil {
		log.Fatal(err)
	}
	fmt.Fprintf(&buf, "{\"go\":%q,\"math32\":\"v1.11.2\",\n\"exhaustive\":%s,\n\"cases\":[\n", runtime.Version(), exh)
	for i, c := range cs {
		c.X = hexOf(c.xs)
		switch {
		case c.Fn == "ldexp":
			c.Y = intsOf(c.es)
		case c.ys != nil:
			c.Y = hexOf(c.ys)
		}
		c.Asm, c.NoAsm = asm.Results[i], noasm.Results[i]
		line, err := json.Marshal(c)
		if err != nil {
			log.Fatal(err)
		}
		buf.Write(line)
		if i < len(cs)-1 {
			buf.WriteString(",")
		}
		buf.WriteString("\n")
	}
	buf.WriteString("]}\n")
	if err := os.WriteFile(*out, buf.Bytes(), 0o644); err != nil {
		log.Fatal(err)
	}
}
