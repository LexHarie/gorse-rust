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

// Command modelreplay writes the replay goldens that the Rust ports of
// model/cf and model/ctr are checked against.
//
// It fits the Go models on small synthetic datasets with one job and fixed
// seeds, so the output is a deterministic function of the Go code:
//
//   - the train and test splits, as tab-separated (user, item) index pairs;
//   - for each model and each epoch count k from 1 to -epochs, the score that
//     Fit returns after k epochs (a model fitted for k epochs is the same as the
//     first k epochs of a longer fit, because the seed and the training loop do
//     not depend on the total);
//   - the fitted user and item factors after -epochs epochs.
//
// Usage, from the repository root:
//
//	go run ./contracts/tools/modelreplay -mode cf -out crates/gorse-model/testdata/cf
//	go run ./contracts/tools/modelreplay -mode ctr -out crates/gorse-model/testdata/ctr
//
// The ctr mode (ctr.go) fits the factorization machine with attention on a
// synthetic dataset and writes afm_replay.json: the dataset, the score after
// every epoch and the final parameters. Go's Fit returns only its last score,
// and the initial weights come from the process-wide math/rand, so each epoch
// count is fitted in a child process (-fit-epochs) with GODEBUG=randautoseed=0.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/gorse-io/gorse/dataset"
	"github.com/gorse-io/gorse/model"
	"github.com/gorse-io/gorse/model/cf"
	"github.com/gorse-io/gorse/storage/data"
)

// The synthetic dataset: every user has interactions with eight distinct items,
// the last two of which go to the test split.
const (
	numUsers       = 40
	numItems       = 60
	interactions   = 8
	testInteracts  = 2
	trainInteracts = interactions - testInteracts
)

type epochScore struct {
	NDCG      float32 `json:"ndcg"`
	Precision float32 `json:"precision"`
	Recall    float32 `json:"recall"`
}

type modelGolden struct {
	Params     model.Params `json:"params"`
	Epochs     []epochScore `json:"epochs"`
	UserFactor [][]float32  `json:"user_factor"`
	ItemFactor [][]float32  `json:"item_factor"`
}

type golden struct {
	Commit string      `json:"commit"`
	Users  int         `json:"users"`
	Items  int         `json:"items"`
	BPR    modelGolden `json:"bpr"`
	ALS    modelGolden `json:"als"`
}

func main() {
	mode := flag.String("mode", "cf", "the model family to generate: cf or ctr")
	out := flag.String("out", "", "the directory to write into")
	epochs := flag.Int("epochs", 5, "the number of epochs of the final fit")
	fitEpochs := flag.Int("fit-epochs", 0, "internal, ctr mode: fit this many epochs in this process")
	fitOut := flag.String("fit-out", "", "internal, ctr mode: the file the fit result is written to")
	flag.Parse()
	if *mode == "ctr" && *fitEpochs > 0 {
		// A child process of writeCTR, which fits one epoch count on its own.
		if err := fitCTR(*fitEpochs, *fitOut); err != nil {
			log.Fatal(err)
		}
		return
	}
	if *out == "" {
		log.Fatal("-out is required")
	}
	if err := os.MkdirAll(*out, 0o755); err != nil {
		log.Fatal(err)
	}
	switch *mode {
	case "cf":
		if err := generateCF(*out, *epochs); err != nil {
			log.Fatal(err)
		}
	case "ctr":
		if err := writeCTR(*out, *epochs); err != nil {
			log.Fatal(err)
		}
	default:
		log.Fatalf("unknown mode %q", *mode)
	}
}

// pairs returns the (user, item) index pairs of the split: the first
// trainInteracts interactions of every user for train, the rest for test.
func pairs(test bool) [][2]int {
	var out [][2]int
	for u := 0; u < numUsers; u++ {
		for k := 0; k < interactions; k++ {
			if (k >= trainInteracts) != test {
				continue
			}
			out = append(out, [2]int{u, (u*11 + k*13) % numItems})
		}
	}
	return out
}

