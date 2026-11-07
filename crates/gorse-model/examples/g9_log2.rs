//! G9 for the ndcg log table (D-354, P4-10): the table `LOG2_TABLE` of
//! `gorse_model::cf::evaluator`, printed for comparison with the Lean oracle `oracle_log2`.
//!
//! The example prints one line for each k in 2..=65: `k 0xBITS`, where `0xBITS` is the binary32
//! bit pattern of the table entry, in upper case hexadecimal. scripts/check-oracle-log2.sh
//! compares these lines, byte for byte, with the lines of the oracle.
//!
//! Usage:
//!   g9_log2    print the table

use gorse_model::cf::evaluator::LOG2_TABLE;

fn main() {
    for (i, bits) in LOG2_TABLE.iter().enumerate() {
        println!("{} 0x{:08X}", i + 2, bits);
    }
}
