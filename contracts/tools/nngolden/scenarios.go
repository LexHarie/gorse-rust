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
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"math"
	"math/rand"
	"strings"

	"github.com/chewxy/math32"
	"github.com/gorse-io/gorse/common/nn"
)

// rawFloats is floats without the digest, for values the Rust test replays.
func rawFloats(d []float32) string {
	var b strings.Builder
	for _, x := range d {
		fmt.Fprintf(&b, "%08x", math.Float32bits(x))
	}
	return b.String()
}

func rawFloat64s(d []float64) string {
	var b strings.Builder
	for _, x := range d {
		fmt.Fprintf(&b, "%016x", math.Float64bits(x))
	}
	return b.String()
}

func scalar(v float32) *nn.Tensor { return nn.NewScalar(v) }

func goldstein(x, y *nn.Tensor) *nn.Tensor {
	return nn.Mul(
		nn.Add(scalar(1), nn.Mul(
			nn.Pow(nn.Add(x, y, scalar(1)), scalar(2)),
			nn.Add(
				scalar(19),
				nn.Mul(scalar(-14), x),
				nn.Mul(scalar(3), nn.Pow(x, scalar(2))),
				nn.Mul(scalar(-14), y),
				nn.Mul(scalar(6), nn.Mul(x, y)),
				nn.Mul(scalar(3), nn.Pow(y, scalar(2)))))),
		nn.Add(scalar(30), nn.Mul(
			nn.Pow(nn.Sub(nn.Mul(scalar(2), x), nn.Mul(scalar(3), y)), scalar(2)),
			nn.Add(
				scalar(18),
				nn.Mul(scalar(-32), x),
				nn.Mul(scalar(12), nn.Pow(x, scalar(2))),
				nn.Mul(scalar(48), y),
				nn.Mul(scalar(-36), nn.Mul(x, y)),
				nn.Mul(scalar(27), nn.Pow(y, scalar(2)))))))
}

func matyas(x, y *nn.Tensor) *nn.Tensor {
	return nn.Sub(nn.Mul(scalar(0.26), nn.Add(nn.Mul(x, x), nn.Mul(y, y))), nn.Mul(scalar(0.48), nn.Mul(x, y)))
}

func one6() *nn.Tensor { return lit([]float32{1, 2, 3, 4, 5, 6}, 2, 3) }

