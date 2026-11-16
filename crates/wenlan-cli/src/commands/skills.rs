// SPDX-License-Identifier: Apache-2.0
//! Read approved Space rules from the local Wenlan library.

use anyhow::Result;

use crate::client::WenlanClient;

pub async fn read_space_rules(client: &WenlanClient, quiet: bool) -> Result<()> {
    let response = client.read_space_rules().await?;
    if response.lines.is_empty() {
        if !quiet {
            println!("No approved working rules for {}.", response.space_name);
        }
        return Ok(());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&response).expect("Space rules response serializes")
    );
    Ok(())
}
