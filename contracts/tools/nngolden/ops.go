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
	"fmt"

	"github.com/gorse-io/gorse/common/nn"
)

// opCase applies f to the inputs and records the output (or the panic), then
// runs Backward on the output and records the panic if any, the gradient of
// each input, the output's own gradient and the output's data afterwards.
func opCase(r *rec, f func(in []*nn.Tensor) *nn.Tensor, in ...*nn.Tensor) {
	var out *nn.Tensor
	if p := catch(func() { out = f(in) }); p != nil {
		r.add("out", p)
		return
	}
	r.t("out", out)
	r.p("backward", out.Backward)
	for i, x := range in {
		r.t(fmt.Sprintf("grad%d", i), x.Grad())
	}
	r.t("out.grad", out.Grad())
	r.t("out.after", out)
}

func unary(f func(*nn.Tensor) *nn.Tensor) func([]*nn.Tensor) *nn.Tensor {
	return func(in []*nn.Tensor) *nn.Tensor { return f(in[0]) }
}

func binop(f func(*nn.Tensor, *nn.Tensor) *nn.Tensor) func([]*nn.Tensor) *nn.Tensor {
	return func(in []*nn.Tensor) *nn.Tensor { return f(in[0], in[1]) }
}

func add(in []*nn.Tensor) *nn.Tensor { return nn.Add(in[0], in[1:]...) }

func tf(b bool) string {
	if b {
		return "t"
	}
	return "f"
}

