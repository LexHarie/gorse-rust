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

// Command tpegolden produces what the Rust port of goptuna's TPE sampler, and
// of Go's math.Erf, math.Max and math.Min, is checked against:
//
//   - erf: math.Erf over -6 to 6 in steps of 1/64, the grid the TPE sampler's
//     normal CDF reads;
//   - maxmin: math.Max and math.Min over pairs of special values;
//   - trials: a goptuna study of 60 trials with tpe.NewSampler and
//     tpe.SamplerOptionSeed(1), whose objective suggests every distribution
//     and records each suggestion and the objective value.
//
// goptuna's categorical sampling reads math/rand's global source, which Go
// seeds from the clock unless GODEBUG=randautoseed=0 is set; that setting seeds
// it with 1, which the Rust test reproduces. Run the generator with:
//
//	GODEBUG=randautoseed=0 go run ./contracts/tools/tpegolden -out crates/gorse-model/testdata/tpe/golden.json
//
// Numbers are written as the hex of their 64-bit pattern, as randgolden writes
// them. The objective uses only additions, subtractions and absolute values, so
// no fused multiply-add can change its result on any architecture.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"math"
	"os"

	"github.com/c-bata/goptuna"
	"github.com/c-bata/goptuna/tpe"
)

const (
	samplerSeed = 1
	trialCount  = 60
)

var choices = []string{"a", "b", "c"}

type golden struct {
	Erf    []erfCase    `json:"erf"`
	MaxMin []maxMinCase `json:"maxmin"`
	Trials []trialCase  `json:"trials"`
}

type erfCase struct {
	X   string `json:"x"`
	Erf string `json:"erf"`
}

type maxMinCase struct {
	A   string `json:"a"`
	B   string `json:"b"`
	Max string `json:"max"`
	Min string `json:"min"`
}

type trialCase struct {
	Number int    `json:"number"`
	X      string `json:"x"`
	LR     string `json:"lr"`
	Q      string `json:"q"`
	N      string `json:"n"`
	S      string `json:"s"`
	C      string `json:"c"`
	Value  string `json:"value"`
}

func bits(x float64) string {
	return fmt.Sprintf("0x%016x", math.Float64bits(x))
}

func absInt(x int) int {
	if x < 0 {
		return -x
	}
	return x
}

func erfCases() []erfCase {
	var cases []erfCase
	for i := -384; i <= 384; i++ {
		x := float64(i) / 64
		cases = append(cases, erfCase{X: bits(x), Erf: bits(math.Erf(x))})
	}
	return cases
}

func maxMinCases() []maxMinCase {
	special := []float64{
		math.Inf(-1), -2.5, math.Copysign(0, -1), 0, 1, 3, math.Inf(1), math.NaN(),
	}
	var cases []maxMinCase
	for _, a := range special {
		for _, b := range special {
			cases = append(cases, maxMinCase{
				A:   bits(a),
				B:   bits(b),
				Max: bits(math.Max(a, b)),
				Min: bits(math.Min(a, b)),
			})
		}
	}
	return cases
}

func trialCases() []trialCase {
	var cases []trialCase
	study, err := goptuna.CreateStudy("golden",
		goptuna.StudyOptionDirection(goptuna.StudyDirectionMinimize),
		goptuna.StudyOptionSampler(tpe.NewSampler(tpe.SamplerOptionSeed(samplerSeed))))
	if err != nil {
		log.Fatal(err)
	}
	err = study.Optimize(func(trial goptuna.Trial) (float64, error) {
		number, err := trial.Number()
		if err != nil {
			return 0, err
		}
		x, err := trial.SuggestFloat("x", -10, 10)
		if err != nil {
			return 0, err
		}
		lr, err := trial.SuggestLogFloat("lr", 1e-3, 10)
		if err != nil {
			return 0, err
		}
		q, err := trial.SuggestDiscreteFloat("q", 0, 1, 0.25)
		if err != nil {
			return 0, err
		}
		n, err := trial.SuggestInt("n", 1, 10)
		if err != nil {
			return 0, err
		}
		s, err := trial.SuggestStepInt("s", 0, 10, 2)
		if err != nil {
			return 0, err
		}
		c, err := trial.SuggestCategorical("c", choices)
		if err != nil {
			return 0, err
		}
		idx := 0
		for i, choice := range choices {
			if choice == c {
				idx = i
			}
		}
		value := math.Abs(x-1) + math.Abs(lr-0.5) + math.Abs(q-0.5) +
			float64(absInt(n-4)) + float64(s) + float64(idx)
		cases = append(cases, trialCase{
			Number: number,
			X:      bits(x),
			LR:     bits(lr),
			Q:      bits(q),
			N:      bits(float64(n)),
			S:      bits(float64(s)),
			C:      bits(float64(idx)),
			Value:  bits(value),
		})
		return value, nil
	}, trialCount)
	if err != nil {
		log.Fatal(err)
	}
	return cases
}

func main() {
	out := flag.String("out", "", "the golden JSON file to write")
	flag.Parse()
	if *out == "" {
		log.Fatal("-out is required")
	}
	data, err := json.MarshalIndent(golden{
		Erf:    erfCases(),
		MaxMin: maxMinCases(),
		Trials: trialCases(),
	}, "", "  ")
	if err != nil {
		log.Fatal(err)
	}
	if err := os.WriteFile(*out, append(data, '\n'), 0o644); err != nil {
		log.Fatal(err)
	}
}
