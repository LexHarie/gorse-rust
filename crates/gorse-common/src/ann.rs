// Ports: common/ann/ann.go, common/ann/bruteforce.go, common/ann/hnsw.go @ add21b5

//! Nearest-neighbour indexes over vectors of any type `T` with a distance
//! function: [`Bruteforce`], exact, and [`Hnsw`], a Hierarchical Navigable
//! Small World graph. At the pinned commit Gorse builds an HNSW only in
//! `logics.MatrixFactorizationItems`, which only its tests create.
//!
//! Both follow the Go code step for step on the heap port, so equal distances
//! come out in Go's order and an HNSW built from the same vectors and the same
//! layer draws is the same graph, down to its marshalled bytes. The quirks are
//! Go's (`PORTING_NOTES.md`, "Vector indexes"): `Bruteforce::add` returns the
//! new length while `Hnsw::add` returns the index, `Hnsw::search_index` finds
//! the query vector itself while `Bruteforce::search_index` leaves it out, and
//! an HNSW grows by at most one layer per insert.
//!
//! Go's HNSW takes inserts from many goroutines behind its own locks; here
//! [`Hnsw::add`] takes `&mut self` and searches take `&self` (X-21).

use std::cmp::max;
use std::collections::{BTreeMap, HashSet};
use std::hash::{BuildHasher, RandomState};
use std::io::{Read, Write};

use crate::encoding::gob::GobValue;
use crate::encoding::{self, Element};
use crate::heap::PriorityQueue;
use crate::math32;

/// A distance between two vectors (Go `func(a, b T) float32`).
pub type Distance<T> = fn(&T, &T) -> f32;

/// A search result: the vector's index and its distance to the query (Go
/// `lo.Tuple2[int, float32]`).
pub type Neighbor = (usize, f32);

/// Errors of the indexes.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `search_index` was given an index the index does not hold.
    #[error("index out of range: {0}")]
    IndexOutOfRange(usize),
    /// A count read by `Hnsw::unmarshal` is negative; Go panics in
    /// `makeslice` (`PORTING_NOTES.md` X-22).
    #[error("negative count {0}")]
    NegativeCount(i64),
    /// The input ended early, or a vector or queue in it is malformed.
    #[error(transparent)]
    Encoding(#[from] encoding::Error),
}

/// Go's `ann.Index`: an index that takes vectors and finds the `k` nearest of
/// them.
pub trait Index<T> {
    /// Adds a vector. What the return value means depends on the index, as in
    /// Go: the new length for [`Bruteforce`], the vector's index for [`Hnsw`].
    fn add(&mut self, v: T) -> usize;

    /// The `k` vectors nearest to the stored vector `q`, nearest first; with
    /// `prune0`, only those at a distance above zero.
    ///
    /// # Errors
    ///
    /// [`Error::IndexOutOfRange`] when the index holds no vector `q`.
    fn search_index(&self, q: usize, k: usize, prune0: bool) -> Result<Vec<Neighbor>, Error>;

    /// The `k` vectors nearest to `q`, nearest first; with `prune0`, only
    /// those at a distance above zero.
    fn search_vector(&self, q: &T, k: usize, prune0: bool) -> Vec<Neighbor>;
}

/// `int` of a count Go compares with lengths.
fn signed(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// The index of a vector held in a queue; Go indexes its slices with the
/// `int32` and panics on a negative one.
fn slot(e: i32) -> usize {
    usize::try_from(e).unwrap_or_else(|_| panic!("index out of range [{e}]"))
}

/// Pops a queue to the end, keeping what `prune0` lets through.
fn drain(mut pq: PriorityQueue, prune0: bool) -> Vec<Neighbor> {
    let mut scores = Vec::new();
    while let Some((value, score)) = pq.pop() {
        if !prune0 || score > 0.0 {
            scores.push((slot(value), score));
        }
    }
    scores
}

/// The exact index: every search computes the distance to every vector (Go
/// `ann.Bruteforce`).
pub struct Bruteforce<T> {
    distance: Distance<T>,
    vectors: Vec<T>,
}

impl<T> std::fmt::Debug for Bruteforce<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bruteforce")
            .field("len", &self.vectors.len())
            .finish_non_exhaustive()
    }
}

impl<T> Bruteforce<T> {
    /// `NewBruteforce`.
    pub fn new(distance: Distance<T>) -> Self {
        Self {
            distance,
            vectors: Vec::new(),
        }
    }