func graphCases() []testCase {
	return []testCase{
		{"graph/reuse_leaf", func(r *rec) {
			x := one6()
			y := nn.Add(x, x)
			y.Backward()
			r.t("y", y)
			r.t("x.grad", x.Grad())
		}},
		{"graph/reuse_node", func(r *rec) {
			x := one6()
			temp := nn.Pow(x, lit([]float32{2}))
			y := nn.Add(temp, temp)
			y.Backward()
			r.t("y", y)
			r.t("x.grad", x.Grad())
			r.t("temp.grad", temp.Grad())
		}},
		{"graph/dependency", func(r *rec) {
			x := one6()
			temp := nn.Pow(x, lit([]float32{2}))
			y := nn.Add(temp, nn.Mul(lit([]float32{2}), temp))
			y.Backward()
			r.t("y", y)
			r.t("x.grad", x.Grad())
		}},
		{"graph/sphere", func(r *rec) {
			x, y := scalar(1), scalar(1)
			z := nn.Add(nn.Mul(x, x), nn.Mul(y, y))
			z.Backward()
			r.t("z", z)
			r.t("x.grad", x.Grad())
			r.t("y.grad", y.Grad())
		}},
		{"graph/matyas", func(r *rec) {
			x, y := scalar(1), scalar(1)
			z := matyas(x, y)
			z.Backward()
			r.t("z", z)
			r.t("x.grad", x.Grad())
			r.t("y.grad", y.Grad())
		}},
		{"graph/goldstein", func(r *rec) {
			x, y := scalar(1), scalar(1)
			z := goldstein(x, y)
			z.Backward()
			r.t("z", z)
			r.t("x.grad", x.Grad())
			r.t("y.grad", y.Grad())
		}},
		{"graph/goldstein_off_minimum", func(r *rec) {
			x, y := scalar(0.3), scalar(-0.7)
			z := goldstein(x, y)
			z.Backward()
			r.t("z", z)
			r.t("x.grad", x.Grad())
			r.t("y.grad", y.Grad())
		}},
		{"graph/accumulate", func(r *rec) {
			x := tensor(600, 2, 3)
			y := nn.Mul(x, x)
			y.Backward()
			r.t("after_first", x.Grad())
			z := nn.Sin(x)
			z.Backward()
			r.t("after_second", x.Grad())
		}},
		{"graph/order", func(r *rec) {
			x := tensor(601, 2, 3)
			a := nn.Exp(x)
			b := nn.Sin(x)
			c := nn.Mul(a, b)
			d := nn.Add(c, a, b)
			e := nn.Sum(nn.Mul(d, nn.Cos(x)))
			r.add("generations", []int{x.GoldenGeneration(), a.GoldenGeneration(), b.GoldenGeneration(),
				c.GoldenGeneration(), d.GoldenGeneration(), e.GoldenGeneration()})
			e.Backward()
			r.t("e", e)
			r.t("x.grad", x.Grad())
		}},
		{"graph/no_grad", func(r *rec) {
			x := tensor(602, 2, 3)
			y := nn.Exp(x)
			y.NoGrad()
			z := nn.Sin(y)
			z.Backward()
			r.t("x.grad", x.Grad())
			r.t("y.grad", y.Grad())
		}},
		{"graph/backward_leaf", func(r *rec) {
			x := tensor(603, 2, 3)
			r.p("backward", x.Backward)
			r.t("x.grad", x.Grad())
		}},
		{"graph/backward_twice", func(r *rec) {
			x := tensor(604, 2, 3)
			y := nn.Square(x)
			y.Backward()
			y.Backward()
			r.t("x.grad", x.Grad())
			r.t("y.grad", y.Grad())
		}},
		{"graph/inference", func(r *rec) {
			x := tensor(605, 2, 3)
			nn.SetInferenceMode(true)
			y := nn.Exp(x)
			z := nn.Sum(y)
			nn.SetInferenceMode(false)
			r.t("z", z)
			r.add("generation", z.GoldenGeneration())
			r.p("backward", z.Backward)
			r.t("z.grad", z.Grad())
			r.t("x.grad", x.Grad())
		}},
		{"graph/inference_partial", func(r *rec) {
			x := tensor(606, 2, 3)
			y := nn.Exp(x)
			nn.SetInferenceMode(true)
			z := nn.Sin(y)
			nn.SetInferenceMode(false)
			w := nn.Sum(z)
			w.Backward()
			r.t("z.grad", z.Grad())
			r.t("y.grad", y.Grad())
			r.t("x.grad", x.Grad())
		}},
		{"graph/shape_mismatch", func(r *rec) {
			x := tensor(607, 2, 3)
			y := nn.Neg(x)
			var buf bytes.Buffer
			if err := nn.Save(tensor(608, 6), &buf); err != nil {
				panic(err)
			}
			if err := nn.Load(y, &buf); err != nil {
				panic(err)
			}
			r.p("backward", y.Backward)
			r.t("x.grad", x.Grad())
		}},
		{"graph/generation", func(r *rec) {
			x := tensor(609, 2, 3)
			a := nn.Exp(x)
			b := nn.Add(a, x)
			c := nn.Mul(b, a)
			d := nn.Add(scalar(1), c)
			r.add("generations", []int{x.GoldenGeneration(), a.GoldenGeneration(), b.GoldenGeneration(),
				c.GoldenGeneration(), d.GoldenGeneration()})
		}},
		{"graph/softmax_alias", func(r *rec) {
			x := scaled(610, 3, 2, 3)
			s := nn.Softmax(x, 1)
			f := nn.Flatten(s)
			loss := nn.Sum(nn.Mul(f, tensor(611, 6)))
			loss.Backward()
			r.t("s.after", s)
			r.t("f.after", f)
			r.t("x.grad", x.Grad())
		}},
		{"graph/reshape_alias", func(r *rec) {
			x := tensor(612, 2, 3)
			y := nn.Reshape(x, 3, 2)
			z := nn.Sum(nn.Square(y))
			z.Backward()
			r.t("x.grad", x.Grad())
			nn.NewSGD([]*nn.Tensor{x}, 0.5).Step()
			r.t("x.after", x)
			r.t("y.after", y)
		}},
		{"graph/embedding_index_node", func(r *rec) {
			x := lit([]float32{0.1, 0.9, 0.5}, 3)
			idx := nn.Mul(x, scalar(4))
			w := tensor(613, 6, 2)
			y := nn.Embedding(w, idx)
			y.Backward()
			r.t("y", y)
			r.t("w.grad", w.Grad())
			r.t("idx.grad", idx.Grad())
			r.t("x.grad", x.Grad())
		}},
	}
}

