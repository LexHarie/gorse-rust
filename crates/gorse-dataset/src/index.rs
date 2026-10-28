// Ports: dataset/index.go @ add21b5

//! The index that maps sparse IDs (user IDs and item IDs) to dense indices, and
//! its binary form, which model blobs store.

use std::collections::HashMap;
use std::io::{Read, Write};

use gorse_common::encoding;

use crate::Error;

/// Go's `NotId`: the index of an ID that is not in the index.
pub const NOT_ID: i32 = -1;

/// Go's `Index`: maps sparse IDs to dense indices (`numbers`) and back
/// (`names`). A dense index is the position of its sparse ID in `names`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Index {
    /// Sparse ID to dense index (`Numbers`).
    pub numbers: HashMap<String, i32>,
    /// Dense index to sparse ID (`Names`).
    pub names: Vec<String>,
}

impl Index {
    /// `NewMapIndex`: an empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of indexed names (`Len`). Go's nil `*Index` also has length 0;
    /// Rust has no nil index, so a call site with an `Option<&Index>` uses
    /// `map_or(0, Index::len)`.
    pub fn len(&self) -> i32 {
        self.names.len() as i32
    }

    /// Whether no names are indexed.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Adds `name` if it is not indexed yet. A name already present keeps its
    /// index.
    pub fn add(&mut self, name: &str) {
        if !self.numbers.contains_key(name) {
            self.numbers
                .insert(name.to_owned(), self.names.len() as i32);
            self.names.push(name.to_owned());
        }
    }

    /// The dense index of `name`, or [`NOT_ID`] when it is not indexed.
    pub fn to_number(&self, name: &str) -> i32 {
        self.numbers.get(name).copied().unwrap_or(NOT_ID)
    }

    /// The sparse ID at dense index `index`. Panics when `index` is out of range,
    /// as Go's index expression does.
    pub fn to_name(&self, index: i32) -> &str {
        &self.names[index as usize]
    }

    /// All names, in dense-index order (`GetNames`).
    pub fn get_names(&self) -> &[String] {
        &self.names
    }

    /// `Marshal`: a little-endian `i32` count, then each name as an
    /// [`encoding`] string.
    pub fn marshal(&self, w: &mut impl Write) -> Result<(), Error> {
        write_i32(w, self.names.len() as i32)?;
        for name in &self.names {
            encoding::write_string(w, name)?;
        }
        Ok(())
    }

    /// `Unmarshal`: reads what [`Index::marshal`] wrote. A name that repeats an
    /// earlier one is dropped, as [`Index::add`] drops it, so the index can hold
    /// fewer names than the count says.
    pub fn unmarshal(r: &mut impl Read) -> Result<Self, Error> {
        let n = read_i32(r)?;
        if n < 0 {
            // Go's make([]string, 0, n) panics on a negative capacity.
            return Err(Error::NegativeLength(n));
        }
        let mut index = Index::new();
        for _ in 0..n {
            let name = encoding::read_string(r)?;
            index.add(&name);
        }
        Ok(index)
    }
}

/// `MarshalIndex`: writes `index` to `w`.
pub fn marshal_index(w: &mut impl Write, index: &Index) -> Result<(), Error> {
    index.marshal(w)
}

/// `UnmarshalIndex`: reads an [`Index`] from `r`.
pub fn unmarshal_index(r: &mut impl Read) -> Result<Index, Error> {
    Index::unmarshal(r)
}

/// Writes `v` as four little-endian bytes, as Go's `binary.Write` does.
pub(crate) fn write_i32(w: &mut impl Write, v: i32) -> Result<(), Error> {
    w.write_all(&v.to_le_bytes())?;
    Ok(())
}

/// Reads four little-endian bytes as an `i32`, as Go's `binary.Read` does.
pub(crate) fn read_i32(r: &mut impl Read) -> Result<i32, Error> {
    let mut buf = [0; 4];
    r.read_exact(&mut buf)?;
    Ok(i32::from_le_bytes(buf))
}

#[cfg(test)]
mod tests;
