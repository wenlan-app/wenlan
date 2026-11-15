// SPDX-License-Identifier: Apache-2.0
//! Open the personal daemon's embedded browser UI.

use crate::client::WenlanClient;
use anyhow::Context;

pub fn browser_url(base_url: &str) -> String {
    format!("{}/", base_url.trim_end_matches('/'))
}

pub fn run(client: &WenlanClient) -> anyhow::Result<()> {
    let url = browser_url(client.base_url());
    println!("Wenlan in your browser: {url}");
    open::that_detached(&url).with_context(|| format!("open {url} in the default browser"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn browser_address_has_one_trailing_slash() {
        assert_eq!(
            super::browser_url("http://127.0.0.1:7878"),
            "http://127.0.0.1:7878/"
        );
        assert_eq!(
            super::browser_url("http://127.0.0.1:7878/"),
            "http://127.0.0.1:7878/"
        );
    }
}