func tensorCases() []testCase {
	return []testCase{
		{"tensor/slice", func(r *rec) {
			x := tensor(700, 3, 4, 5)
			y := x.Slice(1, 3)
			r.t("y", y)
			r.add("cap", y.GoldenCap())
			r.add("get", floats([]float32{y.Get(0, 0, 0), y.Get(1, 3, 4), y.Get(1, 2, 0)}))
			r.p("scalar", func() { scalar(1).Slice(0, 1) })
			r.p("end", func() { x.Slice(0, 4) })
			r.p("reversed", func() { x.Slice(2, 1) })
			r.t("empty", x.Slice(1, 1))
		}},
		{"tensor/slice_write_through", func(r *rec) {
			x := tensor(701, 3, 2)
			y := x.Slice(1, 2)
			z := nn.Sum(y)
			z.Backward()
			nn.NewSGD([]*nn.Tensor{y}, 0.5).Step()
			r.t("x", x)
			r.t("y", y)
		}},
		{"tensor/slice_indices", func(r *rec) {
			x := one6()
			x = nn.NewTensor(x.Data(), 3, 2)
			r.t("go_test", x.SliceIndices(2, 0))
			parent := tensor(702, 3, 2)
			v := parent.Slice(0, 1)
			r.t("beyond_len", v.SliceIndices(2, 0))
			r.p("beyond_cap", func() { v.SliceIndices(3) })
			r.t("none", x.SliceIndices())
			r.p("scalar", func() { scalar(1).SliceIndices(0) })
			r.t("1d", lit([]float32{7, 8, 9}, 3).SliceIndices(2, 2, 0))
		}},
		{"tensor/get", func(r *rec) {
			x := tensor(703, 2, 3, 4)
			r.add("values", floats([]float32{x.Get(0, 0, 0), x.Get(1, 2, 3), x.Get(1, 0, 2)}))
			r.p("count", func() { x.Get(0, 0) })
			r.p("range", func() { x.Get(0, 3, 0) })
			r.add("scalar", floats([]float32{scalar(5).Get()}))
		}},
		{"tensor/string", func(r *rec) {
			values := []float32{0, bits(0x80000000), 1, -1, 0.1, 1e-5, 1.5e-5, 0.0001, 0.00012345, 123456,
				999999, 1e6, 1234567, 3.4028235e38, 1e-45, 1.17549435e-38, 16777216, 0.3, 2.5e-7, nan, inf,
				-inf, 100, 1e21, 1e20, 33.333332, -2.5e-5, 5e-324, 65504, 1.0000001}
			var s []string
			for _, v := range values {
				s = append(s, scalar(v).String())
			}
			s = append(s,
				lit([]float32{1, 2, 3}, 3).String(),
				nn.Zeros(0).String(),
				lit(gen(704, 10, 100), 10).String(),
				lit(gen(705, 11, 100), 11).String(),
				lit(gen(706, 24, 1e7), 2, 3, 4).String(),
				lit([]float32{nan, inf, -inf, bits(0x80000000)}, 2, 2).String(),
			)
			r.add("strings", s)
		}},
		{"tensor/format", func(r *rec) {
			// The String of scalars of random bits, and of integers divided
			// by 1, 2, 4 or 8, where two shortest forms are often equally near.
			g := seq(712)
			var b strings.Builder
			for range 100000 {
				b.WriteString(scalar(math.Float32frombits(uint32(g.next()))).String())
				b.WriteByte('\n')
			}
			for range 100000 {
				n := g.next()
				b.WriteString(scalar(float32(n>>40) / float32(int(1)<<(n%4))).String())
				b.WriteByte('\n')
			}
			sum := sha256.Sum256([]byte(b.String()))
			r.add("digest", hex.EncodeToString(sum[:]))
		}},
		{"tensor/linspace", func(r *rec) {
			r.t("0_1_5", nn.LinSpace(0, 1, 5))
			r.t("-2_3_2x3", nn.LinSpace(-2, 3, 2, 3))
			r.t("2_2_1", nn.LinSpace(2, 2, 1))
			r.t("scalar", nn.LinSpace(0, 1))
			r.t("empty", nn.LinSpace(0, 1, 0))
			r.t("0.1_0.7_7", nn.LinSpace(0.1, 0.7, 7))
		}},
		{"tensor/constructors", func(r *rec) {
			r.t("ones", nn.Ones(2, 3))
			r.t("zeros", nn.Zeros(3))
			r.t("zeros_scalar", nn.Zeros())
			r.t("ones_empty", nn.Ones(2, 0))
			r.t("scalar", scalar(2.5))
			r.add("is_scalar", []bool{nn.Zeros().IsScalar(), nn.Zeros(1).IsScalar(), scalar(1).IsScalar()})
			r.p("new_tensor_mismatch", func() { nn.NewTensor([]float32{1, 2, 3}, 2, 2) })
			r.p("new_tensor_scalar_mismatch", func() { nn.NewTensor([]float32{1, 2}) })
			r.t("new_tensor_scalar", nn.NewTensor([]float32{4}))
			r.t("new_tensor_empty", nn.NewTensor([]float32{}, 0, 3))
		}},
		{"tensor/max", func(r *rec) {
			x := lit([]float32{3, 2, 5, 6, 0, 0}, 6)
			r.t("1d", x.GoldenMax(0, false))
			r.t("1d_keep", x.GoldenMax(0, true))
			r.p("negative", func() { x.GoldenMax(-1, false) })
			r.p("past", func() { x.GoldenMax(2, false) })
			x = lit([]float32{1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12}, 3, 2, 2)
			r.t("axis1", x.GoldenMax(1, false))
			r.t("axis1_keep", x.GoldenMax(1, true))
			r.t("axis0", x.GoldenMax(0, false))
			r.t("axis2", x.GoldenMax(2, true))
			r.t("nan_first_1d", lit([]float32{nan, 1, 2}, 3).GoldenMax(0, false))
			r.t("nan_later_1d", lit([]float32{1, nan, 2}, 3).GoldenMax(0, false))
			r.t("zeros_1d", lit([]float32{bits(0x80000000), 0}, 2).GoldenMax(0, false))
			r.t("zeros_1d_rev", lit([]float32{0, bits(0x80000000)}, 2).GoldenMax(0, false))
			r.t("empty_1d", lit([]float32{}, 0).GoldenMax(0, false))
			r.t("nan_2d", lit([]float32{1, nan, 3, 4, bits(0x7fc12345), bits(0x7fc00001)}, 3, 2).GoldenMax(0, false))
			r.t("zeros_2d", lit([]float32{bits(0x80000000), 0, 0, bits(0x80000000), bits(0x80000000), bits(0x80000000)}, 2, 3).GoldenMax(0, false))
			r.t("random", scaled(707, 9, 4, 5, 3).GoldenMax(1, false))
		}},
		{"tensor/sum", func(r *rec) {
			x := lit([]float32{1, 2, 3, 4, 5, 6}, 6)
			r.t("1d", x.GoldenSum(0, false))
			r.p("negative", func() { x.GoldenSum(-1, false) })
			r.p("past", func() { x.GoldenSum(2, false) })
			x = lit([]float32{1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12}, 3, 2, 2)
			r.t("axis1", x.GoldenSum(1, false))
			r.t("axis1_keep", x.GoldenSum(1, true))
			r.t("axis0", x.GoldenSum(0, false))
			r.t("neg_zero_1d", lit([]float32{bits(0x80000000)}, 1).GoldenSum(0, false))
			r.t("neg_zero_2d", lit([]float32{bits(0x80000000), bits(0x80000000), bits(0x80000000), bits(0x80000000)}, 2, 2).GoldenSum(0, false))
			r.t("random", scaled(708, 9, 4, 5, 3).GoldenSum(1, true))
			r.t("random_1d", tensor(709, 100).GoldenSum(0, false))
		}},
		{"tensor/argmax", func(r *rec) {
			r.add("ties", lit([]float32{3, 7, 7, 1}, 4).GoldenArgmax())
			r.add("2d", lit([]float32{1, 5, 2, 5, 0, 9}, 2, 3).GoldenArgmax())
			r.add("nan_first", lit([]float32{nan, 1, 2}, 3).GoldenArgmax())
			r.add("nan_later", lit([]float32{1, nan, 3}, 3).GoldenArgmax())
			r.add("empty", nn.Zeros(0).GoldenArgmax())
			r.add("scalar", scalar(3).GoldenArgmax())
			r.add("3d", tensor(710, 2, 3, 4).GoldenArgmax())
		}},
		{"tensor/transpose", func(r *rec) {
			r.t("go_test", nn.NewTensor([]float32{1, 2, 3, 4, 5, 6}, 3, 2).GoldenTranspose())
			r.t("3d", tensor(711, 2, 3, 4).GoldenTranspose())
			r.p("1d", func() { lit([]float32{1, 2}, 2).GoldenTranspose() })
			r.p("scalar", func() { scalar(1).GoldenTranspose() })
		}},
		{"tensor/partition", func(r *rec) {
			for _, c := range [][2]int{{0, 3}, {-1, 2}, {5, 0}, {5, 1}, {5, -2}, {3, 5}, {5, 5}, {10, 3}, {7, 2}, {100, 7}, {64, 3}} {
				r.add(fmt.Sprintf("%d,%d", c[0], c[1]), nn.GoldenPartition(c[0], c[1]))
			}
		}},
		{"tensor/partition_aligned", func(r *rec) {
			for _, c := range [][3]int{{0, 3, 32}, {10, 0, 2}, {10, -1, 2}, {10, 3, 2}, {100, 4, 32}, {3200, 4, 32},
				{1000, 3, 32}, {5, 10, 32}, {10, 3, 0}, {10, 3, -4}, {64, 3, 32}, {50, 4, 32}, {250, 4, 32}} {
				r.add(fmt.Sprintf("%d,%d,%d", c[0], c[1], c[2]), nn.GoldenPartitionAligned(c[0], c[1], c[2]))
			}
		}},
		{"tensor/rand", func(r *rec) {
			x := nn.Rand(rand.New(rand.NewSource(42)), 2, 3)
			draws := rand.New(rand.NewSource(42))
			var f []float32
			for range 6 {
				f = append(f, draws.Float32())
			}
			r.add("draws", rawFloats(f))
			r.t("x", x)
			t := nn.Zeros(3, 4)
			nn.NormalInit(rand.New(rand.NewSource(7)), t, 0.5, 0.1)
			normals := rand.New(rand.NewSource(7))
			var n []float64
			for range 12 {
				n = append(n, normals.NormFloat64())
			}
			r.add("normals", rawFloat64s(n))
			r.t("normal_init", t)
			r.t("scalar", nn.Rand(rand.New(rand.NewSource(42)), []int{}...))
		}},
		{"tensor/uniform_normal_shapes", func(r *rec) {
			// The values come from the global source, which Go seeds randomly.
			r.add("uniform", shapeOf(nn.Uniform(-1, 1, 2, 3)))
			r.add("normal", shapeOf(nn.Normal(0, 1, 4)))
			r.add("rand", shapeOf(nn.Rand(nil, 3)))
		}},
	}
}

