import Refine.PopAll
import Refine.TopLoop

/-!
# The `TopKFilter` wrapper and the final theorem (P3-2 refinement, D-331)

`top_k_u32_i64` pushes the items one by one (the loop is in `TopLoop`), then pops the heap into
a sorted vector. The final theorem says that the extracted function returns the model's list
of all the items, best first.

The model facts used here come from the core of the spec, `Gorse.Spec.TopKRegion` (D-331). The
rest of the spec, including `topKFilter_correct`, is not imported: it needs the older toolchain.
-/

open Aeneas Aeneas.Std Result ControlFlow Error
open aeneas_spike_topk Gorse.Spec.TopK

namespace Refine

/-- The extracted `top_k_u32_i64` returns the model's top-k of its items, best first, for any
`k` and any input shorter than `2^59`. -/
theorem top_k_u32_i64_eq (k : Std.Usize) (items : Slice (heap.Elem Std.U32 Std.I64))
    (hL : items.val.length < 2 ^ 59) (hplat : 2 ^ 60 ≤ Usize.max) :
    ∃ v, top_k_u32_i64 k items = ok v ∧ v.val = topKModel wv kv k.val items.val := by
  have hnew : heap.TopKFilter.new core.cmp.OrdU32 core.cmp.PartialOrdI64 k =
      ok { elems := alloc.vec.Vec.new (heap.Elem Std.U32 Std.I64), k := k, nan := 0#usize } :=
    rfl
  have h0 : (0#usize).val = 0 := by scalar_tac
  have hI0 : TopState k.val items.val
      ({ elems := alloc.vec.Vec.new (heap.Elem Std.U32 Std.I64), k := k, nan := 0#usize },
        0#usize) := by
    refine ⟨?_, rfl, ?_, ?_⟩
    · show (0#usize).val ≤ items.val.length
      omega
    · show (alloc.vec.Vec.new (heap.Elem Std.U32 Std.I64)).val.Perm
        (topKModel wv kv k.val (items.val.take (0#usize).val))
      simp only [h0, List.take_zero, topKModel_nil_eq, vec_new_val, List.Perm.refl]
    · exact heapOrd_nil _
  obtain ⟨f, hf, hfP⟩ := top_k_loop_eq k.val items hL _ 0#usize hI0 hplat
  have hbuf : (topKModel wv kv k.val items.val).length < 2 ^ 59 := by
    have h1 := topK_length_le k.val items.val
    have h2 : min k.val items.val.length ≤ items.val.length := min_le_right _ _
    omega
  obtain ⟨v, self', hpa, hv⟩ := pop_all_eq f (topKModel wv kv k.val items.val) hbuf hfP.1
    (topKModel_sorted k.val items.val) hfP.2 hplat
  refine ⟨v, ?_, hv⟩
  unfold top_k_u32_i64
  simp only [hnew, bind_tc_ok, bind_ok, hf, hpa]

end Refine