// build creates a split with every user and item added in index order, then
// the feedback of the split. Every split shares the same index mapping.
func build(feedback [][2]int) *dataset.Dataset {
	ds := dataset.NewDataset(time.Now(), 0, 0)
	for u := 0; u < numUsers; u++ {
		ds.AddUser(data.User{UserId: strconv.Itoa(u)})
	}
	for i := 0; i < numItems; i++ {
		ds.AddItem(data.Item{ItemId: strconv.Itoa(i)})
	}
	for _, p := range feedback {
		ds.AddFeedback(strconv.Itoa(p[0]), strconv.Itoa(p[1]), time.Time{})
	}
	return ds
}

func writePairs(path string, ps [][2]int) error {
	var sb strings.Builder
	for _, p := range ps {
		fmt.Fprintf(&sb, "%d\t%d\n", p[0], p[1])
	}
	return os.WriteFile(path, []byte(sb.String()), 0o644)
}

func fitConfig() *cf.FitConfig {
	return cf.NewFitConfig().SetVerbose(1).SetJobs(1)
}

// fit fits a fresh model for k epochs and returns its score and the model.
func fit(newModel func(model.Params) cf.Model, params model.Params, k int) (cf.Score, cf.Model) {
	params[model.NEpochs] = k
	m := newModel(params)
	train := build(pairs(false))
	test := build(pairs(true))
	score := m.Fit(context.Background(), train, test, fitConfig())
	return score, m
}

func generateCF(out string, epochs int) error {
	if err := writePairs(filepath.Join(out, "train.tsv"), pairs(false)); err != nil {
		return err
	}
	if err := writePairs(filepath.Join(out, "test.tsv"), pairs(true)); err != nil {
		return err
	}
	g := golden{Commit: "add21b5", Users: numUsers, Items: numItems}
	models := []struct {
		name   string
		golden *modelGolden
		newM   func(model.Params) cf.Model
		params func() model.Params
	}{
		{"bpr", &g.BPR, func(p model.Params) cf.Model { return cf.NewBPR(p) }, bprParams},
		{"als", &g.ALS, func(p model.Params) cf.Model { return cf.NewALS(p) }, alsParams},
	}
	for _, mg := range models {
		mg.golden.Params = mg.params()
		mg.golden.Params[model.NEpochs] = epochs
		for k := 1; k <= epochs; k++ {
			score, m := fit(mg.newM, mg.params(), k)
			mg.golden.Epochs = append(mg.golden.Epochs, epochScore{
				NDCG:      score.NDCG,
				Precision: score.Precision,
				Recall:    score.Recall,
			})
			if k == epochs {
				for u := 0; u < numUsers; u++ {
					mg.golden.UserFactor = append(mg.golden.UserFactor, append([]float32(nil), m.GetUserFactor(int32(u))...))
				}
				for i := 0; i < numItems; i++ {
					mg.golden.ItemFactor = append(mg.golden.ItemFactor, append([]float32(nil), m.GetItemFactor(int32(i))...))
				}
			}
		}
		log.Printf("%s: %d epochs fitted", mg.name, epochs)
	}
	body, err := json.MarshalIndent(g, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(out, "golden.json"), append(body, '\n'), 0o644)
}

// bprParams are the hyper-parameters of TestBPR_MovieLens in model/cf.
func bprParams() model.Params {
	return model.Params{
		model.NFactors:   8,
		model.Reg:        0.01,
		model.Lr:         0.05,
		model.NEpochs:    1,
		model.InitMean:   0,
		model.InitStdDev: 0.001,
	}
}

// alsParams are the hyper-parameters of TestCCD_MovieLens in model/cf.
func alsParams() model.Params {
	return model.Params{
		model.NFactors: 8,
		model.Reg:      0.015,
		model.NEpochs:  1,
		model.Alpha:    0.05,
	}
}