    /// Keeps the `k` nearest of `distances` in a descending queue, as Go does:
    /// push, then pop the farthest once the queue holds more than `k`.
    fn nearest(
        &self,
        distances: impl Iterator<Item = (usize, f32)>,
        k: usize,
        prune0: bool,
    ) -> Vec<Neighbor> {
        let mut pq = PriorityQueue::new(true);
        for (i, d) in distances {
            pq.push(
                i32::try_from(i).expect("more vectors than int32 indexes"),
                d,
            );
            if pq.len() > k {
                pq.pop();
            }
        }
        drain(pq.reverse(), prune0)
    }
}

impl<T> Index<T> for Bruteforce<T> {
    /// Returns the number of vectors after the add, one more than the new
    /// vector's index (`PORTING_NOTES.md` Q-54).
    fn add(&mut self, v: T) -> usize {
        self.vectors.push(v);
        self.vectors.len()
    }

    /// Leaves `q` itself out.
    fn search_index(&self, q: usize, k: usize, prune0: bool) -> Result<Vec<Neighbor>, Error> {
        let query = self.vectors.get(q).ok_or(Error::IndexOutOfRange(q))?;
        let distances = self
            .vectors
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != q)
            .map(|(i, v)| (i, (self.distance)(query, v)));
        Ok(self.nearest(distances, k, prune0))
    }

    fn search_vector(&self, q: &T, k: usize, prune0: bool) -> Vec<Neighbor> {
        let distances = self
            .vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (i, (self.distance)(q, v)));
        self.nearest(distances, k, prune0)
    }
}

/// A source of uniform floats in `[0, 1)`, Go's `rand.Float32`.
type Uniform = Box<dyn FnMut() -> f32 + Send + Sync>;

/// A uniform source made the way Go's `rand.Float32` makes its floats, from 63
/// random bits, resampling the rare 1.0. Go's global source, which HNSW draws
/// from, is randomly seeded, so no stream is expected here; this one is
/// splitmix64 from a random seed.
fn random_uniform() -> Uniform {
    let mut state = RandomState::new().hash_one("gorse ann");
    Box::new(move || {
        loop {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            // Float64: Int63() / 2^63, then Float32 of that.
            let f = (z >> 1) as f64 / 9_223_372_036_854_775_808.0;
            if f == 1.0 {
                continue;
            }
            let f = f as f32;
            if f != 1.0 {
                return f;
            }
        }
    })
}

/// The layer `HNSW.insert` gives a new vector for the uniform `u`:
/// `int(math32.Floor(-math32.Log(u) * levelFactor))`. A zero draw makes the
/// product `+Inf`, which `int` turns into `math.MinInt64` on amd64.
fn level(u: f32, level_factor: f32) -> i64 {
    math32::go_f32_to_i64(math32::floor(-math32::log(u) * level_factor))
}

/// The Hierarchical Navigable Small World index (Go `ann.HNSW`).
///
/// Layer 0 holds every vector; upper layer `l` (1-based, like Go's
/// `upperNeighbors[l-1]`) holds the vectors drawn at level `l` or above.
/// Each vector's neighbours in a layer are an ascending [`PriorityQueue`] of
/// distances.
pub struct Hnsw<T> {
    distance: Distance<T>,
    uniform: Uniform,
    vectors: Vec<T>,
    bottom_neighbors: Vec<PriorityQueue>,
    upper_neighbors: Vec<BTreeMap<i32, PriorityQueue>>,
    enter_point: i32,
    /// Go's `upperNeighbors != nil`: false until the first insert or an
    /// unmarshal; the first insert into a fresh index only sets the enter
    /// point.
    started: bool,
    level_factor: f32,
    max_connection: i64,
    max_connection0: i64,
    ef: i64,
    ef_construction: i64,
}

impl<T> std::fmt::Debug for Hnsw<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hnsw")
            .field("len", &self.vectors.len())
            .field("layers", &self.upper_neighbors.len())
            .field("enter_point", &self.enter_point)
            .finish_non_exhaustive()
    }
}

impl<T> Hnsw<T> {
    /// `NewHNSW`: M = 48 (96 at layer 0), efConstruction = 100, layers drawn
    /// with factor `1 / ln(48)` from a randomly seeded source.
    pub fn new(distance: Distance<T>) -> Self {
        Self::with_uniform(distance, random_uniform())
    }

