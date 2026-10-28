// Ports: logics/cf.go @ add21b5

//! The embeddings of matrix factorization collaborative filtering: the item
//! vectors, searchable through an HNSW index ([`MatrixFactorizationItems`]),
//! and the user vectors, looked up by id ([`MatrixFactorizationUsers`]).

use std::collections::HashMap;
use std::io::{Read, Write};

use gorse_common::ann::{self, Hnsw, Index};
use gorse_common::encoding;
use gorse_common::floats;
use gorse_common::gotime::Time;
use gorse_storage::cache::Score;

/// An error from reading or writing the embeddings.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The input ended early, or a value in it is malformed.
    #[error(transparent)]
    Encoding(#[from] encoding::Error),
    /// The HNSW index in the blob is malformed.
    #[error(transparent)]
    Index(#[from] ann::Error),
    /// A count read from the blob is negative, which Go's `make` panics on
    /// (`PORTING_NOTES.md` X-22).
    #[error("negative count {0}")]
    NegativeCount(i64),
}

/// A count read from a blob, which Go hands to `make`.
fn count(n: i64) -> Result<u64, Error> {
    u64::try_from(n).map_err(|_| Error::NegativeCount(n))
}

/// Go's `distance`: the negated dot product, so the nearest vector has the
/// smallest distance.
#[allow(clippy::ptr_arg)] // `Distance<T>` takes `&T`, and `T` is `Vec<f32>`.
fn distance(a: &Vec<f32>, b: &Vec<f32>) -> f32 {
    -floats::dot(a, b)
}

/// Go's `MatrixFactorizationItems`: the item embeddings of a matrix
/// factorization model, searchable by dot product.
pub struct MatrixFactorizationItems {
    timestamp: Time,
    items: Vec<String>,
    index: Hnsw<Vec<f32>>,
    dimension: i64,
}

impl MatrixFactorizationItems {
    /// Go's `NewMatrixFactorizationItems`: an empty index whose scores carry
    /// `timestamp`.
    pub fn new(timestamp: Time) -> Self {
        Self {
            timestamp,
            items: Vec::new(),
            index: Hnsw::new(distance),
            dimension: 0,
        }
    }

    /// Go's `Add`. The first vector sets the dimension; a vector of another
    /// length is logged and dropped.
    ///
    /// Go takes a lock here, so an index shared by goroutines stays
    /// consistent. Gorse adds items from one goroutine (only `cf_test` builds
    /// one), so this takes `&mut self` (`PORTING_NOTES.md` X-WP34-1).
    pub fn add(&mut self, item_id: &str, v: Vec<f32>) {
        if self.dimension == 0 {
            self.dimension = v.len() as i64;
        } else if self.dimension != v.len() as i64 {
            tracing::error!(dimension = v.len(), "dimension mismatch");
            return;
        }
        // The index assigns the next position, which is the end of `items`.
        self.items.push(item_id.to_owned());
        let j = self.index.add(v);
        debug_assert_eq!(j + 1, self.items.len());
    }

    /// Go's `Search`: the `n` items with the highest dot products with `v`,
    /// best first.
    ///
    /// # Panics
    ///
    /// On an empty index, as Go does (`PORTING_NOTES.md` Q-61).
    pub fn search(&self, v: &[f32], n: usize) -> Vec<Score> {
        self.index
            .search_vector(&v.to_vec(), n, false)
            .into_iter()
            .map(|(position, distance)| Score {
                id: self.items[position].clone(),
                score: -f64::from(distance),
                is_hidden: false,
                categories: Vec::new(),
                timestamp: self.timestamp.clone(),
            })
            .collect()
    }

    /// Go's `Marshal`: the timestamp, the dimension, the index, then the
    /// item ids.
    ///
    /// The timestamp is a gob `time.Time` (its `MarshalBinary` bytes as a gob
    /// `GobEncoder` value), so a Go process reads the blob as it reads its own.
    ///
    /// # Errors
    ///
    /// Fails when the writer fails.
    pub fn marshal(&self, w: &mut impl Write) -> Result<(), Error> {
        encoding::write_gob(w, &self.timestamp)?;
        encoding::write_gob(w, &self.dimension)?;
        self.index.marshal(w)?;
        encoding::write_gob(w, &(self.items.len() as i64))?;
        for item in &self.items {
            encoding::write_gob(w, item)?;
        }
        Ok(())
    }

    /// Go's `Unmarshal`: replaces the items with what [`Self::marshal`]
    /// wrote. Unlike Go, which keeps the parts it has read when a later part
    /// fails, this replaces all of them or none (`PORTING_NOTES.md` X-WP34-4).
    ///
    /// # Errors
    ///
    /// Fails when the input ends early or holds a malformed part, or a
    /// negative item count (where Go panics).
    pub fn unmarshal(&mut self, r: &mut impl Read) -> Result<(), Error> {
        let timestamp: Time = encoding::read_gob(r)?;
        let dimension: i64 = encoding::read_gob(r)?;
        let mut index = Hnsw::new(distance);
        index.unmarshal(r)?;
        let num_items = count(encoding::read_gob(r)?)?;
        // Grown as the data arrives, not from the count (X-22).
        let mut items = Vec::new();
        for _ in 0..num_items {
            items.push(encoding::read_gob(r)?);
        }
        self.timestamp = timestamp;
        self.dimension = dimension;
        self.index = index;
        self.items = items;
        Ok(())
    }
}

/// Go's `MatrixFactorizationUsers`: the user embeddings, by user id.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MatrixFactorizationUsers {
    embeddings: HashMap<String, Vec<f32>>,
}

