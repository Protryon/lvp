use std::{path::PathBuf, collections::BTreeMap};

use anyhow::{Result, Context};
use k8s_openapi::api::core::v1::ConfigMap;
use kube::{Api, core::ObjectMeta, api::Patch};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::NAMESPACE;

use super::CLIENT;

#[derive(Clone, Serialize, Deserialize)]
pub struct Volume {
    pub name: String,
    pub size: u64,
    pub assigned_node_id: Option<String>,
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

pub enum VolumeCreation {
    AlreadyExists,
    Success,
}

impl Volume {
    fn key(&self) -> String {
        format!("lvp-vol-{}", self.name)
    }

    pub async fn create(&self) -> Result<VolumeCreation> {
        let serialized = serde_json::to_string(&self)?;
        let key = self.key();
        let configs: Api<ConfigMap> = Api::default_namespaced(CLIENT.clone());
        if configs.get_opt(&key).await?.is_some() {
            return Ok(VolumeCreation::AlreadyExists);
        };
        let mut data = BTreeMap::new();
        data.insert("data.json".to_string(), serialized);
        configs.create(&Default::default(), &ConfigMap {
            data: Some(data),
            metadata: ObjectMeta {
                name: Some(key),
                namespace: Some(NAMESPACE.clone()),
                ..Default::default()
            },
            ..Default::default()
        }).await?;
        Ok(VolumeCreation::Success)
    }

    pub async fn update(&self) -> Result<()> {
        let serialized = serde_json::to_string(&self)?;
        let key = self.key();
        let configs: Api<ConfigMap> = Api::default_namespaced(CLIENT.clone());
        configs.patch(&key, &Default::default(), &Patch::Merge(json!({
            "data": {
                "data.json": serialized,
            },
        }))).await?;
        Ok(())
    }

    pub async fn delete(&self) -> Result<()> {
        let configs: Api<ConfigMap> = Api::default_namespaced(CLIENT.clone());
        configs.delete(&self.key(), &Default::default()).await?;
        Ok(())
    }

    pub async fn load(name: &str) -> Result<Option<Self>> {
        let configs: Api<ConfigMap> = Api::default_namespaced(CLIENT.clone());
        let Some(config) = configs.get_opt(&format!("lvp-vol-{name}")).await? else {
            return Ok(None);
        };
        Ok(serde_json::from_str(config.data.context("no data")?.get("data.json").context("missing data.json")?)?)
    }

    pub async fn list() -> Result<Vec<Self>> {
        let configs: Api<ConfigMap> = Api::default_namespaced(CLIENT.clone());
        let mut out = vec![];
        for config in configs.list(&Default::default()).await? {
            if !config.metadata.name.as_deref().unwrap_or_default().starts_with("lvp-vol-") {
                continue;
            }
            out.push(serde_json::from_str(config.data.context("no data")?.get("data.json").context("missing data.json")?)?);
        }
        Ok(out)
    }
}