func setLinear(l *nn.LinearLayer, seed uint64) {
	setData(l.W, seed, 0.5)
	setData(l.B, seed+1, 0.1)
}

func trainLoop(r *rec, iters int, step func() float32, params []*nn.Tensor) {
	var losses []float32
	for range iters {
		losses = append(losses, step())
	}
	r.add("losses", floats(losses))
	for i, p := range params {
		r.t(fmt.Sprintf("param%d", i), p)
	}
}

func mlpSGD(r *rec, jobs int, wd float32) {
	l1 := nn.NewLinear(4, 8).(*nn.LinearLayer)
	l2 := nn.NewLinear(8, 3).(*nn.LinearLayer)
	setLinear(l1, 800)
	setLinear(l2, 802)
	model := nn.NewSequential(l1, nn.NewSigmoid(), l2)
	model.SetJobs(jobs)
	x := scaled(804, 2, 16, 4)
	y := lit(labels(805, 16, 3), 16)
	opt := nn.NewSGD(model.Parameters(), 0.1)
	opt.SetWeightDecay(wd)
	opt.SetJobs(jobs)
	trainLoop(r, 15, func() float32 {
		loss := nn.SoftmaxCrossEntropy(model.Forward(x), y)
		opt.ZeroGrad()
		loss.Backward()
		opt.Step()
		return loss.Data()[0]
	}, model.Parameters())
}

