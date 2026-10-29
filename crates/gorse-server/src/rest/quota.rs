// Ports: server/rest.go (checkLabelsSize, checkCategoriesSize, checkCommentSize,
// checkItemSize, checkUserSize, checkUserCountLimit, checkItemCountLimit) @
// add21b5

//! The quota checks the write routes make before they store anything.
//!
//! Each check returns the message Go's `errors.Errorf` builds, which the
//! handler writes with `TooManyRequests`. A check that cannot read the store's
//! count logs a warning and lets the write through, as Go does.

use gorse_common::gocontext::Context;
use gorse_common::jsonutil;
use gorse_config::config::schema::Config;
use gorse_storage::data::types::{Item, Labels, User};

use super::RestServer;
use super::response::ApiError;

/// Go's `checkLabelsSize`: the labels, as JSON, must fit `quota.max_labels_size`.
/// Nil labels and a limit of zero or less are not checked.
pub(crate) fn check_labels_size(config: &Config, labels: &Labels) -> Result<(), String> {
    let limit = config.quota.max_labels_size;
    if limit <= 0 || *labels == Labels::Nil {
        return Ok(());
    }
    let buf = jsonutil::marshal(labels).map_err(|err| err.to_string())?;
    if buf.len() as i64 > limit {
        return Err(format!(
            "labels size exceeds limit ({} > {limit} bytes)",
            buf.len()
        ));
    }
    Ok(())
}

/// Go's `checkCategoriesSize`: the count and the length of each category must
/// fit the quota. A limit of zero or less is not checked.
pub(crate) fn check_categories_size(config: &Config, categories: &[String]) -> Result<(), String> {
    let count_limit = config.quota.max_categories_count;
    if count_limit > 0 && categories.len() as i64 > count_limit {
        return Err(format!(
            "item categories count exceeds limit ({} > {count_limit})",
            categories.len()
        ));
    }
    let size_limit = config.quota.max_categories_size;
    if size_limit <= 0 {
        return Ok(());
    }
    for category in categories {
        if category.len() as i64 > size_limit {
            return Err(format!(
                "category size exceeds limit ({} > {size_limit} bytes)",
                category.len()
            ));
        }
    }
    Ok(())
}

/// Go's `checkCommentSize`: the comment's byte length must fit
/// `quota.max_comment_size`. A limit of zero or less is not checked.
pub(crate) fn check_comment_size(config: &Config, comment: &str) -> Result<(), String> {
    let limit = config.quota.max_comment_size;
    if limit > 0 && comment.len() as i64 > limit {
        return Err(format!(
            "comment size exceeds limit ({} > {limit} bytes)",
            comment.len()
        ));
    }
    Ok(())
}

/// Go's `checkItemSize`: the labels, comment and categories of an item.
pub(crate) fn check_item_size(config: &Config, item: &Item) -> Result<(), String> {
    check_labels_size(config, &item.labels)?;
    check_comment_size(config, &item.comment)?;
    check_categories_size(config, item.categories.as_deref().unwrap_or(&[]))
}

/// Go's `checkUserSize`: the labels and comment of a user.
pub(crate) fn check_user_size(config: &Config, user: &User) -> Result<(), String> {
    check_labels_size(config, &user.labels)?;
    check_comment_size(config, &user.comment)
}

/// Go's `checkUserCountLimit`: the users already stored plus `user_ids` must fit
/// `quota.max_users_count`. A count that cannot be read is logged and allowed.
pub(crate) async fn check_user_count_limit(
    rest: &RestServer,
    user_ids: &[String],
) -> Result<(), ApiError> {
    let limit = rest.config().quota.max_users_count;
    if limit <= 0 {
        return Ok(());
    }
    let total = match rest.data().count_users(&Context::background()).await {
        Ok(total) => total,
        Err(err) => {
            tracing::warn!(error = %err, "failed to check user count limit");
            return Ok(());
        }
    };
    let count = total + user_ids.len() as i64;
    if count > limit {
        return Err(ApiError::TooManyRequests(format!(
            "users count exceeds limit ({count} > {limit})"
        )));
    }
    Ok(())
}

/// Go's `checkItemCountLimit`: the items already stored plus `item_ids` must
/// fit `quota.max_items_count`. A count that cannot be read is logged and
/// allowed.
pub(crate) async fn check_item_count_limit(
    rest: &RestServer,
    item_ids: &[String],
) -> Result<(), ApiError> {
    let limit = rest.config().quota.max_items_count;
    if limit <= 0 {
        return Ok(());
    }
    let total = match rest.data().count_items(&Context::background()).await {
        Ok(total) => total,
        Err(err) => {
            tracing::warn!(error = %err, "failed to check item count limit");
            return Ok(());
        }
    };
    let count = total + item_ids.len() as i64;
    if count > limit {
        return Err(ApiError::TooManyRequests(format!(
            "items count exceeds limit ({count} > {limit})"
        )));
    }
    Ok(())
}