    /// [`Hnsw::new`] drawing the layers from `uniform`, which must return
    /// floats in `[0, 1)` as Go's `rand.Float32` does. Every insert but the
    /// first into a fresh index draws one.
    pub fn with_uniform(
        distance: Distance<T>,
        uniform: impl FnMut() -> f32 + Send + Sync + 'static,
    ) -> Self {
        Self {
            distance,
            uniform: Box::new(uniform),
            vectors: Vec::new(),
            bottom_neighbors: Vec::new(),
            upper_neighbors: Vec::new(),
            enter_point: 0,
            started: false,
            level_factor: 1.0 / math32::log(48.0),
            max_connection: 48,
            max_connection0: 96,
            ef: 0,
            ef_construction: 100,
        }
    }

    /// The number of vectors.
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    /// Whether the index holds no vector.
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }

    /// The number of upper layers.
    pub fn layers(&self) -> usize {
        self.upper_neighbors.len()
    }

    /// `knnSearch`: greedy descent through the upper layers, then a search
    /// of breadth `ef` at layer 0.
    fn knn_search(&self, q: &T, k: i64, ef: i64) -> PriorityQueue {
        let mut enter_points = self.distances(q, &[self.enter_point]);
        let mut layer = signed(self.upper_neighbors.len());
        while layer > 0 {
            let w = self.search_layer(q, &enter_points, 1, layer);
            enter_points = PriorityQueue::new(false);
            let (v, d) = w.peek().expect("index out of range [0] with length 0");
            enter_points.push(v, d);
            layer -= 1;
        }
        let w = self.search_layer(q, &enter_points, ef, 0);
        select_neighbors(&w, k)
    }

    /// `insert`: links the vector `q`, already stored, into the graph.
    fn insert(&mut self, q: i32) {
        let qi = slot(q);
        if !self.started {
            self.bottom_neighbors[qi] = PriorityQueue::new(false);
            self.started = true;
            self.enter_point = q;
            return;
        }
        // Go computes the enter point's distance before it draws the level.
        let mut enter_points = self.distances(&self.vectors[qi], &[self.enter_point]);
        let l = level((self.uniform)(), self.level_factor);
        let top_layer = signed(self.upper_neighbors.len());

        let mut layer = top_layer;
        while layer >= l.wrapping_add(1) {
            let w = self.search_layer(&self.vectors[qi], &enter_points, 1, layer);
            enter_points = select_neighbors(&w, 1);
            layer -= 1;
        }

        let mut layer = top_layer.min(l);
        while layer >= 0 {
            let w = self.search_layer(
                &self.vectors[qi],
                &enter_points,
                self.ef_construction,
                layer,
            );
            let neighbors = select_neighbors(&w, self.max_connection);
            // Values in a queue are distinct, so the order is Go's whatever
            // sort it uses.
            let mut elems = neighbors.elems().to_vec();
            elems.sort_unstable_by_key(|e| e.value);
            self.set_neighbourhood(q, layer, neighbors);
            let max_connection = self.max_connection;
            let current_max = if layer == 0 {
                self.max_connection0
            } else {
                max_connection
            };
            for e in elems {
                let connections = self.neighbourhood_mut(e.value, layer);
                connections.push(q, e.weight);
                if signed(connections.len()) > current_max {
                    // Shrinks to maxConnection even at layer 0.
                    let shrunk = select_neighbors(connections, max_connection);
                    self.set_neighbourhood(e.value, layer, shrunk);
                }
            }
            enter_points = w;
            layer -= 1;
        }

        if l > top_layer {
            // One new layer, however high l is.
            self.enter_point = q;
            self.upper_neighbors.push(BTreeMap::new());
            self.set_neighbourhood(q, top_layer + 1, PriorityQueue::new(false));
        }
    }

    /// `searchLayer`: the `ef` nearest vectors to `q` reachable in `layer`
    /// from `enter_points`, as an ascending queue.
    fn search_layer(
        &self,
        q: &T,
        enter_points: &PriorityQueue,
        ef: i64,
        layer: i64,
    ) -> PriorityQueue {
        let mut visited: HashSet<i32> = enter_points.values().into_iter().collect();
        let mut candidates = enter_points.clone_queue();
        let mut w = enter_points.reverse();
        while let Some((c, cq)) = candidates.pop() {
            let (_, fq) = w.peek().expect("index out of range [0] with length 0");
            if cq > fq {
                break;
            }
            for e in self.neighbourhood(c, layer).values() {
                if visited.insert(e) {
                    let (_, fq) = w.peek().expect("index out of range [0] with length 0");
                    let eq = (self.distance)(&self.vectors[slot(e)], q);
                    if eq < fq || signed(w.len()) < ef {
                        candidates.push(e, eq);
                        w.push(e, eq);
                        if signed(w.len()) > ef {
                            w.pop();
                        }
                    }
                }
            }
        }
        w.reverse()
    }

    /// The position of upper layer `layer` (1-based) in `upper_neighbors`;
    /// Go indexes its slice with `layer - 1` and panics outside it.
    fn upper(&self, layer: i64) -> usize {
        match usize::try_from(layer - 1) {
            Ok(i) if i < self.upper_neighbors.len() => i,
            _ => panic!("index out of range [{}]", layer - 1),
        }
    }

    /// `getNeighbourhood`; Go returns nil for a vector missing from an upper
    /// layer and its caller dereferences it.
    fn neighbourhood(&self, e: i32, layer: i64) -> &PriorityQueue {
        if layer == 0 {
            return &self.bottom_neighbors[slot(e)];
        }
        self.upper_neighbors[self.upper(layer)]
            .get(&e)
            .unwrap_or_else(|| panic!("nil pointer dereference: no vector {e} in layer {layer}"))
    }

    fn neighbourhood_mut(&mut self, e: i32, layer: i64) -> &mut PriorityQueue {
        if layer == 0 {
            return &mut self.bottom_neighbors[slot(e)];
        }
        let i = self.upper(layer);
        self.upper_neighbors[i]
            .get_mut(&e)
            .unwrap_or_else(|| panic!("nil pointer dereference: no vector {e} in layer {layer}"))
    }

    /// `setNeighbourhood`.
    fn set_neighbourhood(&mut self, e: i32, layer: i64, connections: PriorityQueue) {
        if layer == 0 {
            self.bottom_neighbors[slot(e)] = connections;
        } else {
            let i = self.upper(layer);
            self.upper_neighbors[i].insert(e, connections);
        }
    }

    /// `distance`: an ascending queue of the distances from `q` to `points`.
    fn distances(&self, q: &T, points: &[i32]) -> PriorityQueue {
        let mut pq = PriorityQueue::new(false);
        for &point in points {
            pq.push(point, (self.distance)(&self.vectors[slot(point)], q));
        }
        pq
    }

    /// `efSearchValue`.
    fn ef_search_value(&self, n: i64) -> i64 {
        if self.ef > 0 {
            max(self.ef, n)
        } else {
            max(self.ef_construction, n)
        }
    }
}

