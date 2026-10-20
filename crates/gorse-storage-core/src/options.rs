// Ports: storage/options.go @ add21b5

//! The options every store's `Open` takes.

use std::fmt;

use gorse_common::gotime::Duration;

/// Go's `storage.Options`. Go's `int` is 64 bits on the platforms Gorse
/// ships for, so the counts are `i64`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Go's `IsolationLevel`: the MySQL transaction isolation level.
    pub isolation_level: String,
    /// Go's `MaxOpenConns`: the SQL pool's maximum open connections.
    pub max_open_conns: i64,
    /// Go's `MaxIdleConns`: the SQL pool's maximum idle connections.
    pub max_idle_conns: i64,
    /// Go's `ConnMaxLifetime`: the SQL pool's connection lifetime.
    pub conn_max_lifetime: Duration,
    /// Go's `MaxSearchResults`: the most results a search returns.
    pub max_search_results: i64,
    /// Go's `RedisClientName`: the name Redis connections report.
    pub redis_client_name: String,
    /// Go's `NumJobs`: the number of parallel jobs.
    pub num_jobs: i64,
    /// Go's `SkipUnindexedSegments`: whether vector searches skip segments
    /// that are not indexed yet.
    pub skip_unindexed_segments: bool,
}

/// Go's `storage.Option`: a function that sets one of the [`Options`].
pub struct Opt(Box<dyn Fn(&mut Options) + Send + Sync>);

impl Opt {
    fn new(f: impl Fn(&mut Options) + Send + Sync + 'static) -> Opt {
        Opt(Box::new(f))
    }

    /// Calls the function on o.
    pub fn apply(&self, o: &mut Options) {
        (self.0)(o)
    }
}

impl fmt::Debug for Opt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Opt(..)")
    }
}

/// Go's `WithNumJobs`.
pub fn with_num_jobs(num_jobs: i64) -> Opt {
    Opt::new(move |o| o.num_jobs = num_jobs)
}

/// Go's `WithSkipUnindexedSegments`.
pub fn with_skip_unindexed_segments(skip: bool) -> Opt {
    Opt::new(move |o| o.skip_unindexed_segments = skip)
}

/// Go's `WithIsolationLevel`.
pub fn with_isolation_level(isolation_level: impl Into<String>) -> Opt {
    let isolation_level = isolation_level.into();
    Opt::new(move |o| o.isolation_level.clone_from(&isolation_level))
}

/// Go's `WithMaxOpenConns`.
pub fn with_max_open_conns(max_open_conns: i64) -> Opt {
    Opt::new(move |o| o.max_open_conns = max_open_conns)
}

/// Go's `WithMaxIdleConns`.
pub fn with_max_idle_conns(max_idle_conns: i64) -> Opt {
    Opt::new(move |o| o.max_idle_conns = max_idle_conns)
}

/// Go's `WithConnMaxLifetime`.
pub fn with_conn_max_lifetime(conn_max_lifetime: Duration) -> Opt {
    Opt::new(move |o| o.conn_max_lifetime = conn_max_lifetime)
}

/// Go's `WithMaxSearchResults`.
pub fn with_max_search_results(limit: i64) -> Opt {
    Opt::new(move |o| o.max_search_results = limit)
}

/// Go's `WithRedisClientName`.
pub fn with_redis_client_name(client_name: impl Into<String>) -> Opt {
    let client_name = client_name.into();
    Opt::new(move |o| o.redis_client_name.clone_from(&client_name))
}

/// The pool settings [`apply_sql_pool`] makes: Go's `*sql.DB` setters.
pub trait SqlPool {
    /// Go's `SetMaxOpenConns`.
    fn set_max_open_conns(&mut self, n: i64);
    /// Go's `SetMaxIdleConns`.
    fn set_max_idle_conns(&mut self, n: i64);
    /// Go's `SetConnMaxLifetime`.
    fn set_conn_max_lifetime(&mut self, d: Duration);
}

/// Go's `ApplySQLPool`: sets each pool limit that is positive in opt and
/// leaves the pool's own default for the others.
pub fn apply_sql_pool(db: &mut impl SqlPool, opt: &Options) {
    if opt.max_open_conns > 0 {
        db.set_max_open_conns(opt.max_open_conns);
    }
    if opt.max_idle_conns > 0 {
        db.set_max_idle_conns(opt.max_idle_conns);
    }
    if opt.conn_max_lifetime > Duration(0) {
        db.set_conn_max_lifetime(opt.conn_max_lifetime);
    }
}

/// Go's `NewOptions`: isolation level "READ-UNCOMMITTED", at most 10000
/// search results and zero for the rest, then each option in order.
pub fn new_options<'a>(opts: impl IntoIterator<Item = &'a Opt>) -> Options {
    let mut opt = Options {
        isolation_level: "READ-UNCOMMITTED".to_owned(),
        max_open_conns: 0,
        max_idle_conns: 0,
        conn_max_lifetime: Duration(0),
        max_search_results: 10000,
        redis_client_name: String::new(),
        num_jobs: 0,
        skip_unindexed_segments: false,
    };
    for o in opts {
        o.apply(&mut opt);
    }
    opt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default, Debug, PartialEq)]
    struct Pool {
        max_open: Option<i64>,
        max_idle: Option<i64>,
        lifetime: Option<Duration>,
    }

    impl SqlPool for Pool {
        fn set_max_open_conns(&mut self, n: i64) {
            self.max_open = Some(n);
        }
        fn set_max_idle_conns(&mut self, n: i64) {
            self.max_idle = Some(n);
        }
        fn set_conn_max_lifetime(&mut self, d: Duration) {
            self.lifetime = Some(d);
        }
    }

    #[test]
    fn apply_sql_pool_sets_only_positive_limits() {
        let mut pool = Pool::default();
        apply_sql_pool(&mut pool, &new_options([]));
        assert_eq!(pool, Pool::default());

        let opts = [
            with_max_open_conns(1),
            with_max_idle_conns(-1),
            with_conn_max_lifetime(Duration(1)),
        ];
        apply_sql_pool(&mut pool, &new_options(&opts));
        assert_eq!(
            pool,
            Pool {
                max_open: Some(1),
                max_idle: None,
                lifetime: Some(Duration(1))
            }
        );

        let mut pool = Pool::default();
        let opts = [
            with_max_open_conns(0),
            with_max_idle_conns(2),
            with_conn_max_lifetime(Duration(0)),
        ];
        apply_sql_pool(&mut pool, &new_options(&opts));
        assert_eq!(
            pool,
            Pool {
                max_open: None,
                max_idle: Some(2),
                lifetime: None
            }
        );
    }
}
