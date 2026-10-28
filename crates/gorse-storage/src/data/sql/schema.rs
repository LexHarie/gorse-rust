// Ports: storage/data/sql.go (SQLDatabase.Init, the tables GORM migrates) @ add21b5

//! The tables and indexes `Init()` creates, per dialect.
//!
//! GORM's `AutoMigrate` writes these from the struct tags. Here each statement
//! is written out, and it must produce the same schema: the dumps in
//! `contracts/schema/data/` are what the Go implementation's database holds,
//! and a Rust-created database must dump to the same text. The SQLite text is
//! GORM's own, backticks and all, because SQLite keeps a table's `CREATE` text
//! as it is written.
//!
//! The index names are not prefixed, as Go's are not. Only tables are.

use super::query::Driver;

/// The statements `Init()` runs on `driver` for a store with `prefix`, in
/// order: each table, then its indexes. Every statement is idempotent, so a
/// second `Init()` changes nothing.
///
/// Deviation (X-WP22-1): GORM's `AutoMigrate` also adds columns and indexes
/// that an older table lacks. These statements only create what is missing; a
/// table that exists is left as it is.
pub fn init_statements(driver: Driver, prefix: &str) -> Vec<String> {
    match driver {
        Driver::MySql => mysql(prefix),
        Driver::Postgres => postgres(prefix),
        Driver::Sqlite => sqlite(prefix),
    }
}

fn mysql(prefix: &str) -> Vec<String> {
    vec![
        format!(
            "CREATE TABLE IF NOT EXISTS `{prefix}users` (\n\
            \x20 `update_at` datetime(6) NOT NULL DEFAULT '1970-01-01 00:00:00',\n\
            \x20 `user_id` varchar(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,\n\
            \x20 `labels` json NOT NULL,\n\
            \x20 `comment` text NOT NULL,\n\
            \x20 PRIMARY KEY (`user_id`)\n\
            ) ENGINE=InnoDB"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS `{prefix}items` (\n\
            \x20 `update_at` datetime(6) NOT NULL DEFAULT '1970-01-01 00:00:00',\n\
            \x20 `item_id` varchar(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,\n\
            \x20 `is_hidden` bool NOT NULL,\n\
            \x20 `categories` json NOT NULL,\n\
            \x20 `time_stamp` datetime NOT NULL,\n\
            \x20 `labels` json NOT NULL,\n\
            \x20 `comment` text NOT NULL,\n\
            \x20 PRIMARY KEY (`item_id`),\n\
            \x20 KEY `time_stamp_index` (`time_stamp`)\n\
            ) ENGINE=InnoDB"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS `{prefix}feedback` (\n\
            \x20 `feedback_type` varchar(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,\n\
            \x20 `user_id` varchar(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,\n\
            \x20 `item_id` varchar(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,\n\
            \x20 `value` double NOT NULL DEFAULT 0,\n\
            \x20 `time_stamp` datetime NOT NULL,\n\
            \x20 `updated` datetime NOT NULL DEFAULT '2000-01-01 00:00:00',\n\
            \x20 `labels` json,\n\
            \x20 `comment` text NOT NULL,\n\
            \x20 PRIMARY KEY (`feedback_type`,`user_id`,`item_id`),\n\
            \x20 KEY `user_id` (`user_id`),\n\
            \x20 KEY `item_id` (`item_id`)\n\
            ) ENGINE=InnoDB"
        ),
    ]
}

fn postgres(prefix: &str) -> Vec<String> {
    vec![
        format!(
            "CREATE TABLE IF NOT EXISTS {prefix}users (\n\
            \x20   update_at timestamptz NOT NULL DEFAULT '1970-01-01 00:00:00+00',\n\
            \x20   user_id varchar(256) COLLATE \"C\" NOT NULL,\n\
            \x20   labels json NOT NULL DEFAULT '[]',\n\
            \x20   comment text NOT NULL DEFAULT '',\n\
            \x20   PRIMARY KEY (user_id)\n\
            )"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS {prefix}items (\n\
            \x20   update_at timestamptz NOT NULL DEFAULT '1970-01-01 00:00:00+00',\n\
            \x20   item_id varchar(256) COLLATE \"C\" NOT NULL,\n\
            \x20   is_hidden bool NOT NULL DEFAULT false,\n\
            \x20   categories json NOT NULL DEFAULT '[]',\n\
            \x20   time_stamp timestamptz NOT NULL,\n\
            \x20   labels json NOT NULL DEFAULT '[]',\n\
            \x20   comment text NOT NULL DEFAULT '',\n\
            \x20   PRIMARY KEY (item_id)\n\
            )"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS {prefix}feedback (\n\
            \x20   feedback_type varchar(256) COLLATE \"C\" NOT NULL,\n\
            \x20   user_id varchar(256) COLLATE \"C\" NOT NULL,\n\
            \x20   item_id varchar(256) COLLATE \"C\" NOT NULL,\n\
            \x20   value float8 NOT NULL DEFAULT 0,\n\
            \x20   time_stamp timestamptz NOT NULL,\n\
            \x20   updated timestamptz NOT NULL DEFAULT '2000-01-01 00:00:00+00',\n\
            \x20   labels json,\n\
            \x20   comment text NOT NULL DEFAULT '',\n\
            \x20   PRIMARY KEY (feedback_type, user_id, item_id)\n\
            )"
        ),
        format!("CREATE INDEX IF NOT EXISTS time_stamp_index ON {prefix}items (time_stamp)"),
        format!("CREATE INDEX IF NOT EXISTS user_id_index ON {prefix}feedback (user_id)"),
        format!("CREATE INDEX IF NOT EXISTS item_id_index ON {prefix}feedback (item_id)"),
    ]
}

fn sqlite(prefix: &str) -> Vec<String> {
    vec![
        format!(
            "CREATE TABLE IF NOT EXISTS `{prefix}users` (`update_at` datetime NOT NULL DEFAULT '1970-01-01 00:00:00',`user_id` varchar(256) not null,`labels` json NOT NULL DEFAULT \"null\",`comment` text NOT NULL DEFAULT \"\",PRIMARY KEY (`user_id`))"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS `{prefix}items` (`update_at` datetime NOT NULL DEFAULT '1970-01-01 00:00:00',`item_id` varchar(256) NOT NULL,`is_hidden` numeric NOT NULL DEFAULT false,`categories` json NOT NULL DEFAULT \"[]\",`time_stamp` datetime NOT NULL DEFAULT \"0001-01-01\",`labels` json NOT NULL DEFAULT \"[]\",`comment` text NOT NULL DEFAULT \"\",PRIMARY KEY (`item_id`))"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS `{prefix}feedback` (`feedback_type` varchar(256) NOT NULL,`user_id` varchar(256) NOT NULL,`item_id` varchar(256) NOT NULL,`value` real NOT NULL DEFAULT 0,`time_stamp` datetime NOT NULL DEFAULT \"0001-01-01\",`updated` datetime NOT NULL DEFAULT \"0001-01-01\",`labels` json,`comment` text NOT NULL DEFAULT \"\",PRIMARY KEY (`feedback_type`,`user_id`,`item_id`))"
        ),
        format!("CREATE INDEX IF NOT EXISTS `time_stamp_index` ON `{prefix}items`(`time_stamp`)"),
        format!("CREATE INDEX IF NOT EXISTS `user_id_index` ON `{prefix}feedback`(`user_id`)"),
        format!("CREATE INDEX IF NOT EXISTS `item_id_index` ON `{prefix}feedback`(`item_id`)"),
    ]
}
