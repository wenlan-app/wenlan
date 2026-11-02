// SPDX-License-Identifier: Apache-2.0
//! Storage for the move to files (phase B of the storage migration).
//!
//! The daemon writes the file copy in [`mirror`] (step 3), from a snapshot
//! of the database, and reads it back only in [`restore`]: when the
//! database file is gone at start. The database stays the truth until the
//! cutover.

pub mod export;
pub mod format;
pub mod history;
pub mod mirror;
pub mod rebuild;
pub mod restore;