func mlpAdam(r *rec, jobs int) {
	l1 := nn.NewLinear(64, 50).(*nn.LinearLayer)
	l2 := nn.NewLinear(50, 5).(*nn.LinearLayer)
	setLinear(l1, 810)
	setLinear(l2, 812)
	model := nn.NewSequential(l1, nn.NewReLU(), l2)
	model.SetJobs(jobs)
	x := tensor(814, 20, 64)
	y := tensor(815, 20, 5)
	opt := nn.NewAdam(model.Parameters(), 0.01)
	opt.SetWeightDecay(0.001)
	opt.SetJobs(jobs)
	trainLoop(r, 8, func() float32 {
		loss := nn.MeanSquareError(model.Forward(x), y)
		opt.ZeroGrad()
		loss.Backward()
		opt.Step()
		return loss.Data()[0]
	}, model.Parameters())
}

func newAttention(dim, k int, seed uint64) *nn.Attention {
	a := nn.NewAttention(dim, k)
	setLinear(a.W.(*nn.LinearLayer), seed)
	setData(a.H, seed+2, 0.5)
	return a
}

// afm is model/ctr's AFM with the parts Forward uses.
type afm struct {
	B        *nn.Tensor
	W        nn.Layer
	V        nn.Layer
	A        []nn.Layer
	E        []nn.Layer
	nFactors int
}

