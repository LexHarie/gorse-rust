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

package main

import (
	"bytes"
	"encoding/hex"
	"fmt"

	"github.com/gorse-io/gorse/common/nn"
	"github.com/gorse-io/gorse/protocol"
	"github.com/matttproud/golang_protobuf_extensions/pbutil"
)

func seqModel(seed uint64) *nn.Sequential {
	l1 := nn.NewLinear(2, 3).(*nn.LinearLayer)
	l2 := nn.NewLinear(3, 2).(*nn.LinearLayer)
	setLinear(l1, seed)
	setLinear(l2, seed+2)
	return nn.NewSequential(l1, nn.NewSigmoid(), l2).(*nn.Sequential)
}

// message is one length-delimited protocol.Tensor, as Save writes them.
func message(key []string, shape []int32, data []float32) []byte {
	var buf bytes.Buffer
	if _, err := pbutil.WriteDelimited(&buf, &protocol.Tensor{Key: key, Shape: shape, Data: data}); err != nil {
		panic(err)
	}
	return buf.Bytes()
}

func cat(parts ...[]byte) []byte { return bytes.Join(parts, nil) }

func saveCase(r *rec, o any) {
	var buf bytes.Buffer
	r.add("err", errValue(nn.Save(o, &buf)))
	r.add("bytes", hex.EncodeToString(buf.Bytes()))
}

// loadCase loads the stream into the target and records the error or panic
// and the target's parameters afterwards.
func loadCase(r *rec, stream []byte, target any, params []*nn.Tensor) {
	var err error
	p := catch(func() { err = nn.Load(target, bytes.NewReader(stream)) })
	if p != nil {
		r.add("result", p)
	} else {
		r.add("result", errValue(err))
	}
	for i, t := range params {
		r.t(fmt.Sprintf("param%d", i), t)
	}
}

// unexportedType has an exported field Save cannot write.
type unexportedType struct {
	T *nn.Tensor
	N int
}

