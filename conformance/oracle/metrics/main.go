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

// Command metrics is the Go side of the dataset metric comparison in
// conformance/tests/metrics.rs (WP-3-8).
//
// It fits one configuration on one built-in dataset with the same parameters
// as the Go model test that uses that dataset, and it writes the metrics of the
// last evaluation as JSON. The Rust test runs it for every configuration and
// compares the Go metrics with the Rust fit on the same files.
//
// The datasets are read from ~/.gorse/dataset, where the Go loader expects them
// (model/built_in.go), so the archives must be unpacked there first. The Rust
// test runs this with GODEBUG=randautoseed=0, which fixes the factorization
// machines' initial weights to seed 1 (see INIT_SEED in conformance/tests/metrics.rs).
//
// The fit runs with -jobs goroutines, which defaults to runtime.NumCPU(), as the
// Go tests do. With one job the fit is deterministic, and the one-job checks
// compare the two builds on the same schedule.
//
// Usage, from the repository root:
//
//	go run ./conformance/oracle/metrics -config bpr-ml-1m -out /tmp/bpr.json
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"log"
	"os"
	"runtime"

	"github.com/gorse-io/gorse/dataset"
	"github.com/gorse-io/gorse/model"
	"github.com/gorse-io/gorse/model/cf"
	"github.com/gorse-io/gorse/model/ctr"
)

// cfResult is the output of a collaborative-filtering configuration.
type cfResult struct {
	Config    string  `json:"config"`
	Dataset   string  `json:"dataset"`
	NDCG      float32 `json:"ndcg"`
	Precision float32 `json:"precision"`
	Recall    float32 `json:"recall"`
}

// ctrResult is the output of a factorization-machine configuration. Clone is the
// same evaluation after MarshalModel and UnmarshalModel, as the Go test does.
type ctrResult struct {
	Config    string   `json:"config"`
	Dataset   string   `json:"dataset"`
	Accuracy  float32  `json:"accuracy"`
	Precision float32  `json:"precision"`
	Recall    float32  `json:"recall"`
	AUC       float32  `json:"auc"`
	Clone     ctrClone `json:"clone"`
}

// ctrClone is the evaluation of the model after a marshal round trip.
type ctrClone struct {
	Accuracy  float32 `json:"accuracy"`
	Precision float32 `json:"precision"`
	Recall    float32 `json:"recall"`
	AUC       float32 `json:"auc"`
}

// cfConfigs are the collaborative-filtering configurations: the parameters of
// the Go test that uses the dataset, or of the commented-out Go test for
// pinterest-20 (model/cf/model_test.go), or, for ml-100k, which no Go test uses,
// the parameters of the ml-1m tests.
var cfConfigs = map[string]struct {
	dataset  string
	newModel func() fitter
}{
	"bpr-ml-1m": {"ml-1m", func() fitter {
		return cf.NewBPR(model.Params{
			model.NFactors:   8,
			model.Reg:        0.01,
			model.Lr:         0.05,
			model.NEpochs:    30,
			model.InitMean:   0,
			model.InitStdDev: 0.001,
		})
	}},
	"als-ml-1m": {"ml-1m", func() fitter {
		return cf.NewALS(model.Params{
			model.NFactors: 8,
			model.Reg:      0.015,
			model.NEpochs:  30,
			model.Alpha:    0.05,
		})
	}},
	"bpr-ml-100k": {"ml-100k", func() fitter {
		return cf.NewBPR(model.Params{
			model.NFactors:   8,
			model.Reg:        0.01,
			model.Lr:         0.05,
			model.NEpochs:    30,
			model.InitMean:   0,
			model.InitStdDev: 0.001,
		})
	}},
	"als-ml-100k": {"ml-100k", func() fitter {
		return cf.NewALS(model.Params{
			model.NFactors: 8,
			model.Reg:      0.015,
			model.NEpochs:  30,
			model.Alpha:    0.05,
		})
	}},
	"bpr-pinterest-20": {"pinterest-20", func() fitter {
		return cf.NewBPR(model.Params{
			model.NFactors:   8,
			model.Reg:        0.005,
			model.Lr:         0.05,
			model.NEpochs:    50,
			model.InitMean:   0,
			model.InitStdDev: 0.001,
		})
	}},
	"als-pinterest-20": {"pinterest-20", func() fitter {
		return cf.NewALS(model.Params{
			model.NFactors:   8,
			model.Reg:        0.01,
			model.NEpochs:    20,
			model.InitStdDev: 0.01,
			model.Alpha:      0.001,
		})
	}},
}

