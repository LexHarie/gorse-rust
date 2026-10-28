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

// Command ftoagolden records what strconv.FormatFloat, FormatInt and
// FormatUint print, for the Rust port's tests.
//
// Each float is formatted in every format, at every precision in precs and
// at both bit sizes; the outputs, joined by newlines in that order, are hashed
// with SHA-256 and the first 8 bytes are kept. Each integer is formatted in
// every base from 2 to 36 the same way. The inputs are written out, so the
// Rust test needs no generator, and a mismatch names its input.
//
// The floats are special values, powers of two, values that tie when rounded
// (to a few digits, or to their shortest form), short decimals and random bit
// patterns from splitmix64.
package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"os"
	"strconv"
	"strings"
)

// The formats and precisions every float is printed with; '?' is unknown.
var (
	formats = []byte{'b', 'e', 'E', 'f', 'g', 'G', 'x', 'X', '?'}
	precs   = []int{-1, 0, 1, 2, 3, 4, 5, 6, 8, 10, 14, 15, 16, 17, 18, 19, 20, 25, 40}
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

func floats() []float64 {
	v := []float64{
		0, math.Copysign(0, -1), 1, -1, 0.5, 1.5, 2.5, -2.5, 0.125, 0.375,
		0.1, 0.2, 0.3, 1.0 / 3, 2.0 / 3, 100, 1e21, 1e20, 1e-6, 1e-7, 1e-5,
		1e23, 99999999999999974834176, 100000000000000008388608,
		math.MaxFloat64, -math.MaxFloat64, math.SmallestNonzeroFloat64,
		math.MaxFloat32, math.SmallestNonzeroFloat32, 0x1p-1022, 0x1p-126,
		math.Pi, math.E, 123456789, 0.000123456789, 9.5, 99.5, 0.95, 0.995,
		0.9999999, 999999.5, 9999999.5, 1e15 + 0.5, 4.35, 2.675, 1.005,
		math.NaN(), math.Inf(1), math.Inf(-1),
		math.Float64frombits(0x7ff0000000000001), math.Float64frombits(0xfff8000000000000),
		16777216, 16777217, 33554431, 3.4028235677973366e+38, 3.4028236e+38,
	}
	// Powers of two, both float sizes' ranges; every one near 1, where
	// the shortest form can tie.
	for e := -1075; e <= 1024; e += 7 {
		v = append(v, math.Ldexp(1, e))
	}
	for e := -160; e <= 160; e++ {
		v = append(v, math.Ldexp(1, e))
	}
	r := &rng{state: 20260419}
	// float32s of nine digits that end in 125 or 375, whose shortest form
	// ties between two eight-digit ones.
	for range 400 {
		n := 1<<20 | r.next()%(1<<24-1<<20) | 1
		v = append(v, float64(float32(n))/8)
	}
	// Ties: an odd multiple of 2^-k, which halves at some digit.
	for range 300 {
		k := int(r.next() % 12)
		n := int64(r.next()%2000000) | 1
		v = append(v, math.Ldexp(float64(n), -k-1)*float64(1-2*int64(r.next()%2)))
	}
	// Short decimals, as people write them.
	for range 600 {
		digits := strconv.FormatUint(10000000+r.next()%90000000, 10)[:1+r.next()%8]
		e := int(r.next()%60) - 30
		f, err := strconv.ParseFloat(digits+"e"+strconv.Itoa(e), 64)
		if err != nil {
			panic(err)
		}
		v = append(v, f)
	}
	// Random bit patterns over all floats, and over the float32 range.
	for range 1500 {
		v = append(v, math.Float64frombits(r.next()))
	}
	for range 600 {
		v = append(v, float64(math.Float32frombits(uint32(r.next()))))
	}
	return v
}

func ints() []int64 {
	v := []int64{0, 1, -1, 9, 10, 35, 36, -36, math.MaxInt64, math.MinInt64, math.MaxInt32, math.MinInt32}
	r := &rng{state: 1 << 40}
	for range 400 {
		v = append(v, int64(r.next())>>(r.next()%64))
	}
	return v
}

func digest(out []string) string {
	sum := sha256.Sum256([]byte(strings.Join(out, "\n")))
	return hex.EncodeToString(sum[:8])
}

type golden struct {
	Formats string     `json:"formats"`
	Precs   []int      `json:"precs"`
	Floats  [][]string `json:"floats"`
	Ints    [][]string `json:"ints"`
}

func main() {
	outPath := flag.String("out", "", "golden JSON to write")
	flag.Parse()
	if *outPath == "" {
		fmt.Fprintln(os.Stderr, "ftoagolden: -out is required")
		os.Exit(2)
	}
	g := golden{Formats: string(formats), Precs: precs}
	for _, f := range floats() {
		var out []string
		for _, bitSize := range []int{64, 32} {
			for _, fmtc := range formats {
				for _, prec := range precs {
					out = append(out, strconv.FormatFloat(f, fmtc, prec, bitSize))
				}
			}
		}
		g.Floats = append(g.Floats, []string{fmt.Sprintf("%016x", math.Float64bits(f)), digest(out)})
	}
	for _, i := range ints() {
		var out []string
		for base := 2; base <= 36; base++ {
			out = append(out, strconv.FormatInt(i, base), strconv.FormatUint(uint64(i), base))
		}
		g.Ints = append(g.Ints, []string{strconv.FormatInt(i, 10), digest(out)})
	}
	// One input per line.
	var b strings.Builder
	head, err := json.Marshal(struct {
		Formats string `json:"formats"`
		Precs   []int  `json:"precs"`
	}{g.Formats, g.Precs})
	if err != nil {
		panic(err)
	}
	b.Write(head[:len(head)-1])
	for _, list := range []struct {
		name    string
		entries [][]string
	}{{"floats", g.Floats}, {"ints", g.Ints}} {
		fmt.Fprintf(&b, ",\n\"%s\": [", list.name)
		for i, e := range list.entries {
			entry, err := json.Marshal(e)
			if err != nil {
				panic(err)
			}
			if i > 0 {
				b.WriteByte(',')
			}
			b.WriteString("\n  ")
			b.Write(entry)
		}
		b.WriteString("\n]")
	}
	b.WriteString("\n}\n")
	if err := os.WriteFile(*outPath, []byte(b.String()), 0o644); err != nil {
		fmt.Fprintln(os.Stderr, "ftoagolden:", err)
		os.Exit(1)
	}
}