func newAFM(seed uint64) *afm {
	fm := &afm{
		B:        nn.Zeros(),
		W:        nn.NewEmbedding(10, 1),
		V:        nn.NewEmbedding(10, 4),
		A:        []nn.Layer{newAttention(5, 4, seed+3)},
		E:        []nn.Layer{nn.NewLinear(5, 4)},
		nFactors: 4,
	}
	setData(fm.W.(*nn.EmbeddingLayer).W, seed, 0.1)
	setData(fm.V.(*nn.EmbeddingLayer).W, seed+1, 0.1)
	setLinear(fm.E[0].(*nn.LinearLayer), seed+6)
	return fm
}

func (fm *afm) setJobs(jobs int) {
	fm.W.SetJobs(jobs)
	fm.V.SetJobs(jobs)
	fm.A[0].SetJobs(jobs)
	fm.E[0].SetJobs(jobs)
}

// forward is AFM.Forward, line for line.
func (fm *afm) forward(indices, values *nn.Tensor, embeddings []*nn.Tensor, jobs int) *nn.Tensor {
	batchSize := indices.Shape()[0]
	numDimension := indices.Shape()[1]
	v := fm.V.Forward(indices)
	x := nn.Reshape(values, batchSize, numDimension, 1)
	vx := nn.BMM(v, x, true, false, jobs)
	sumSquare := nn.Square(vx)
	e2 := nn.Square(v)
	x2 := nn.Square(x)
	squareSum := nn.BMM(e2, x2, true, false, jobs)
	sum := nn.Sub(sumSquare, squareSum)
	sum = nn.Sum(sum, 1)
	sum = nn.Mul(sum, nn.NewScalar(0.5))
	w := fm.W.Forward(indices)
	linear := nn.BMM(w, x, true, false, jobs)
	fmOutput := nn.Add(nn.Reshape(linear, batchSize), nn.Reshape(sum, batchSize), fm.B)
	for i, embedding := range embeddings {
		encodedNorm := fm.E[i].Forward(fm.A[i].Forward(embedding))
		encodedNorm = nn.Reshape(encodedNorm, batchSize, fm.nFactors, 1)
		fmOutput = nn.Add(fmOutput, nn.Reshape(nn.BMM(vx, encodedNorm, true, false, jobs), batchSize))
	}
	return nn.Flatten(fmOutput)
}

