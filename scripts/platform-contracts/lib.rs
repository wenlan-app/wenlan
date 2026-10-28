// SPDX-License-Identifier: AGPL-3.0-only
// Compile and exercise the modules the product uses, rather than replicas.
#![allow(dead_code)]
extern crate self as wenlan_types;
#[path = "../../app/src/config.rs"]
mod config;
#[path = "../../app/src/error.rs"]
mod error;
#[path = "../../crates/wenlan-core/src/export/filename.rs"]
mod filename;
#[path = "../../app/src/identity_paths.rs"]
mod identity_paths;
#[path = "../../app/src/sources/obsidian.rs"]
mod obsidian;
#[path = "../../crates/wenlan-cli/src/commands/open_document.rs"]
mod open_document;
#[path = "../../app/src/remote_access_platform.rs"]
#[allow(unused_imports)]
mod remote_access_platform;
#[path = "../../crates/wenlan-cli/src/commands/secret_input.rs"]
mod secret_input;
#[path = "../../crates/wenlan-cli/src/commands/service_binding.rs"]
mod service_binding;
#[path = "../../app/src/source_paths.rs"]
mod source_paths;
#[path = "../../crates/wenlan-types/src/sources.rs"]
pub mod sources;
#[path = "../../crates/wenlan-server/src/main/startup_paths.rs"]
mod startup_paths;
#[cfg(test)]
#[path = "../../app/src/test_env.rs"]
mod test_env;
#[path = "../../crates/wenlan-cli/src/commands/windows_task.rs"]
mod windows_task;
