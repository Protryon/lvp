use std::path::PathBuf;

use anyhow::Result;
use redb::{ReadableTable, TableDefinition, TableError};
use serde::{Deserialize, Serialize};

use super::DATABASE;

#[derive(Clone, Serialize, Deserialize)]
pub struct Volume {
    pub name: String,
    pub size: u64,
    pub assigned_node_id: String,
    pub state: VolumeState,
    pub published_readonly: bool,
    pub published_config: Option<VolumeConfig>,
    pub filesystem: Filesystem,
    pub valid_configs: Vec<VolumeConfig>,
    pub loop_device: Option<PathBuf>,
    pub mount_paths: Vec<PathBuf>,
    pub host_path: String,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VolumeConfig {
    pub mode: VolumeMode,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Filesystem {
    #[default]
    Ext4,
    Xfs,
    Bind,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VolumeState {
    #[default]
    Open,
    ControllerPublished,
    NodePublished,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VolumeMode {
    SingleNodeWriter,
    SingleNodeReader,
    SingleNodeSingleWriter,
    SingleNodeMultiWriter,
}

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("volumes");

pub enum VolumeCreation {
    AlreadyExists,
    Success,
}

impl Volume {
    pub async fn create(&self) -> Result<VolumeCreation> {
        let serialized = serde_json::to_string(&self)?;
        let name = self.name.clone();
        Ok(
            tokio::task::spawn_blocking(move || -> Result<VolumeCreation> {
                let txn = DATABASE.begin_write()?;
                {
                    let mut table = txn.open_table(TABLE)?;
                    if table.get(&*name)?.is_some() {
                        return Ok(VolumeCreation::AlreadyExists);
                    }
                    table.insert(&*name, &*serialized)?;
                }
                txn.commit()?;
                Ok(VolumeCreation::Success)
            })
            .await??,
        )
    }

    pub async fn update(&self) -> Result<()> {
        let serialized = serde_json::to_string(&self)?;
        let name = self.name.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let txn = DATABASE.begin_write()?;
            {
                let mut table = txn.open_table(TABLE)?;
                table.insert(&*name, &*serialized)?;
            }
            txn.commit()?;
            Ok(())
        })
        .await??;
        Ok(())
    }

    pub async fn delete(&self) -> Result<()> {
        let name = self.name.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            //TODO: validate not in use?
            let txn = DATABASE.begin_write()?;
            {
                let mut table = txn.open_table(TABLE)?;
                table.remove(&*name)?;
            }
            txn.commit()?;
            Ok(())
        })
        .await??;
        Ok(())
    }

    pub async fn load(name: String) -> Result<Option<Self>> {
        tokio::task::spawn_blocking(move || -> Result<Option<Self>> {
            let txn = DATABASE.begin_read()?;
            let table = match txn.open_table(TABLE) {
                Ok(x) => x,
                Err(TableError::TableDoesNotExist(_)) => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            let Some(raw) = table.get(&*name)? else {
                return Ok(None);
            };
            Ok(Some(serde_json::from_str(raw.value())?))
        })
        .await?
    }

    pub async fn list() -> Result<Vec<Self>> {
        tokio::task::spawn_blocking(move || -> Result<Vec<Self>> {
            let txn = DATABASE.begin_read()?;
            let table = match txn.open_table(TABLE) {
                Ok(x) => x,
                Err(TableError::TableDoesNotExist(_)) => return Ok(vec![]),
                Err(e) => return Err(e.into()),
            };
            let mut out = vec![];
            for range in table.iter()? {
                let (_, value) = range?;
                out.push(serde_json::from_str(value.value())?);
            }
            Ok(out)
        })
        .await?
    }
}