func (fm *afm) parameters() []*nn.Tensor {
	var params []*nn.Tensor
	params = append(params, fm.B)
	params = append(params, fm.V.Parameters()...)
	params = append(params, fm.W.Parameters()...)
	params = append(params, fm.A[0].Parameters()...)
	params = append(params, fm.E[0].Parameters()...)
	return params
}

func afmCase(r *rec, adam bool, jobs int) {
	fm := newAFM(820)
	fm.setJobs(jobs)
	indices := lit(labels(830, 18, 10), 6, 3)
	values := tensor(831, 6, 3)
	embedding := tensor(832, 6, 5)
	target := lit(signs(833, 6), 6)
	var opt nn.Optimizer
	if adam {
		opt = nn.NewAdam(fm.parameters(), 0.01)
	} else {
		opt = nn.NewSGD(fm.parameters(), 0.05)
	}
	opt.SetWeightDecay(0.001)
	opt.SetJobs(jobs)
	trainLoop(r, 6, func() float32 {
		output := fm.forward(indices, values, []*nn.Tensor{embedding}, jobs)
		loss := nn.BCEWithLogits(target, output, nil)
		opt.ZeroGrad()
		loss.Backward()
		opt.Step()
		return loss.Data()[0]
	}, fm.parameters())
	var buf bytes.Buffer
	if err := nn.Save(fm.parameters(), &buf); err != nil {
		panic(err)
	}
	r.add("saved", floats(bytesAsFloats(buf.Bytes())))
	loaded := newAFM(840)
	if err := nn.Load(loaded.parameters(), &buf); err != nil {
		panic(err)
	}
	r.t("prediction", loaded.forward(indices, values, []*nn.Tensor{embedding}, jobs))
}

// bytesAsFloats pads b to a multiple of 4 and reads it as little-endian
// float32 bits, so that long byte strings are digested like tensors.
func bytesAsFloats(b []byte) []float32 {
	for len(b)%4 != 0 {
		b = append(b, 0)
	}
	f := make([]float32, len(b)/4)
	for i := range f {
		f[i] = math.Float32frombits(uint32(b[4*i]) | uint32(b[4*i+1])<<8 | uint32(b[4*i+2])<<16 | uint32(b[4*i+3])<<24)
	}
	return f
}