/// `selectNeighbors`: the `m` nearest of `candidates`, as an ascending queue.
fn select_neighbors(candidates: &PriorityQueue, m: i64) -> PriorityQueue {
    let mut pq = candidates.reverse();
    while signed(pq.len()) > m {
        pq.pop();
    }
    pq.reverse()
}

impl<T> Index<T> for Hnsw<T> {
    /// Returns the new vector's index.
    fn add(&mut self, v: T) -> usize {
        self.vectors.push(v);
        self.bottom_neighbors.push(PriorityQueue::new(false));
        let q = self.vectors.len() - 1;
        self.insert(i32::try_from(q).expect("more vectors than int32 indexes"));
        q
    }

    /// Finds `q` itself too, at distance zero unless the distance says
    /// otherwise (`PORTING_NOTES.md` Q-55).
    fn search_index(&self, q: usize, k: usize, prune0: bool) -> Result<Vec<Neighbor>, Error> {
        let query = self.vectors.get(q).ok_or(Error::IndexOutOfRange(q))?;
        let k = signed(k);
        Ok(drain(
            self.knn_search(query, k, self.ef_search_value(k)),
            prune0,
        ))
    }

    /// # Panics
    ///
    /// On an empty index, as Go does: there is no enter point.
    fn search_vector(&self, q: &T, k: usize, prune0: bool) -> Vec<Neighbor> {
        let k = signed(k);
        drain(self.knn_search(q, k, self.ef_search_value(k)), prune0)
    }
}

