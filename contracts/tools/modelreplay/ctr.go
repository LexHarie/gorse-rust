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

package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math/rand"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"

	"github.com/gorse-io/gorse/common/floats"
	"github.com/gorse-io/gorse/common/nn"
	"github.com/gorse-io/gorse/dataset"
	"github.com/gorse-io/gorse/model"
	"github.com/gorse-io/gorse/model/ctr"
	"github.com/samber/lo"
)

// ctrSeed seeds the synthetic data. The data has its own source, so it does
// not depend on the global one that the model draws its weights from.
const ctrSeed = 20261028

// ctrGolden is the file the Rust replay test reads: the dataset, the score after
// every epoch and the final parameters of the golden fit.
type ctrGolden struct {
	Params             ctrParams    `json:"params"`
	InitSeed           int64        `json:"init_seed"`
	Names              names        `json:"names"`
	UserLabels         [][]feature  `json:"user_labels"`
	ItemLabels         [][]feature  `json:"item_labels"`
	EmbeddingDimension []int        `json:"embedding_dimension"`
	ItemEmbeddings     [][][]uint16 `json:"item_embeddings"`
	Train              split        `json:"train"`
	Test               split        `json:"test"`
	Epochs             []ctrScore   `json:"epochs"`
	Parameters         []tensor     `json:"parameters"`
}

// ctrParams are the hyper-parameters of the golden fit. NEpochs is the largest
// epoch count; the replay fits every count up to it.
type ctrParams struct {
	BatchSize  int     `json:"batch_size"`
	NFactors   int     `json:"n_factors"`
	NEpochs    int     `json:"n_epochs"`
	Lr         float32 `json:"lr"`
	Reg        float32 `json:"reg"`
	InitMean   float32 `json:"init_mean"`
	InitStdDev float32 `json:"init_std_dev"`
	Optimizer  string  `json:"optimizer"`
	AutoScale  bool    `json:"auto_scale"`
}

// names are the ids of the unified index, in the order they were added. The
// position of a name is its id.
type names struct {
	Users         []string `json:"users"`
	Items         []string `json:"items"`
	UserLabels    []string `json:"user_labels"`
	ItemLabels    []string `json:"item_labels"`
	ContextLabels []string `json:"context_labels"`
	Embeddings    []string `json:"embeddings"`
}

// feature is a label id and its value. A user or item label id is local to
// its label names; a context label id is absolute in the unified index, as Go
// stores it.
type feature struct {
	ID    int32   `json:"id"`
	Value float32 `json:"value"`
}

// split is a train or test set, by row.
type split struct {
	Users         []int32     `json:"users"`
	Items         []int32     `json:"items"`
	ContextLabels [][]feature `json:"context_labels"`
	Target        []float32   `json:"target"`
}

// ctrScore is the classification score after an epoch.
type ctrScore struct {
	Epoch     int     `json:"epoch"`
	Precision float32 `json:"precision"`
	Recall    float32 `json:"recall"`
	Accuracy  float32 `json:"accuracy"`
	AUC       float32 `json:"auc"`
}

// tensor is a parameter: its shape and its FP32 values, row-major.
type tensor struct {
	Shape []int     `json:"shape"`
	Data  []float32 `json:"data"`
}

// fitResult is what a child process writes: the score after its epochs and
// the parameters after them.
type fitResult struct {
	Score      ctrScore `json:"score"`
	Parameters []tensor `json:"parameters"`
}

