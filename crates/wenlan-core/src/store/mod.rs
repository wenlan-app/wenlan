// SPDX-License-Identifier: Apache-2.0
//! Storage for the move to files (phase B of the storage migration).
//!
//! Every write to what the files hold reaches [`write_log`] before the
//! database commits it; the daemon brings the file copy in [`mirror`] up to
//! date from a snapshot of the database, and records how far into the log
//! it goes. The files plus the log are the truth and the database is an
//! index of them: [`restore`] builds a missing database again from both.

pub mod export;
pub mod format;
pub mod history;
pub mod mirror;
pub mod rebuild;
pub mod restore;
pub mod write_log;