func neuralNetwork(r *rec) {
	// TestNeuralNetwork: the draws of rand.New(rand.NewSource(0)), then the
	// training run they seed.
	draws := rand.New(rand.NewSource(0))
	var f []float32
	for range 200 {
		f = append(f, draws.Float32())
	}
	var n []float64
	for range 20 {
		n = append(n, draws.NormFloat64())
	}
	r.add("draws", rawFloats(f))
	r.add("normals", rawFloat64s(n))

	rng := rand.New(rand.NewSource(0))
	x := nn.Rand(rng, 100, 1)
	y := nn.Add(nn.Rand(rng, 100, 1), nn.Sin(nn.Mul(x, nn.NewScalar(2*math32.Pi))))
	model := nn.NewSequential(nn.NewLinear(1, 10), nn.NewSigmoid(), nn.NewLinear(10, 1))
	nn.NormalInit(rng, model.(*nn.Sequential).Layers[0].(*nn.LinearLayer).W, 0, 0.01)
	nn.NormalInit(rng, model.(*nn.Sequential).Layers[2].(*nn.LinearLayer).W, 0, 0.01)
	optimizer := nn.NewSGD(model.Parameters(), 0.2)
	var checkpoints []float32
	for i := range 10000 {
		yPred := model.Forward(x)
		loss := nn.MeanSquareError(y, yPred)
		optimizer.ZeroGrad()
		loss.Backward()
		optimizer.Step()
		switch i {
		case 0, 1, 2, 10, 100, 1000, 5000, 9999:
			checkpoints = append(checkpoints, loss.Data()[0])
		}
	}
	r.add("checkpoints", floats(checkpoints))
	for i, p := range model.Parameters() {
		r.t(fmt.Sprintf("param%d", i), p)
	}
}

func trainCases() []testCase {
	return []testCase{
		{"train/mlp_sgd/j0", func(r *rec) { mlpSGD(r, 0, 0) }},
		{"train/mlp_sgd/j3_wd", func(r *rec) { mlpSGD(r, 3, 0.01) }},
		{"train/mlp_adam/j0", func(r *rec) { mlpAdam(r, 0) }},
		{"train/mlp_adam/j4", func(r *rec) { mlpAdam(r, 4) }},
		{"train/attention", func(r *rec) {
			a := newAttention(6, 4, 850)
			a.SetJobs(3)
			x := tensor(855, 5, 6)
			y := tensor(856, 5, 6)
			opt := nn.NewAdam(a.Parameters(), 0.01)
			trainLoop(r, 5, func() float32 {
				loss := nn.MeanSquareError(a.Forward(x), y)
				opt.ZeroGrad()
				loss.Backward()
				opt.Step()
				return loss.Data()[0]
			}, a.Parameters())
		}},
		{"train/afm_sgd/j0", func(r *rec) { afmCase(r, false, 0) }},
		{"train/afm_adam/j3", func(r *rec) { afmCase(r, true, 3) }},
		{"train/neural_network", neuralNetwork},
		{"train/sgd_empty_param", func(r *rec) {
			// A parameter without a gradient crashes Go's process: the nil
			// dereference happens in a goroutine of Step. With no elements
			// Step starts no goroutine and never reads the gradient.
			r.p("step", func() { nn.NewSGD([]*nn.Tensor{nn.Zeros(0)}, 0.1).Step() })
			r.p("adam_step", func() { nn.NewAdam([]*nn.Tensor{nn.Zeros(0)}, 0.1).Step() })
		}},
		{"train/adam_shared_param", func(r *rec) {
			p := tensor(861, 40)
			q := nn.Sum(nn.Square(p))
			q.Backward()
			opt := nn.NewAdam([]*nn.Tensor{p, p}, 0.1)
			opt.Step()
			opt.Step()
			r.t("p", p)
		}},
	}
}
