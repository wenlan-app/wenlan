// SPDX-License-Identifier: Apache-2.0
//! Storage for the move to files (phase B of the storage migration).
//!
//! Nothing in the daemon reads or writes these yet. The export, rebuild and
//! dual-write steps build on them, and the database stays the truth until the
//! cutover.

pub mod format;
pub mod history;
