// SPDX-License-Identifier: Apache-2.0
//! Storage for the move to files (phase B of the storage migration).
//!
//! The daemon reads none of these. It writes only the file copy in
//! [`mirror`] (step 3), from a snapshot of the database. The database stays
//! the truth until the cutover.

pub mod export;
pub mod format;
pub mod history;
pub mod mirror;
pub mod rebuild;