func opCases() []testCase {
	sub := binop(nn.Sub)
	mul := binop(nn.Mul)
	div := binop(nn.Div)
	pow := binop(nn.Pow)
	cs := []testCase{
		{"op/neg", func(r *rec) { opCase(r, unary(nn.Neg), tensor(1, 2, 3)) }},
		{"op/add/same", func(r *rec) { opCase(r, add, tensor(2, 2, 3), tensor(3, 2, 3)) }},
		{"op/add/scalar", func(r *rec) { opCase(r, add, tensor(4, 2, 3), tensor(5)) }},
		{"op/add/suffix", func(r *rec) { opCase(r, add, tensor(6, 2, 3), tensor(7, 3)) }},
		{"op/add/scalar_first", func(r *rec) { opCase(r, add, tensor(8), tensor(9, 2, 3)) }},
		{"op/add/three", func(r *rec) { opCase(r, add, tensor(10, 2, 3), tensor(11, 3), tensor(12, 2, 3)) }},
		{"op/add/flat_quirk", func(r *rec) { opCase(r, add, tensor(13, 6), tensor(14, 2, 3)) }},
		{"op/add/negative_index", func(r *rec) { opCase(r, add, tensor(15, 3), tensor(16, 2, 3), tensor(17, 4, 2, 3)) }},
		{"op/add/mismatch", func(r *rec) { opCase(r, add, tensor(18, 2, 3), tensor(19, 2)) }},
		{"op/add/same_input", func(r *rec) {
			opCase(r, func(in []*nn.Tensor) *nn.Tensor { return nn.Add(in[0], in[0]) }, tensor(20, 2, 3))
		}},
		{"op/add/one", func(r *rec) { opCase(r, add, tensor(21, 2, 3)) }},
		{"op/sub/same", func(r *rec) { opCase(r, sub, tensor(22, 2, 3), tensor(23, 2, 3)) }},
		{"op/sub/scalar", func(r *rec) { opCase(r, sub, tensor(24, 2, 3), tensor(25)) }},
		{"op/sub/suffix", func(r *rec) { opCase(r, sub, tensor(26, 2, 3), tensor(27, 3)) }},
		{"op/sub/longer", func(r *rec) { opCase(r, sub, tensor(28, 3), tensor(29, 2, 3)) }},
		{"op/sub/mismatch", func(r *rec) { opCase(r, sub, tensor(30, 2, 3), tensor(31, 2)) }},
		{"op/mul/same", func(r *rec) { opCase(r, mul, tensor(32, 2, 3), tensor(33, 2, 3)) }},
		{"op/mul/scalar", func(r *rec) { opCase(r, mul, tensor(34, 2, 3), tensor(35)) }},
		{"op/mul/suffix", func(r *rec) { opCase(r, mul, tensor(36, 2, 3), tensor(37, 3)) }},
		{"op/mul/swap", func(r *rec) { opCase(r, mul, tensor(38, 3), tensor(39, 2, 3)) }},
		{"op/mul/mismatch", func(r *rec) { opCase(r, mul, tensor(40, 2, 3), tensor(41, 2)) }},
		{"op/mul/same_input", func(r *rec) {
			opCase(r, func(in []*nn.Tensor) *nn.Tensor { return nn.Mul(in[0], in[0]) }, tensor(42, 2, 3))
		}},
		{"op/div/same", func(r *rec) { opCase(r, div, tensor(43, 2, 3), lit(positive(44, 6), 2, 3)) }},
		{"op/div/scalar", func(r *rec) { opCase(r, div, tensor(45, 2, 3), lit(positive(46, 1))) }},
		{"op/div/suffix", func(r *rec) { opCase(r, div, tensor(47, 2, 3), lit(positive(48, 3), 3)) }},
		{"op/div/longer", func(r *rec) { opCase(r, div, tensor(49, 3), tensor(50, 2, 3)) }},
		{"op/div/mismatch", func(r *rec) { opCase(r, div, tensor(51, 2, 3), tensor(52, 2)) }},
		{"op/div/zero", func(r *rec) {
			opCase(r, div, lit([]float32{1, -1, 0, 0, 2, bits(0x80000000)}, 2, 3),
				lit([]float32{0, 0, 0, bits(0x80000000), 0, 1}, 2, 3))
		}},
		{"op/square", func(r *rec) { opCase(r, unary(nn.Square), tensor(53, 2, 3)) }},
		{"op/square/long", func(r *rec) { opCase(r, unary(nn.Square), scaled(54, 1e20, 37)) }},
		{"op/pow/same", func(r *rec) { opCase(r, pow, lit(positive(55, 6), 2, 3), scaled(56, 3, 2, 3)) }},
		{"op/pow/scalar", func(r *rec) { opCase(r, pow, lit(positive(57, 6), 2, 3), lit([]float32{2})) }},
		{"op/pow/suffix_quirk", func(r *rec) { opCase(r, pow, lit(positive(58, 6), 2, 3), tensor(59, 3)) }},
		{"op/pow/longer", func(r *rec) { opCase(r, pow, lit(positive(60, 3), 3), tensor(61, 2, 3)) }},
		{"op/pow/specials", func(r *rec) {
			opCase(r, pow, lit([]float32{0, -2, 2, 0.5, -0.5, 4}, 6), lit([]float32{2, 3, 0.5, -1, 0.5, 0}, 6))
		}},
		{"op/exp/specials", func(r *rec) {
			opCase(r, unary(nn.Exp), lit([]float32{-104, -103.97, -88, -87.5, -1, -1e-8, 0, bits(0x80000000),
				1e-8, 0.5, 1, 10, 88.7, 88.72284, 88.73, 89, 710, 1e30, nan, inf, -inf}, 21))
		}},
		{"op/exp/range", func(r *rec) { opCase(r, unary(nn.Exp), scaled(62, 100, 50)) }},
		{"op/log/specials", func(r *rec) {
			opCase(r, unary(nn.Log), lit([]float32{0, bits(0x80000000), 1, 2, 0.5, 1e-38, 1e-45, 3.4e38, -1,
				nan, inf, -inf}, 12))
		}},
		{"op/log/range", func(r *rec) { opCase(r, unary(nn.Log), lit(positive(63, 40), 5, 8)) }},
		{"op/sin", func(r *rec) { opCase(r, unary(nn.Sin), scaled(64, 10, 4, 5)) }},
		{"op/cos", func(r *rec) { opCase(r, unary(nn.Cos), scaled(65, 10, 4, 5)) }},
		{"op/sin/large", func(r *rec) {
			opCase(r, unary(nn.Sin), lit([]float32{1e5, 1e10, -3e7, 3.4e38, nan, inf, 3.1415927, -0.0}, 8))
		}},
		{"op/cos/large", func(r *rec) {
			opCase(r, unary(nn.Cos), lit([]float32{1e5, 1e10, -3e7, 3.4e38, nan, inf, 3.1415927, 0}, 8))
		}},
		{"op/abs/specials", func(r *rec) {
			opCase(r, unary(nn.Abs), lit([]float32{bits(0x80000000), 0, -1.5, 2, nan, -inf, inf, -1e-45}, 8))
		}},
		{"op/sum/all", func(r *rec) { opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Sum(x) }), tensor(66, 3, 4)) }},
		{"op/sum/long", func(r *rec) { opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Sum(x) }), tensor(67, 1000)) }},
		{"op/sum/two_alongs", func(r *rec) {
			opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Sum(x, 0, 1) }), tensor(68, 2, 3))
		}},
		{"op/mean", func(r *rec) { opCase(r, unary(nn.Mean), tensor(69, 3, 4)) }},
		{"op/mean/long", func(r *rec) { opCase(r, unary(nn.Mean), tensor(70, 999)) }},
		{"op/mse", func(r *rec) { opCase(r, binop(nn.MeanSquareError), tensor(71, 4, 3), tensor(72, 4, 3)) }},
	}
	for _, along := range []int{-1, 0, 1, 2, 3} {
		cs = append(cs, testCase{fmt.Sprintf("op/sum/along%d", along), func(r *rec) {
			opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Sum(x, along) }), tensor(73, 2, 3, 2))
		}})
	}
	cs = append(cs, testCase{"op/sum/along_1d", func(r *rec) {
		opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Sum(x, 0) }), tensor(74, 5))
	}})

	// MatMul over the four transpositions, shapes that reach the kernels'
	// tails, and job counts that split the rows differently.
	type mmShape struct{ m, k, n int }
	seed := uint64(100)
	for _, t1 := range []bool{false, true} {
		for _, t2 := range []bool{false, true} {
			for _, s := range []mmShape{{2, 3, 4}, {9, 17, 5}, {33, 20, 31}, {64, 64, 10}} {
				jobs := []int{0, 3}
				if s.m == 33 {
					jobs = []int{0, 1, 3, 8, 100}
				}
				for _, j := range jobs {
					xs, ys := []int{s.m, s.k}, []int{s.k, s.n}
					if t1 {
						xs = []int{s.k, s.m}
					}
					if t2 {
						ys = []int{s.n, s.k}
					}
					sx, sy := seed, seed+1
					seed += 2
					t1, t2, j := t1, t2, j
					cs = append(cs, testCase{fmt.Sprintf("op/matmul/%s%s/%dx%dx%d/j%d", tf(t1), tf(t2), s.m, s.k, s.n, j), func(r *rec) {
						opCase(r, func(in []*nn.Tensor) *nn.Tensor { return nn.MatMul(in[0], in[1], t1, t2, j) },
							tensor(sx, xs...), tensor(sy, ys...))
					}})
				}
			}
			t1, t2 := t1, t2
			cs = append(cs, testCase{fmt.Sprintf("op/matmul/%s%s/mismatch", tf(t1), tf(t2)), func(r *rec) {
				opCase(r, func(in []*nn.Tensor) *nn.Tensor { return nn.MatMul(in[0], in[1], t1, t2, 0) },
					tensor(200, 2, 3), tensor(201, 4, 5))
			}})
		}
	}
	cs = append(cs, testCase{"op/matmul/not2d", func(r *rec) {
		opCase(r, func(in []*nn.Tensor) *nn.Tensor { return nn.MatMul(in[0], in[1], false, false, 0) },
			tensor(202, 2, 3, 1), tensor(203, 3, 4))
	}})

	// BMM likewise; fm.go calls it with transpose1.
	type bmmShape struct{ b, m, k, n int }
	seed = 300
	for _, t1 := range []bool{false, true} {
		for _, t2 := range []bool{false, true} {
			for _, s := range []bmmShape{{2, 2, 3, 4}, {3, 7, 9, 5}, {5, 16, 17, 8}} {
				for _, j := range []int{0, 4} {
					xs, ys := []int{s.b, s.m, s.k}, []int{s.b, s.k, s.n}
					if t1 {
						xs = []int{s.b, s.k, s.m}
					}
					if t2 {
						ys = []int{s.b, s.n, s.k}
					}
					sx, sy := seed, seed+1
					seed += 2
					t1, t2, j := t1, t2, j
					cs = append(cs, testCase{fmt.Sprintf("op/bmm/%s%s/%dx%dx%dx%d/j%d", tf(t1), tf(t2), s.b, s.m, s.k, s.n, j), func(r *rec) {
						opCase(r, func(in []*nn.Tensor) *nn.Tensor { return nn.BMM(in[0], in[1], t1, t2, j) },
							tensor(sx, xs...), tensor(sy, ys...))
					}})
				}
			}
			t1, t2 := t1, t2
			cs = append(cs, testCase{fmt.Sprintf("op/bmm/%s%s/mismatch", tf(t1), tf(t2)), func(r *rec) {
				opCase(r, func(in []*nn.Tensor) *nn.Tensor { return nn.BMM(in[0], in[1], t1, t2, 0) },
					tensor(400, 2, 2, 3), tensor(401, 3, 5, 6))
			}})
		}
	}
	cs = append(cs, testCase{"op/bmm/not3d", func(r *rec) {
		opCase(r, func(in []*nn.Tensor) *nn.Tensor { return nn.BMM(in[0], in[1], false, false, 0) },
			tensor(402, 2, 3), tensor(403, 3, 4))
	}})

	cs = append(cs, []testCase{
		{"op/broadcast/1d", func(r *rec) {
			opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Broadcast(x, 3) }), tensor(500, 2))
		}},
		{"op/broadcast/2d", func(r *rec) {
			opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Broadcast(x, 3, 2) }), tensor(501, 2, 2))
		}},
		{"op/broadcast/none", func(r *rec) {
			opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Broadcast(x) }), tensor(502, 2, 3))
		}},
		{"op/flatten", func(r *rec) { opCase(r, unary(nn.Flatten), tensor(503, 2, 3)) }},
		{"op/flatten/scalar", func(r *rec) { opCase(r, unary(nn.Flatten), tensor(504)) }},
		{"op/reshape", func(r *rec) {
			opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Reshape(x, 3, 2) }), tensor(505, 2, 3))
		}},
		{"op/reshape/mismatch", func(r *rec) {
			opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Reshape(x, 4) }), tensor(506, 2, 3))
		}},
		{"op/reshape/scalar_to_1", func(r *rec) {
			opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Reshape(x, 1) }), tensor(507))
		}},
		{"op/reshape/to_scalar", func(r *rec) {
			opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Reshape(x) }), tensor(508, 1))
		}},
		{"op/embedding/2d", func(r *rec) {
			opCase(r, binop(nn.Embedding), tensor(509, 6, 2), lit([]float32{0, 1, 0, 3, 0, 5}, 2, 3))
		}},
		{"op/embedding/3d", func(r *rec) {
			opCase(r, binop(nn.Embedding), tensor(510, 6, 1, 2), lit([]float32{0, 1, 0, 3, 0, 5}, 2, 3))
		}},
		{"op/embedding/truncation", func(r *rec) {
			opCase(r, binop(nn.Embedding), tensor(511, 6, 2), lit([]float32{2.7, 0.2, 5.99, -0.5}, 4))
		}},
		{"op/embedding/nan_dim2", func(r *rec) {
			opCase(r, binop(nn.Embedding), tensor(512, 6, 2), lit([]float32{nan, 1}, 2))
		}},
		{"op/embedding/nan_dim1", func(r *rec) {
			opCase(r, binop(nn.Embedding), tensor(513, 6, 1), lit([]float32{nan, 1}, 2))
		}},
		{"op/embedding/out_of_range", func(r *rec) {
			opCase(r, binop(nn.Embedding), tensor(514, 6, 2), lit([]float32{6}, 1))
		}},
		{"op/embedding/negative", func(r *rec) {
			opCase(r, binop(nn.Embedding), tensor(515, 6, 2), lit([]float32{-1}, 1))
		}},
		{"op/embedding/huge", func(r *rec) {
			opCase(r, binop(nn.Embedding), tensor(516, 6, 2), lit([]float32{3e9}, 1))
		}},
		{"op/embedding/view", func(r *rec) {
			w := tensor(517, 6, 2).Slice(0, 3)
			opCase(r, binop(nn.Embedding), w, lit([]float32{4, 1}, 2))
		}},
		{"op/sigmoid", func(r *rec) { opCase(r, unary(nn.Sigmoid), scaled(518, 8, 3, 4)) }},
		{"op/sigmoid/specials", func(r *rec) {
			opCase(r, unary(nn.Sigmoid), lit([]float32{nan, inf, -inf, 100, -100, 0, bits(0x80000000)}, 7))
		}},
		{"op/relu", func(r *rec) { opCase(r, unary(nn.ReLu), tensor(519, 3, 4)) }},
		{"op/relu/specials", func(r *rec) {
			opCase(r, unary(nn.ReLu), lit([]float32{nan, bits(0x7fc12345), bits(0xffc00000), bits(0x7f800001),
				bits(0x80000000), 0, -1, 1, inf, -inf}, 10))
		}},
	}...)
	for _, s := range []struct {
		name  string
		shape []int
		axis  int
	}{
		{"1x3_axis1", []int{1, 3}, 1}, {"2x3_axis1", []int{2, 3}, 1}, {"3x2_axis0", []int{3, 2}, 0},
		{"2x3x4_axis1", []int{2, 3, 4}, 1}, {"4_axis0", []int{4}, 0}, {"2x3_axis2", []int{2, 3}, 2},
		{"2x3_axis-1", []int{2, 3}, -1}, {"3x1_axis1", []int{3, 1}, 1},
	} {
		cs = append(cs, testCase{"op/softmax/" + s.name, func(r *rec) {
			opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Softmax(x, s.axis) }), scaled(520, 3, s.shape...))
		}})
	}
	cs = append(cs, testCase{"op/softmax/go_test", func(r *rec) {
		opCase(r, unary(func(x *nn.Tensor) *nn.Tensor { return nn.Softmax(x, 1) }), lit([]float32{3.0, 1.0, 0.2}, 1, 3))
	}})
	sce := binop(nn.SoftmaxCrossEntropy)
	cs = append(cs, []testCase{
		{"op/sce/go_test", func(r *rec) {
			opCase(r, sce, lit([]float32{0.3, 2.9, 4.0, 0.2, 1.0, 3.0}, 3, 2), lit([]float32{1, 0, 1}, 3))
		}},
		{"op/sce/5x4", func(r *rec) { opCase(r, sce, scaled(521, 3, 5, 4), lit(labels(522, 5, 4), 5)) }},
		{"op/sce/label_truncation", func(r *rec) { opCase(r, sce, tensor(523, 2, 3), lit([]float32{2.9, 0.5}, 2)) }},
		{"op/sce/label_out_of_range", func(r *rec) { opCase(r, sce, tensor(524, 2, 3), lit([]float32{0, 3}, 2)) }},
		{"op/sce/label_nan", func(r *rec) { opCase(r, sce, tensor(525, 2, 3), lit([]float32{nan, 0}, 2)) }},
		{"op/sce/not2d", func(r *rec) { opCase(r, sce, tensor(526, 6), lit([]float32{0}, 1)) }},
		{"op/sce/not1d", func(r *rec) { opCase(r, sce, tensor(527, 2, 3), lit([]float32{0, 1}, 2, 1)) }},
		{"op/sce/size", func(r *rec) { opCase(r, sce, tensor(528, 2, 3), lit([]float32{0, 1, 2}, 3)) }},
		{"op/bce/no_weights", func(r *rec) {
			opCase(r, func(in []*nn.Tensor) *nn.Tensor { return nn.BCEWithLogits(in[0], in[1], nil) },
				lit(signs(529, 8), 8), scaled(530, 5, 8))
		}},
		{"op/bce/weights", func(r *rec) {
			opCase(r, func(in []*nn.Tensor) *nn.Tensor { return nn.BCEWithLogits(in[0], in[1], in[2]) },
				lit(signs(531, 8), 8), scaled(532, 5, 8), lit(positive(533, 8), 8))
		}},
		{"op/bce/extreme", func(r *rec) {
			opCase(r, func(in []*nn.Tensor) *nn.Tensor { return nn.BCEWithLogits(in[0], in[1], nil) },
				lit([]float32{1, -1, 1, -1, 1}, 5), lit([]float32{100, -100, 0, 1e-30, nan}, 5))
		}},
	}...)

	// NaNs of distinct payloads, quiet and signaling, meet in each operation,
	// so that which operand's NaN a result carries is checked.
	nx := func() *nn.Tensor {
		return lit([]float32{bits(0x7fc00011), bits(0x7fc00012), 1, bits(0x7f800013), bits(0xffc00014), 2}, 2, 3)
	}
	ny := func() *nn.Tensor {
		return lit([]float32{bits(0x7fc00021), 3, bits(0x7fc00022), bits(0xffc00023), bits(0x7f800024), bits(0x7fc00025)}, 2, 3)
	}
	nz := func() *nn.Tensor { return lit([]float32{bits(0x7fc00031)}) }
	for _, c := range []struct {
		name string
		f    func([]*nn.Tensor) *nn.Tensor
	}{
		{"add", add}, {"sub", sub}, {"mul", mul}, {"div", div}, {"pow", pow},
		{"mse", binop(nn.MeanSquareError)},
		{"matmul", func(in []*nn.Tensor) *nn.Tensor { return nn.MatMul(in[0], in[1], false, true, 0) }},
		{"maximum_relu", func(in []*nn.Tensor) *nn.Tensor { return nn.ReLu(nn.Add(in[0], in[1])) }},
	} {
		cs = append(cs, testCase{"op/nan/" + c.name, func(r *rec) { opCase(r, c.f, nx(), ny()) }})
	}
	for _, c := range []struct {
		name string
		f    func([]*nn.Tensor) *nn.Tensor
	}{
		{"add_scalar", add}, {"sub_scalar", sub}, {"mul_scalar", mul}, {"div_scalar", div}, {"pow_scalar", pow},
	} {
		cs = append(cs, testCase{"op/nan/" + c.name, func(r *rec) { opCase(r, c.f, nx(), nz()) }})
	}
	for _, c := range []struct {
		name string
		f    func(*nn.Tensor) *nn.Tensor
	}{
		{"sum", func(x *nn.Tensor) *nn.Tensor { return nn.Sum(x) }},
		{"sum_along0", func(x *nn.Tensor) *nn.Tensor { return nn.Sum(x, 0) }},
		{"mean", nn.Mean},
		{"square", nn.Square},
		{"softmax", func(x *nn.Tensor) *nn.Tensor { return nn.Softmax(x, 1) }},
		{"sigmoid", nn.Sigmoid},
	} {
		cs = append(cs, testCase{"op/nan/" + c.name, func(r *rec) { opCase(r, unary(c.f), nx()) }})
	}
	return cs
}
