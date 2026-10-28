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

// Command ratelimitgolden writes golden data produced by
// github.com/juju/ratelimit v1.0.2 and Gorse's common/parallel/ratelimit.go
// for the Rust ports (crates/gorse-ratelimit, and
// crates/gorse-common/src/parallel/ratelimit.rs):
//
//	go run ./contracts/tools/ratelimitgolden \
//	    -out crates/gorse-ratelimit/tests/data/ratelimit_golden.json \
//	    -gorse-out crates/gorse-common/tests/data/parallel_ratelimit_golden.json
//
// -out holds:
//
//   - scripts: buckets made with NewBucketWithQuantumAndClock on a fake clock,
//     then sequences of clock moves (forwards and backwards) and Take,
//     TakeMaxDuration, TakeAvailable, Available, Wait and WaitMaxDuration
//     calls, with each result and the clock afterwards (Wait sleeps on the
//     fake clock);
//   - constructors: NewBucketWithQuantum arguments and the panic, if any;
//   - rates: NewBucketWithRateAndClock rates (as float64 bits) and the panic,
//     or the bucket's Rate(), fill interval and quantum, found by emptying a
//     bucket of capacity 1<<62 and timing one more token;
//   - sweep: a SHA-256 over the same for every rate juju's own (never run)
//     NewBucketWithRate test tries: 1, 8, 15, ... below 1e6;
//   - formats: strconv.FormatFloat(f, 'g', -1, 64), which the rate panic
//     message uses.
//
// -gorse-out holds sequences of InitChatCompletionLimiters and
// InitEmbeddingLimiters calls, with the panic, if any, and the package
// variables after each call: the backoff, and each limiter as "unlimited" or
// as a bucket's capacity and rate.
//
// Floats are written as the hex of their float64 bits.
package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"math/rand/v2"
	"os"
	"strconv"
	"time"

	"github.com/gorse-io/gorse/common/parallel"
	"github.com/juju/ratelimit"
)

// fakeClock is a ratelimit.Clock whose time moves only when told to.
type fakeClock struct {
	start time.Time
	now   time.Time
}

func newFakeClock() *fakeClock {
	now := time.Now()
	return &fakeClock{start: now, now: now}
}

func (c *fakeClock) Now() time.Time { return c.now }

func (c *fakeClock) Sleep(d time.Duration) { c.now = c.now.Add(d) }

func (c *fakeClock) offset() int64 { return int64(c.now.Sub(c.start)) }

// Op is one clock move and one bucket call.
type Op struct {
	// Advance moves the clock (nanoseconds, may be negative) before the call.
	Advance int64  `json:"advance"`
	Op      string `json:"op"`
	Count   int64  `json:"count"`
	MaxWait int64  `json:"max_wait"`
	// Result is the wait (take, take_max_duration), the tokens taken
	// (take_available) or available (available); Ok is the second result of
	// take_max_duration and wait_max_duration.
	Result int64 `json:"result"`
	Ok     bool  `json:"ok"`
	// Clock is the clock after the call, from the bucket's start.
	Clock int64 `json:"clock"`
}

// Script is a bucket and the calls made on it.
type Script struct {
	FillInterval int64  `json:"fill_interval"`
	Capacity     int64  `json:"capacity"`
	Quantum      int64  `json:"quantum"`
	Rate         string `json:"rate"`
	Ops          []Op   `json:"ops"`
}

// Constructor is one NewBucketWithQuantum call.
type Constructor struct {
	FillInterval int64  `json:"fill_interval"`
	Capacity     int64  `json:"capacity"`
	Quantum      int64  `json:"quantum"`
	Panic        string `json:"panic"`
}

// RateCase is one NewBucketWithRate call.
type RateCase struct {
	Rate         string `json:"rate"`
	Capacity     int64  `json:"capacity"`
	Panic        string `json:"panic"`
	RateOut      string `json:"rate_out"`
	FillInterval int64  `json:"fill_interval"`
	Quantum      int64  `json:"quantum"`
}

// Sweep digests the rates start, start+step, ... below end.
type Sweep struct {
	Start  float64 `json:"start"`
	Step   float64 `json:"step"`
	End    float64 `json:"end"`
	Count  int     `json:"count"`
	SHA256 string  `json:"sha256"`
}

// Format is one FormatFloat(f, 'g', -1, 64).
type Format struct {
	Bits string `json:"bits"`
	Text string `json:"text"`
}