impl MatrixFactorizationUsers {
    /// Go's `NewMatrixFactorizationUsers`: no users.
    pub fn new() -> Self {
        Self::default()
    }

    /// Go's `Add`: stores `v` as the user's embedding, replacing any earlier
    /// one.
    pub fn add(&mut self, user_id: &str, v: Vec<f32>) {
        self.embeddings.insert(user_id.to_owned(), v);
    }

    /// Go's `Get`: the user's embedding, if the user has one.
    pub fn get(&self, user_id: &str) -> Option<&[f32]> {
        self.embeddings.get(user_id).map(Vec::as_slice)
    }

    /// Go's `Marshal`: the user count, then each user's id and embedding in
    /// map order. Go's map order is random, and so is this one.
    ///
    /// # Errors
    ///
    /// Fails when the writer fails.
    pub fn marshal(&self, w: &mut impl Write) -> Result<(), Error> {
        encoding::write_gob(w, &(self.embeddings.len() as i64))?;
        for (user_id, embedding) in &self.embeddings {
            encoding::write_string(w, user_id)?;
            encoding::write_slice(w, embedding.as_slice())?;
        }
        Ok(())
    }

    /// Go's `Unmarshal`: replaces the embeddings with what
    /// [`Self::marshal`] wrote, all or nothing (`PORTING_NOTES.md` X-WP34-4).
    ///
    /// # Errors
    ///
    /// Fails when the input ends early or holds a malformed value, or a
    /// negative user count (where Go panics).
    pub fn unmarshal(&mut self, r: &mut impl Read) -> Result<(), Error> {
        let num_users = count(encoding::read_gob(r)?)?;
        let mut embeddings = HashMap::new();
        for _ in 0..num_users {
            let user_id = encoding::read_string(r)?;
            let embedding = encoding::read_slice::<f32>(r)?;
            embeddings.insert(user_id, embedding);
        }
        self.embeddings = embeddings;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Ports: logics/cf_test.go @ add21b5
    use super::*;
    use gorse_common::gotime::now;
    use std::fs::File;

    #[test]
    fn test_matrix_factorization_items() {
        let ts = now();
        let mut items = MatrixFactorizationItems::new(ts);
        items.add("1", vec![1.0, 1.0, 1.0]);
        items.add("2", vec![2.0, 2.0, 2.0]);
        items.add("3", vec![3.0, 3.0, 3.0]);
        items.add("4", vec![4.0, 4.0, 4.0]);
        items.add("5", vec![5.0, 5.0, 5.0]);

        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("items");
        let mut f = File::create(&path).expect("create");
        items.marshal(&mut f).expect("marshal");
        drop(f);

        let mut f = File::open(&path).expect("open");
        let mut items2 = MatrixFactorizationItems::new(Time::default());
        items2.unmarshal(&mut f).expect("unmarshal");

        assert_eq!(items.timestamp.unix_nano(), items2.timestamp.unix_nano());
        assert_eq!(items.dimension, items2.dimension);
        assert_eq!(items.items, items2.items);
        let scores = items2.search(&[1.0, 1.0, 1.0], 3);
        assert_eq!(
            scores,
            vec![
                Score {
                    id: "5".to_owned(),
                    score: 15.0,
                    is_hidden: false,
                    categories: Vec::new(),
                    timestamp: items2.timestamp.clone(),
                },
                Score {
                    id: "4".to_owned(),
                    score: 12.0,
                    is_hidden: false,
                    categories: Vec::new(),
                    timestamp: items2.timestamp.clone(),
                },
                Score {
                    id: "3".to_owned(),
                    score: 9.0,
                    is_hidden: false,
                    categories: Vec::new(),
                    timestamp: items2.timestamp.clone(),
                },
            ]
        );
    }

    #[test]
    fn test_new_matrix_factorization_users() {
        let mut users = MatrixFactorizationUsers::new();
        users.add("1", vec![1.0, 1.0, 1.0]);
        users.add("2", vec![2.0, 2.0, 2.0]);
        users.add("3", vec![3.0, 3.0, 3.0]);

        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("users");
        let mut f = File::create(&path).expect("create");
        users.marshal(&mut f).expect("marshal");
        drop(f);

        let mut f = File::open(&path).expect("open");
        let mut users2 = MatrixFactorizationUsers::new();
        users2.unmarshal(&mut f).expect("unmarshal");
        assert_eq!(users.embeddings, users2.embeddings);
    }
}