func saveLoadCases() []testCase {
	cs := []testCase{
		{"saveload/save/sequential", func(r *rec) { saveCase(r, seqModel(900)) }},
		{"saveload/save/params", func(r *rec) { saveCase(r, seqModel(900).Parameters()) }},
		{"saveload/save/attention", func(r *rec) { saveCase(r, newAttention(3, 2, 904)) }},
		{"saveload/save/layers", func(r *rec) {
			e := nn.NewEmbedding(4, 2).(*nn.EmbeddingLayer)
			setData(e.W, 907, 1)
			saveCase(r, nn.NewSequential(e, nn.NewFlatten(), nn.NewReLU(), nn.NewSigmoid()))
		}},
		{"saveload/save/tensor", func(r *rec) { saveCase(r, tensor(908, 2, 2)) }},
		{"saveload/save/scalars", func(r *rec) {
			saveCase(r, []*nn.Tensor{nn.Zeros(), scalar(-2.5), nn.Zeros(0), lit([]float32{bits(0x80000000), nan}, 2)})
		}},
		{"saveload/save/unexpected_type", func(r *rec) {
			saveCase(r, &unexportedType{T: tensor(909, 2), N: 3})
		}},
		{"saveload/roundtrip/sequential", func(r *rec) {
			var buf bytes.Buffer
			if err := nn.Save(seqModel(910), &buf); err != nil {
				panic(err)
			}
			m := seqModel(920)
			loadCase(r, buf.Bytes(), m, m.Parameters())
			r.t("forward", m.Forward(tensor(930, 4, 2)))
		}},
		{"saveload/roundtrip/params", func(r *rec) {
			var buf bytes.Buffer
			if err := nn.Save(seqModel(911).Parameters(), &buf); err != nil {
				panic(err)
			}
			m := seqModel(921)
			loadCase(r, buf.Bytes(), m.Parameters(), m.Parameters())
		}},
		{"saveload/roundtrip/attention", func(r *rec) {
			var buf bytes.Buffer
			if err := nn.Save(newAttention(3, 2, 912), &buf); err != nil {
				panic(err)
			}
			a := newAttention(3, 2, 922)
			loadCase(r, buf.Bytes(), a, a.Parameters())
		}},
	}
	seqStream := map[string][]byte{
		"empty":                {},
		"mid_varint":           {0x80},
		"header_only":          {0x05},
		"partial_body":         {0x05, 0x0a},
		"long_length":          {0xff, 0x01, 0x0a, 0x01, 0x30},
		"bad_varint":           {0x80, 0x80, 0x80, 0x80, 0x80},
		"five_byte_zero":       {0x80, 0x80, 0x80, 0x80, 0x00},
		"unknown_field":        message([]string{"Foo"}, []int32{2, 3}, gen(940, 6, 1)),
		"unexported":           message([]string{"Layers", "0", "jobs"}, nil, nil),
		"key_exhausted_struct": message([]string{"Layers", "0"}, nil, nil),
		"key_exhausted_slice":  message([]string{"Layers"}, nil, nil),
		"key_too_long":         message([]string{"Layers", "0", "W", "extra"}, []int32{2, 3}, gen(941, 6, 1)),
		"sigmoid_layer":        message([]string{"Layers", "1", "W"}, []int32{1}, []float32{1}),
		"bad_layer_index":      message([]string{"Layers", "first", "W"}, nil, nil),
		"multiple": cat(message([]string{"Layers", "2", "B"}, []int32{2}, []float32{1, 2}),
			message([]string{"Layers", "2", "B"}, []int32{2}, []float32{3, 4})),
		"after_error":   cat(message([]string{"Layers", "x"}, nil, nil), message([]string{"Layers", "2", "B"}, []int32{2}, []float32{5, 6})),
		"invalid_wire":  {0x02, 0x0a, 0xff},
		"invalid_utf8":  {0x03, 0x0a, 0x01, 0xff},
		"nil_key":       message(nil, []int32{1}, []float32{1}),
		"unpacked_data": {0x0d, 0x0a, 0x06, 'L', 'a', 'y', 'e', 'r', 's', 0x1d, 0x00, 0x00, 0x80, 0x3f},
	}
	for _, name := range sortedKeys(seqStream) {
		stream := seqStream[name]
		cs = append(cs, testCase{"saveload/load/sequential/" + name, func(r *rec) {
			m := seqModel(950)
			loadCase(r, stream, m, m.Parameters())
		}})
	}
	paramStream := map[string][]byte{
		"bad_key":            message([]string{"x"}, nil, nil),
		"index_out_of_range": message([]string{"9"}, nil, nil),
		"negative_index":     message([]string{"-1"}, nil, nil),
		"plus_index":         message([]string{"+1"}, []int32{3}, []float32{7, 8, 9}),
		"shape_change":       message([]string{"1"}, []int32{6}, gen(942, 6, 1)),
		"inconsistent":       message([]string{"0"}, []int32{2, 2}, []float32{1, 2, 3}),
		"no_shape":           message([]string{"3"}, nil, []float32{1, 2}),
		"nil_key":            message(nil, []int32{1}, []float32{1}),
		"unpacked_data":      {0x0d, 0x0a, 0x01, '0', 0x1d, 0x00, 0x00, 0x80, 0x3f, 0x1d, 0x00, 0x00, 0x00, 0x40},
		"unpacked_shape":     {0x0b, 0x0a, 0x01, '2', 0x10, 0x01, 0x10, 0x02, 0x1a, 0x04, 0x00, 0x00, 0x40, 0x40},
	}
	for _, name := range sortedKeys(paramStream) {
		stream := paramStream[name]
		cs = append(cs, testCase{"saveload/load/params/" + name, func(r *rec) {
			params := seqModel(960).Parameters()
			loadCase(r, stream, params, params)
		}})
	}
	tensorStream := map[string][]byte{
		"no_key":   message(nil, []int32{3}, []float32{1, 2, 3}),
		"with_key": message([]string{"a", "b"}, []int32{1, 2}, []float32{4, 5}),
		"empty":    message(nil, nil, nil),
	}
	for _, name := range sortedKeys(tensorStream) {
		stream := tensorStream[name]
		cs = append(cs, testCase{"saveload/load/tensor/" + name, func(r *rec) {
			t := tensor(970, 2, 2)
			loadCase(r, stream, t, []*nn.Tensor{t})
		}})
	}
	return cs
}

func sortedKeys(m map[string][]byte) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	for i := range keys {
		for j := i + 1; j < len(keys); j++ {
			if keys[j] < keys[i] {
				keys[i], keys[j] = keys[j], keys[i]
			}
		}
	}
	return keys
}