// Golden is the -out file.
type Golden struct {
	Scripts      []Script      `json:"scripts"`
	Constructors []Constructor `json:"constructors"`
	Rates        []RateCase    `json:"rates"`
	Sweep        Sweep         `json:"sweep"`
	Formats      []Format      `json:"formats"`
}

// Limiter is a limiter variable: "unlimited", or a bucket.
type Limiter struct {
	Kind     string `json:"kind"`
	Capacity int64  `json:"capacity"`
	Rate     string `json:"rate"`
}

// InitCall is one Init*Limiters call and the variables after it.
type InitCall struct {
	RPM      int     `json:"rpm"`
	TPM      int     `json:"tpm"`
	Panic    string  `json:"panic"`
	Backoff  int64   `json:"backoff"`
	Requests Limiter `json:"requests"`
	Tokens   Limiter `json:"tokens"`
}

// GorseGolden is the -gorse-out file.
type GorseGolden struct {
	ChatCompletion []InitCall `json:"chat_completion"`
	Embedding      []InitCall `json:"embedding"`
}

func bits(f float64) string {
	return strconv.FormatUint(math.Float64bits(f), 16)
}

// catch runs f and returns the text of its panic, or "".
func catch(f func()) (msg string) {
	defer func() {
		if r := recover(); r != nil {
			msg = fmt.Sprint(r)
		}
	}()
	f()
	return ""
}

func pick[T any](rng *rand.Rand, xs ...T) T {
	return xs[rng.IntN(len(xs))]
}

func script(rng *rand.Rand, fillInterval, capacity, quantum int64) Script {
	clock := newFakeClock()
	tb := ratelimit.NewBucketWithQuantumAndClock(time.Duration(fillInterval), capacity, quantum, clock)
	s := Script{FillInterval: fillInterval, Capacity: capacity, Quantum: quantum, Rate: bits(tb.Rate())}
	fi := fillInterval
	if fi > 1<<40 {
		fi = 1 << 40 // keeps the clock moves within a few years
	}
	for range 40 {
		var advance int64
		switch rng.IntN(6) {
		case 0:
			advance = 0
		case 1:
			advance = rng.Int64N(2*fi + 1)
		case 2:
			advance = rng.Int64N(100*fi + 1)
		case 3:
			advance = rng.Int64N(1_000_000_000_000_000)
		case 4:
			// backwards, but never more than 5s before the start
			advance = -rng.Int64N(min(fi, 5_000_000_000) + 1)
			if clock.offset()+advance < -5_000_000_000 {
				advance = 0
			}
		default:
			advance = fi
		}
		clock.now = clock.now.Add(time.Duration(advance))
		op := Op{
			Advance: advance,
			Op:      pick(rng, "take", "take_max_duration", "take_available", "available", "wait", "wait_max_duration"),
			Count: pick(rng, 0, -1, 1, 1+rng.Int64N(20), capacity, capacity+1, quantum, 1<<62,
				math.MaxInt64, math.MinInt64, 1+rng.Int64N(1_000_000)),
			MaxWait: pick(rng, 0, 1, fillInterval, math.MaxInt64, rng.Int64N(10*fi+1), -1),
		}
		switch op.Op {
		case "take":
			op.Result = int64(tb.Take(op.Count))
		case "take_max_duration":
			d, ok := tb.TakeMaxDuration(op.Count, time.Duration(op.MaxWait))
			op.Result, op.Ok = int64(d), ok
		case "take_available":
			op.Result = tb.TakeAvailable(op.Count)
		case "available":
			op.Result = tb.Available()
		case "wait":
			tb.Wait(op.Count)
		case "wait_max_duration":
			op.Ok = tb.WaitMaxDuration(op.Count, time.Duration(op.MaxWait))
		}
		op.Clock = clock.offset()
		s.Ops = append(s.Ops, op)
	}
	return s
}