fn read_le<E: Element>(r: &mut impl Read) -> Result<E, encoding::Error> {
    let mut b = [0; 8];
    let b = &mut b[..E::SIZE];
    r.read_exact(b)?;
    Ok(E::get(b))
}

fn write_le<E: Element>(w: &mut impl Write, v: E) -> Result<(), encoding::Error> {
    let mut b = Vec::with_capacity(E::SIZE);
    v.put(&mut b);
    w.write_all(&b)?;
    Ok(())
}

impl<T: GobValue> Hnsw<T> {
    /// `Marshal`, all little-endian: the level factor (`f32`); maxConnection,
    /// maxConnection0, ef and efConstruction (`i64`); the vector count
    /// (`i64`) and each vector with [`encoding::write_gob`]; each vector's
    /// layer-0 queue; the layer count (`i64`) and, per layer, its entry count
    /// (`i32`) and each entry's vector (`i32`) and queue; the enter point
    /// (`i32`). Go writes a layer's entries in `sync.Map` order, which changes
    /// from run to run; this writes them by vector index, one of Go's orders.
    ///
    /// # Errors
    ///
    /// Fails when the writer fails.
    pub fn marshal(&self, w: &mut impl Write) -> Result<(), encoding::Error> {
        write_le(w, self.level_factor)?;
        for v in [
            self.max_connection,
            self.max_connection0,
            self.ef,
            self.ef_construction,
        ] {
            write_le(w, v)?;
        }
        write_le(w, signed(self.vectors.len()))?;
        for v in &self.vectors {
            encoding::write_gob(w, v)?;
        }
        for pq in &self.bottom_neighbors {
            pq.marshal(w)?;
        }
        write_le(w, signed(self.upper_neighbors.len()))?;
        for layer in &self.upper_neighbors {
            // Go's int32(len(elements)).
            write_le(w, layer.len() as i32)?;
            for (&e, pq) in layer {
                write_le(w, e)?;
                pq.marshal(w)?;
            }
        }
        write_le(w, self.enter_point)
    }

    /// `Unmarshal`: replaces the index with what [`Hnsw::marshal`] wrote,
    /// keeping the distance and the uniform source. The connection limits and
    /// search breadths come from the input. A negative entry count reads as
    /// zero, as in Go.
    ///
    /// Go leaves the index half-replaced on an error; this leaves it as it
    /// was (`PORTING_NOTES.md` X-22).
    ///
    /// # Errors
    ///
    /// Fails when the input ends early, holds a malformed vector or queue, or
    /// a negative vector or layer count (where Go panics).
    pub fn unmarshal(&mut self, r: &mut impl Read) -> Result<(), Error> {
        let level_factor: f32 = read_le(r)?;
        let max_connection: i64 = read_le(r)?;
        let max_connection0: i64 = read_le(r)?;
        let ef: i64 = read_le(r)?;
        let ef_construction: i64 = read_le(r)?;
        let n = count(read_le(r)?)?;
        // Grown as the data arrives, not from the counts (X-22).
        let mut vectors = Vec::new();
        for _ in 0..n {
            vectors.push(encoding::read_gob(r)?);
        }
        let mut bottom_neighbors = Vec::new();
        for _ in 0..n {
            bottom_neighbors.push(PriorityQueue::unmarshal(r)?);
        }
        let layers = count(read_le(r)?)?;
        let mut upper_neighbors = Vec::new();
        for _ in 0..layers {
            let mut layer = BTreeMap::new();
            let entries: i32 = read_le(r)?;
            for _ in 0..entries {
                let e: i32 = read_le(r)?;
                // A key seen before is replaced, as sync.Map.Store does.
                layer.insert(e, PriorityQueue::unmarshal(r)?);
            }
            upper_neighbors.push(layer);
        }
        let enter_point: i32 = read_le(r)?;
        self.vectors = vectors;
        self.bottom_neighbors = bottom_neighbors;
        self.upper_neighbors = upper_neighbors;
        self.enter_point = enter_point;
        self.started = true;
        self.level_factor = level_factor;
        self.max_connection = max_connection;
        self.max_connection0 = max_connection0;
        self.ef = ef;
        self.ef_construction = ef_construction;
        Ok(())
    }
}

/// A count read from a blob, which Go hands to `make`.
fn count(n: i64) -> Result<u64, Error> {
    u64::try_from(n).map_err(|_| Error::NegativeCount(n))
}

#[cfg(test)]
mod tests;
