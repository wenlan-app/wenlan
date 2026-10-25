// SPDX-License-Identifier: Apache-2.0
//! Wakes the background worker that embeds memory rows stored without a
//! vector. A big vault import stores its notes first (keyword-searchable at
//! once) and calls [`request`]; the worker started at daemon boot embeds them
//! in batches and then waits for the next request.

use std::sync::LazyLock;

use tokio::sync::Notify;

static PENDING: LazyLock<Notify> = LazyLock::new(Notify::new);

/// Ask the worker to embed rows that have no vector. A request made while
/// the worker is busy is kept, so it looks again after its current run.
pub fn request() {
    PENDING.notify_one();
}

/// Wait for the next [`request`].
pub async fn requested() {
    PENDING.notified().await;
}