func scripts(rng *rand.Rand) []Script {
	const ms = int64(time.Millisecond)
	fixed := [][3]int64{
		{250 * ms, 10, 1},
		{ms, 10, 1},
		{10 * ms, 5, 1},
		{int64(time.Second), 100, 20},
		{int64(time.Second), 2, 2},
		{int64(time.Second), 3, 3},
		{1, 1, 1},
		{1, 1 << 62, 1 << 40},
		{math.MaxInt64, 1, 1},
		{7, math.MaxInt64, math.MaxInt64},
		{int64(time.Second), 1 << 62, 1},
	}
	var out []Script
	for _, f := range fixed {
		out = append(out, script(rng, f[0], f[1], f[2]))
	}
	for range 200 {
		fillInterval := pick(rng, 1, 3, 1000, ms, 250*ms, int64(time.Second), int64(time.Hour),
			1+rng.Int64N(1_000_000_000_000))
		capacity := pick(rng, 1, 2, 10, 1000, 1<<62, math.MaxInt64, 1+rng.Int64N(1_000_000))
		quantum := pick(rng, 1, 2, 5, 20, 1+rng.Int64N(1_000_000), 1<<40, math.MaxInt64)
		out = append(out, script(rng, fillInterval, capacity, quantum))
	}
	return out
}

func constructors() []Constructor {
	var out []Constructor
	for _, fi := range []int64{math.MinInt64, -2, 0, 1, math.MaxInt64} {
		for _, capacity := range []int64{math.MinInt64, -2, 0, 1, math.MaxInt64} {
			for _, quantum := range []int64{math.MinInt64, -1, 0, 1, math.MaxInt64} {
				c := Constructor{FillInterval: fi, Capacity: capacity, Quantum: quantum}
				c.Panic = catch(func() {
					ratelimit.NewBucketWithQuantum(time.Duration(fi), capacity, quantum)
				})
				out = append(out, c)
			}
		}
	}
	return out
}

// probe finds the fill interval and quantum of a full bucket of capacity
// 1<<62 on clock: empty it, take one more token (the wait is one fill
// interval), move to the first tick and count the tokens.
func probe(tb *ratelimit.Bucket, clock *fakeClock) (fillInterval, quantum int64) {
	tb.TakeAvailable(1 << 62)
	fillInterval = int64(tb.Take(1))
	clock.now = clock.now.Add(time.Duration(fillInterval))
	quantum = tb.Available() + 1
	return fillInterval, quantum
}

func rateCase(rate float64, capacity int64) RateCase {
	c := RateCase{Rate: bits(rate), Capacity: capacity}
	clock := newFakeClock()
	var tb *ratelimit.Bucket
	c.Panic = catch(func() { tb = ratelimit.NewBucketWithRateAndClock(rate, capacity, clock) })
	if c.Panic == "" {
		c.RateOut = bits(tb.Rate())
		if capacity == 1<<62 {
			c.FillInterval, c.Quantum = probe(tb, clock)
		}
	}
	return c
}

func rates(rng *rand.Rand) []RateCase {
	var out []RateCase
	juju := []float64{1024 * 1024 * 1024, 1e-5, 0.9e-5, 0.5, 0.9, 0.9e8, 3e12, 4e18, float64(1<<63 - 1)}
	for _, r := range juju {
		for _, rate := range []float64{r, r / 3, r * 1.3} {
			out = append(out, rateCase(rate, 1<<62))
		}
	}
	special := []float64{
		0, math.Copysign(0, -1), -1, -1e-300, math.NaN(), math.Inf(1), math.Inf(-1),
		1e24, 1.1e24, 1.2e24, 1e25, 5e-324, 1e-30, 2.2250738585072014e-308, math.MaxFloat64,
		1e9 / 9.25e18, 1e9 / 9.2e18, 1e9 / 9.223372036854775e18, 1e-10, 1e-9, 1e-8, 1, 1e9, 1e10,
	}
	for _, rate := range special {
		out = append(out, rateCase(rate, 1<<62))
	}
	for _, capacity := range []int64{math.MinInt64, -1, 0, 1, 100} {
		out = append(out, rateCase(1000, capacity))
		out = append(out, rateCase(-1, capacity))
	}
	for range 300 {
		// log-uniform from 1e-12 to 1e26
		out = append(out, rateCase(math.Pow(10, -12+38*rng.Float64()), 1<<62))
	}
	for range 100 {
		out = append(out, rateCase(1e6*rng.Float64(), 1<<62))
	}
	for range 100 {
		// any finite or infinite bit pattern, NaNs too
		out = append(out, rateCase(math.Float64frombits(rng.Uint64()), 1<<62))
	}
	return out
}