// writeCTR fits the model once for each epoch count up to epochs, each in a
// fresh process, and writes the golden file afm_replay.json into the directory
// out.
func writeCTR(out string, epochs int) error {
	if epochs < 1 {
		return fmt.Errorf("-epochs must be at least 1, got %d", epochs)
	}
	self, err := os.Executable()
	if err != nil {
		return err
	}
	dir, err := os.MkdirTemp("", "modelreplay")
	if err != nil {
		return err
	}
	defer os.RemoveAll(dir)

	g := newCTRGolden()
	g.Params.NEpochs = epochs
	for k := 1; k <= epochs; k++ {
		result := filepath.Join(dir, "fit-"+strconv.Itoa(k)+".json")
		cmd := exec.Command(self, "-mode", "ctr", "-fit-epochs", strconv.Itoa(k), "-fit-out", result)
		cmd.Env = append(os.Environ(), "GODEBUG=randautoseed=0")
		// The fit logs to stdout, so the child's output goes to stderr.
		cmd.Stdout = os.Stderr
		cmd.Stderr = os.Stderr
		if err := cmd.Run(); err != nil {
			return fmt.Errorf("fit %d epochs: %w", k, err)
		}
		data, err := os.ReadFile(result)
		if err != nil {
			return err
		}
		var fit fitResult
		if err := json.Unmarshal(data, &fit); err != nil {
			return err
		}
		g.Epochs = append(g.Epochs, fit.Score)
		if k == epochs {
			g.Parameters = fit.Parameters
		}
	}

	data, err := json.MarshalIndent(g, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(out, "afm_replay.json"), append(data, '\n'), 0o644)
}

// fitCTR fits the golden model for epochs epochs in this process and writes
// the result to out.
func fitCTR(epochs int, out string) error {
	if out == "" {
		return errors.New("-fit-out is required")
	}
	g := newCTRGolden()
	fm := ctr.NewAFM(g.Params.modelParams(epochs))
	config := &ctr.FitConfig{Jobs: 1, Verbose: 1, Patience: 0}
	s := fm.Fit(context.Background(), g.dataset(g.Train), g.dataset(g.Test), config)
	data, err := json.Marshal(fitResult{
		Score: ctrScore{
			Epoch:     epochs,
			Precision: s.Precision,
			Recall:    s.Recall,
			Accuracy:  s.Accuracy,
			AUC:       s.AUC,
		},
		Parameters: tensors(fm.Parameters()),
	})
	if err != nil {
		return err
	}
	return os.WriteFile(out, data, 0o644)
}

// newCTRGolden makes the hyper-parameters and the synthetic dataset. Users have
// a gender, which is a categorical label, and an age, which is numerical and
// negative for some users. Items have a category, a price and a rating. Each
// sample has an hour, which is numerical, and a device. A user and an item are
// positive when their categories agree, with a tenth of the targets flipped.
func newCTRGolden() *ctrGolden {
	rng := rand.New(rand.NewSource(ctrSeed))
	g := &ctrGolden{
		Params: ctrParams{
			BatchSize:  16,
			NFactors:   4,
			Lr:         0.05,
			Reg:        0.0002,
			InitMean:   0,
			InitStdDev: 0.01,
			Optimizer:  model.Adam,
			AutoScale:  true,
		},
		InitSeed: 1,
		Names: names{
			UserLabels:    []string{"gender:0", "gender:1", "age"},
			ItemLabels:    []string{"category:0", "category:1", "price", "rating"},
			ContextLabels: []string{"hour", "device:0", "device:1"},
			Embeddings:    []string{"image", "text"},
		},
		EmbeddingDimension: []int{4, 3},
	}
	for u := 0; u < 8; u++ {
		g.Names.Users = append(g.Names.Users, fmt.Sprintf("u%d", u))
		g.UserLabels = append(g.UserLabels, []feature{
			{ID: int32(u % 2), Value: 1},
			{ID: 2, Value: rng.Float32()*4 - 2},
		})
	}
	for i := 0; i < 10; i++ {
		g.Names.Items = append(g.Names.Items, fmt.Sprintf("i%d", i))
		g.ItemLabels = append(g.ItemLabels, []feature{
			{ID: int32(i % 2), Value: 1},
			{ID: 2, Value: rng.Float32()*40 + 0.5},
			{ID: 3, Value: rng.Float32()*2 - 1},
		})
		var embeddings [][]uint16
		for _, dim := range g.EmbeddingDimension {
			vector := make([]float32, dim)
			for k := range vector {
				vector[k] = rng.Float32()*2 - 1
			}
			embeddings = append(embeddings, floats.FromFloat32(vector))
		}
		g.ItemEmbeddings = append(g.ItemEmbeddings, embeddings)
	}
	g.Train = g.sample(rng, 96)
	g.Test = g.sample(rng, 32)
	return g
}

