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

// Command modelreplay writes golden data that replays the training of a Go
// model, for the Rust port's replay tests:
//
//   - -mode ctr: a factorization machine with attention (model/ctr) trained on
//     a synthetic dataset; the dataset, the score of every epoch and the final
//     parameters (ctr.go).
//
// Fit returns only its last score, and every fit draws its initial weights from
// the process-wide math/rand. Each epoch count is therefore fitted in a fresh
// child process, which runs with GODEBUG=randautoseed=0: the global source is
// then seeded with 1, the seed the Rust replay gives its own source.
//
// Usage, from the repository root:
//
//	go run ./contracts/tools/modelreplay -mode ctr -out crates/gorse-model/testdata/ctr/afm_replay.json
package main

import (
	"flag"
	"fmt"
	"os"
)

func main() {
	mode := flag.String("mode", "ctr", "the model to replay: ctr")
	out := flag.String("out", "", "the golden file to write")
	fitEpochs := flag.Int("fit-epochs", 0, "internal: fit this many epochs in this process")
	fitOut := flag.String("fit-out", "", "internal: the file the fit result is written to")
	flag.Parse()

	var err error
	switch *mode {
	case "ctr":
		if *fitEpochs > 0 {
			err = fitCTR(*fitEpochs, *fitOut)
		} else {
			err = writeCTR(*out)
		}
	default:
		err = fmt.Errorf("unknown mode %q", *mode)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
