// Ports: storage/scheme.go (the json_contains function registered on each SQLite connection) @ add21b5

//! Registers `json_contains` on each SQLite connection.
//!
//! sqlx-sqlite has no scalar function API, so this calls SQLite's
//! `sqlite3_create_function_v2` on the connection's handle through
//! `libsqlite3-sys`. The argument and error rules are the ones
//! [`gorse_storage_core::json_contains`] states.

#![allow(
    unsafe_code,
    reason = "a scalar function is registered through the SQLite C API, which sqlx does not wrap"
)]

use std::ffi::{c_int, c_void};
use std::ptr;
use std::slice;

use gorse_storage_core::json_contains::{Arg, json_contains, modernc_error_text, modernc_text};
use libsqlite3_sys as ffi;
use sqlx::sqlite::SqliteConnection;

/// Registers `json_contains(a, b)` on `conn`, so the statements of the store
/// can call it.
pub(super) async fn register_json_contains(conn: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let mut locked = conn.lock_handle().await?;
    let handle = locked.as_raw_handle();
    // SAFETY: `handle` is the open database handle of `conn`, which the lock
    // keeps any other call from using while the function is registered. The
    // name is a static C string, no application data is passed, and the
    // callback is `json_contains_fn`, an `extern "C"` function that does not
    // unwind.
    let rc = unsafe {
        ffi::sqlite3_create_function_v2(
            handle.as_ptr(),
            c"json_contains".as_ptr(),
            2,
            ffi::SQLITE_UTF8,
            ptr::null_mut(),
            Some(json_contains_fn),
            None,
            None,
            None,
        )
    };
    if rc == ffi::SQLITE_OK {
        Ok(())
    } else {
        Err(sqlx::Error::Protocol(format!(
            "sqlite3_create_function_v2 returned {rc}"
        )))
    }
}

/// The scalar function SQLite calls for `json_contains(a, b)`.
///
/// # Safety
///
/// SQLite calls this with `argc` argument values in `argv` and a `context`
/// that are valid for the duration of the call, as the registration in
/// [`register_json_contains`] makes it.
unsafe extern "C" fn json_contains_fn(
    context: *mut ffi::sqlite3_context,
    argc: c_int,
    argv: *mut *mut ffi::sqlite3_value,
) {
    // SAFETY: SQLite passes `argc` values in `argv`, valid for this call (see
    // the function's safety notes). Registration allows exactly two.
    let args = unsafe { slice::from_raw_parts(argv, usize::try_from(argc).unwrap_or(0)) };
    if args.len() != 2 {
        return;
    }
    // SAFETY: each value is one SQLite passed to this call, valid until the
    // call returns, and the `Arg`s built from them are used only before then.
    let (a, b) = unsafe { (arg(args[0]), arg(args[1])) };
    // SAFETY: `context` is the context SQLite passed to this call, and the
    // result is set on it once, before the call returns.
    unsafe {
        match json_contains(a, b) {
            Ok(None) => ffi::sqlite3_result_null(context),
            Ok(Some(found)) => ffi::sqlite3_result_int(context, c_int::from(found)),
            Err(err) => {
                let message = modernc_error_text(&err);
                // SQLite copies the message, so it needs to be valid only for
                // this call.
                ffi::sqlite3_result_error(
                    context,
                    message.as_ptr().cast(),
                    c_int::try_from(message.len()).unwrap_or(c_int::MAX),
                );
            }
        }
    }
}

/// The argument `value` holds, typed as SQLite types it.
///
/// # Safety
///
/// `value` is a value SQLite passed to the running scalar function, and the
/// returned argument borrows bytes that are valid only until that function
/// returns.
unsafe fn arg<'a>(value: *mut ffi::sqlite3_value) -> Arg<'a> {
    // SAFETY: `value` is a live argument of the running function (see the
    // safety notes above). Each value is read in the order SQLite requires:
    // text before its length, and the length before a blob's bytes.
    unsafe {
        match ffi::sqlite3_value_type(value) {
            ffi::SQLITE_INTEGER => Arg::Integer(ffi::sqlite3_value_int64(value)),
            ffi::SQLITE_FLOAT => Arg::Real(ffi::sqlite3_value_double(value)),
            ffi::SQLITE_TEXT => {
                let text = ffi::sqlite3_value_text(value);
                let len = ffi::sqlite3_value_bytes(value);
                Arg::Text(modernc_text(bytes_at(text.cast(), len)))
            }
            ffi::SQLITE_BLOB => {
                let blob = ffi::sqlite3_value_blob(value);
                let len = ffi::sqlite3_value_bytes(value);
                Arg::Blob(bytes_at(blob, len))
            }
            _ => Arg::Null,
        }
    }
}

/// The `len` bytes at `data`, or none when `data` is null, which SQLite
/// returns for an empty value.
///
/// # Safety
///
/// `data` is null or points to `len` bytes that stay valid for `'a`.
unsafe fn bytes_at<'a>(data: *const c_void, len: c_int) -> &'a [u8] {
    if data.is_null() {
        return &[];
    }
    // SAFETY: `data` is non-null and, per the caller, points to `len` bytes.
    unsafe { slice::from_raw_parts(data.cast::<u8>(), usize::try_from(len).unwrap_or(0)) }
}
