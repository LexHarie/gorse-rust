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

// Command cfgolden trains Go's collaborative filtering models on the mixed
// cluster's data, with a chosen number of jobs, and prints a digest of every
// prediction for each run. It shows whether two runs with the same data and
// settings give the same model:
//
//	go run ./contracts/tools/cfgolden -model ALS -jobs 4 -runs 3
//
// The data is the one of crates/gorse-conformance/tests/mixed_worker.rs: ten
// users, thirty items, and two likes per user. Each run starts from a new
// model, so a difference between runs comes from the training itself.
package main

import (
	"context"
	"crypto/sha256"
	"encoding/binary"
	"flag"
	"fmt"
	"math"
	"os"
	"time"

	"github.com/gorse-io/gorse/dataset"
	"github.com/gorse-io/gorse/model"
	"github.com/gorse-io/gorse/model/cf"
	"github.com/gorse-io/gorse/storage/data"
)

const (
	users = 10
	items = 30
)

func main() {
	modelType := flag.String("model", "ALS", "the model: ALS or BPR")
	jobs := flag.Int("jobs", 1, "the number of jobs in training and evaluation")
	runs := flag.Int("runs", 2, "how many models to train")
	flag.Parse()
	for run := range *runs {
		digest, err := train(*modelType, *jobs)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		fmt.Printf("model=%s jobs=%d run=%d digest=%x\n", *modelType, *jobs, run, digest)
	}
}

// train fits one model on the seeded data and returns the digest of all of its
// predictions, user by user and item by item.
func train(modelType string, jobs int) ([sha256.Size]byte, error) {
	base := time.Unix(1_700_000_000, 0)
	d := dataset.NewDataset(base, users, items)
	for u := range users {
		d.AddUser(data.User{UserId: fmt.Sprintf("user-%d", u)})
	}
	for i := range items {
		d.AddItem(data.Item{ItemId: fmt.Sprintf("item-%d", i), Timestamp: base.Add(time.Duration(i) * time.Minute)})
	}
	for u := range users {
		for k := range 2 {
			item := (3*u + 7*k) % items
			d.AddFeedback(fmt.Sprintf("user-%d", u), fmt.Sprintf("item-%d", item), base.Add(time.Hour+time.Duration(2*u+k)*time.Minute))
		}
	}
	train, test := d.SplitCF(3, 0)

	var m cf.MatrixFactorization
	switch modelType {
	case "ALS":
		m = cf.NewALS(model.Params{})
	case "BPR":
		m = cf.NewBPR(model.Params{})
	default:
		return [sha256.Size]byte{}, fmt.Errorf("unknown model %q", modelType)
	}
	config := cf.NewFitConfig()
	config.Jobs = jobs
	// Fit divides by Verbose, so it cannot be zero; a large value logs once.
	config.Verbose = 1 << 20
	m.Fit(context.Background(), train, test, config)

	hash := sha256.New()
	for u := range users {
		for i := range items {
			prediction := m.Predict(fmt.Sprintf("user-%d", u), fmt.Sprintf("item-%d", i))
			var bits [4]byte
			binary.LittleEndian.PutUint32(bits[:], math.Float32bits(prediction))
			hash.Write(bits[:])
		}
	}
	var digest [sha256.Size]byte
	copy(digest[:], hash.Sum(nil))
	return digest, nil
}
