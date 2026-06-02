// SPDX-License-Identifier: MPL-2.0
//! `selur-compose restart` command implementation

use anyhow::Result;

use crate::compose::ComposeFile;
use crate::commands::{start, stop};

/// Run `restart` command - restart services
pub async fn restart(
    compose: &ComposeFile,
    project_name: &str,
    services: Vec<String>,
    timeout: u64,
) -> Result<()> {
    tracing::info!("Restarting services for project '{}'", project_name);

    println!("Stopping services...");
    stop(compose, project_name, services.clone(), timeout).await?;

    println!();
    println!("Starting services...");
    start(compose, project_name, services).await?;

    println!();
    println!("✓ Services restarted");

    Ok(())
}