// sample draws n samples of the golden dataset.
func (g *ctrGolden) sample(rng *rand.Rand, n int) split {
	// The context labels follow the user and item labels in the unified index.
	ctxBase := int32(len(g.Names.Users) + len(g.Names.Items) + len(g.Names.UserLabels) + len(g.Names.ItemLabels))
	var s split
	for k := 0; k < n; k++ {
		user := rng.Intn(len(g.Names.Users))
		item := rng.Intn(len(g.Names.Items))
		hour := rng.Float32() * 24
		device := int32(1 + rng.Intn(2))
		target := float32(0)
		if (user%2 == item%2) != (rng.Float32() < 0.1) {
			target = 1
		}
		s.Users = append(s.Users, int32(user))
		s.Items = append(s.Items, int32(item))
		s.ContextLabels = append(s.ContextLabels, []feature{
			{ID: ctxBase, Value: hour},
			{ID: ctxBase + device, Value: 1},
		})
		s.Target = append(s.Target, target)
	}
	return s
}

// index builds the unified index of the names, in their order.
func (g *ctrGolden) index() dataset.UnifiedIndex {
	builder := dataset.NewUnifiedMapIndexBuilder()
	for _, name := range g.Names.Users {
		builder.AddUser(name)
	}
	for _, name := range g.Names.Items {
		builder.AddItem(name)
	}
	for _, name := range g.Names.UserLabels {
		builder.AddUserLabel(name)
	}
	for _, name := range g.Names.ItemLabels {
		builder.AddItemLabel(name)
	}
	for _, name := range g.Names.ContextLabels {
		builder.AddCtxLabel(name)
	}
	return builder.Build()
}

// dataset makes the Go dataset of a split. The item and user tables are shared
// by the splits.
func (g *ctrGolden) dataset(s split) *ctr.Dataset {
	embeddingIndex := dataset.NewMapIndex()
	for _, name := range g.Names.Embeddings {
		embeddingIndex.Add(name)
	}
	ds := &ctr.Dataset{
		Index:                  g.index(),
		UserLabels:             toTuples(g.UserLabels),
		ItemLabels:             toTuples(g.ItemLabels),
		ContextLabels:          toTuples(s.ContextLabels),
		Users:                  s.Users,
		Items:                  s.Items,
		Target:                 s.Target,
		ItemEmbeddings:         g.ItemEmbeddings,
		ItemEmbeddingDimension: g.EmbeddingDimension,
		ItemEmbeddingIndex:     embeddingIndex,
	}
	for _, target := range s.Target {
		if target > 0 {
			ds.PositiveCount++
		} else {
			ds.NegativeCount++
		}
	}
	return ds
}

// modelParams are the parameters of the model, with the given epoch count.
func (p ctrParams) modelParams(epochs int) model.Params {
	return model.Params{
		model.BatchSize:  p.BatchSize,
		model.NFactors:   p.NFactors,
		model.NEpochs:    epochs,
		model.Lr:         p.Lr,
		model.Reg:        p.Reg,
		model.InitMean:   p.InitMean,
		model.InitStdDev: p.InitStdDev,
		model.Optimizer:  p.Optimizer,
		model.AutoScale:  p.AutoScale,
	}
}

// toTuples converts rows of features to the label tuples of the dataset.
func toTuples(rows [][]feature) [][]lo.Tuple2[int32, float32] {
	out := make([][]lo.Tuple2[int32, float32], len(rows))
	for i, row := range rows {
		for _, f := range row {
			out[i] = append(out[i], lo.Tuple2[int32, float32]{A: f.ID, B: f.Value})
		}
	}
	return out
}

// tensors dumps the parameters of a model in the order it lists them.
func tensors(ts []*nn.Tensor) []tensor {
	out := make([]tensor, 0, len(ts))
	for _, t := range ts {
		shape := t.Shape()
		if shape == nil {
			// A scalar has no dimensions; JSON writes its shape as an empty array.
			shape = []int{}
		}
		out = append(out, tensor{Shape: shape, Data: t.Data()})
	}
	return out
}