func sweep() Sweep {
	s := Sweep{Start: 1, Step: 7, End: 1e6}
	h := sha256.New()
	for rate := s.Start; rate < s.End; rate += s.Step {
		c := rateCase(rate, 1<<62)
		fmt.Fprintf(h, "%s %s %s %d %d\n", c.Rate, c.Panic, c.RateOut, c.FillInterval, c.Quantum)
		s.Count++
	}
	s.SHA256 = hex.EncodeToString(h.Sum(nil))
	return s
}

func formats(rng *rand.Rand) []Format {
	var fs []float64
	for e := -324; e <= 308; e++ {
		f, _ := strconv.ParseFloat("1e"+strconv.Itoa(e), 64)
		fs = append(fs, f, -f, 1.5*f, 9.999999*f)
	}
	fs = append(fs, 0, math.Copysign(0, -1), math.NaN(), math.Inf(1), math.Inf(-1),
		1e-5, 9.9999e-5, 1e-4, 0.00012345, 999999, 1e6, 123456.7, 1234567, 100, 120, 0.1, 1.0/3,
		math.MaxFloat64, math.SmallestNonzeroFloat64, 2.2250738585072014e-308, float64(1<<53), float64(1<<63-1))
	for range 2000 {
		fs = append(fs, math.Float64frombits(rng.Uint64()))
	}
	for range 500 {
		fs = append(fs, float64(rng.Int64N(10_000_000))/float64(pick(rng, 1, 10, 100, 1000, 1_000_000)))
	}
	var out []Format
	for _, f := range fs {
		out = append(out, Format{Bits: bits(f), Text: strconv.FormatFloat(f, 'g', -1, 64)})
	}
	return out
}

func limiter(l parallel.RateLimiter) Limiter {
	switch l := l.(type) {
	case *parallel.Unlimited:
		return Limiter{Kind: "unlimited"}
	case *ratelimit.Bucket:
		return Limiter{Kind: "bucket", Capacity: l.Capacity(), Rate: bits(l.Rate())}
	default:
		panic(fmt.Sprintf("unexpected limiter %T", l))
	}
}

func gorse() GorseGolden {
	calls := [][2]int{
		{120, 180}, {0, 0}, {-1, -5}, {60, 60}, {61, 119}, {59, 0}, {0, 1}, {3600, 3_600_000},
		{1, 0}, {math.MaxInt64, math.MaxInt64}, {60, 0}, {1_000_000, 59}, {7200, 0}, {0, -60},
		{math.MinInt64, math.MinInt64}, {6000, 6000},
	}
	var g GorseGolden
	for _, c := range calls {
		call := InitCall{RPM: c[0], TPM: c[1]}
		call.Panic = catch(func() { parallel.InitChatCompletionLimiters(c[0], c[1]) })
		call.Backoff = int64(parallel.ChatCompletionBackoff)
		call.Requests = limiter(parallel.ChatCompletionRequestsLimiter)
		call.Tokens = limiter(parallel.ChatCompletionTokensLimiter)
		g.ChatCompletion = append(g.ChatCompletion, call)
	}
	// The embedding calls in another order, so the two sets differ.
	for i := range calls {
		c := calls[len(calls)-1-i]
		call := InitCall{RPM: c[0], TPM: c[1]}
		call.Panic = catch(func() { parallel.InitEmbeddingLimiters(c[0], c[1]) })
		call.Backoff = int64(parallel.EmbeddingBackoff)
		call.Requests = limiter(parallel.EmbeddingRequestsLimiter)
		call.Tokens = limiter(parallel.EmbeddingTokensLimiter)
		g.Embedding = append(g.Embedding, call)
	}
	return g
}

func write(path string, v any) {
	buf, err := json.MarshalIndent(v, "", " ")
	if err == nil {
		err = os.WriteFile(path, append(buf, '\n'), 0o644)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func main() {
	out := flag.String("out", "", "output JSON file for juju/ratelimit")
	gorseOut := flag.String("gorse-out", "", "output JSON file for common/parallel/ratelimit.go")
	flag.Parse()
	if *out == "" || *gorseOut == "" {
		fmt.Fprintln(os.Stderr, "missing -out or -gorse-out")
		os.Exit(2)
	}
	rng := rand.New(rand.NewPCG(16, 6))
	write(*out, Golden{
		Scripts:      scripts(rng),
		Constructors: constructors(),
		Rates:        rates(rng),
		Sweep:        sweep(),
		Formats:      formats(rng),
	})
	write(*gorseOut, gorse())
}