// ctrConfigs are the factorization-machine configurations of model/ctr/model_test.go.
var ctrConfigs = map[string]struct {
	dataset string
	params  model.Params
}{
	"afm-frappe": {"frappe", model.Params{
		model.NFactors:  8,
		model.NEpochs:   10,
		model.Lr:        0.01,
		model.Reg:       0.0001,
		model.BatchSize: 1024,
	}},
	"afm-ml-tag": {"ml-tag", model.Params{
		model.InitStdDev: 0.01,
		model.NFactors:   8,
		model.NEpochs:    10,
		model.Lr:         0.001,
		model.Reg:        0.0001,
		model.BatchSize:  1024,
	}},
	"afm-criteo": {"criteo", model.Params{
		model.NFactors:  8,
		model.NEpochs:   10,
		model.Lr:        0.01,
		model.Reg:       0.0001,
		model.BatchSize: 1024,
		model.AutoScale: false,
	}},
}

// fitter is the part of a collaborative-filtering model that the oracle uses.
type fitter interface {
	Fit(ctx context.Context, trainSet, valSet dataset.CFSplit, config *cf.FitConfig) cf.Score
}

func main() {
	name := flag.String("config", "", "the configuration to run, for example bpr-ml-1m or afm-frappe")
	out := flag.String("out", "", "the JSON file to write the metrics to")
	jobs := flag.Int("jobs", runtime.NumCPU(), "the number of parallel jobs of the fit")
	flag.Parse()
	if *name == "" || *out == "" {
		log.Fatal("-config and -out are required")
	}
	var result any
	if c, ok := cfConfigs[*name]; ok {
		result = runCF(*name, c.dataset, c.newModel(), *jobs)
	} else if c, ok := ctrConfigs[*name]; ok {
		result = runCTR(*name, c.dataset, c.params, *jobs)
	} else {
		log.Fatalf("unknown configuration %q", *name)
	}
	data, err := json.MarshalIndent(result, "", "  ")
	if err != nil {
		log.Fatal(err)
	}
	if err := os.WriteFile(*out, append(data, '\n'), 0o644); err != nil {
		log.Fatal(err)
	}
}

// runCF fits a collaborative-filtering model on a built-in dataset, as the Go
// tests do: one evaluation per epoch set by the verbose setting, the last score.
func runCF(name, dataSet string, m fitter, jobs int) cfResult {
	train, test, err := dataset.LoadDataFromBuiltIn(dataSet)
	if err != nil {
		log.Fatal(err)
	}
	score := m.Fit(context.Background(), train, test, cf.NewFitConfig().SetVerbose(1).SetJobs(jobs))
	return cfResult{
		Config:    name,
		Dataset:   dataSet,
		NDCG:      score.NDCG,
		Precision: score.Precision,
		Recall:    score.Recall,
	}
}

// runCTR fits an attention factorization machine on a built-in dataset, then
// evaluates the model after a marshal round trip.
func runCTR(name, dataSet string, params model.Params, jobs int) ctrResult {
	train, test, err := ctr.LoadDataFromBuiltIn(dataSet)
	if err != nil {
		log.Fatal(err)
	}
	m := ctr.NewAFM(params)
	cfg := ctr.NewFitConfig().SetVerbose(1).SetJobs(jobs)
	score := m.Fit(context.Background(), train, test, cfg)
	buf := bytes.NewBuffer(nil)
	if err := ctr.MarshalModel(buf, m); err != nil {
		log.Fatal(err)
	}
	tmp, err := ctr.UnmarshalModel(buf)
	if err != nil {
		log.Fatal(err)
	}
	clone := ctr.EvaluateClassification(tmp, test, cfg.Jobs)
	return ctrResult{
		Config:    name,
		Dataset:   dataSet,
		Accuracy:  score.Accuracy,
		Precision: score.Precision,
		Recall:    score.Recall,
		AUC:       score.AUC,
		Clone: ctrClone{
			Accuracy:  clone.Accuracy,
			Precision: clone.Precision,
			Recall:    clone.Recall,
			AUC:       clone.AUC,
		},
	}
}
