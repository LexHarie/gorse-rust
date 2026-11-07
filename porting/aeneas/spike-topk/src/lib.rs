//! The TopKFilter extraction of the Aeneas spike (P3-2, D-331, brief 4).
//!
//! `heap.rs` in this directory is a symbolic link to the file under
//! `crates/gorse-common/src`, so the code Charon reads is the real `TopKFilter`, not
//! a copy. The translation script dereferences the link when it copies this crate
//! (`cp -rL`), so Charon sees the file itself. `encoding.rs` is a stand-in for the
//! items that `heap.rs` names and `TopKFilter` does not reach.
//!
//! `top_k_u32_i64` is the one entry point: it pushes `items` into a `TopKFilter`
//! over a `u32` value and an `i64` weight, then pops everything. Charon is given
//! `--start-from crate::top_k_u32_i64`, so it translates only what that function
//! reaches, which is the filter and its heap loops.
//!
//! The crate is standalone and is not a member of the root workspace.

mod encoding;
mod heap;

use heap::{Elem, TopKFilter};

/// Pushes every element of `items` into a filter that keeps `k`, then returns the
/// kept elements, heaviest first, in the D-329 order that `pop_all` gives.
pub fn top_k_u32_i64(k: usize, items: &[Elem<u32, i64>]) -> Vec<Elem<u32, i64>> {
    let mut f: TopKFilter<u32, i64> = TopKFilter::new(k);
    let mut i: usize = 0;
    while i < items.len() {
        f.push(items[i].value, items[i].weight);
        i += 1;
    }
    f.pop_all()
}
