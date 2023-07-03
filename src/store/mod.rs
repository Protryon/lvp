mod volume;

use always_cell::AlwaysCell;
use kube::Client;
pub use volume::*;
use anyhow::Result;

pub static CLIENT: AlwaysCell<Client> = AlwaysCell::new(); 

pub async fn init_client() -> Result<()> {
    let client = Client::try_default().await?;

    AlwaysCell::set(&CLIENT, client);
    Ok(())
}