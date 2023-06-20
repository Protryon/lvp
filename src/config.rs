use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

lazy_static::lazy_static! {
    static ref CONFIG_PATH: String = {
        let base = std::env::var("LVP_CONFIG").unwrap_or_default();
        if base.is_empty() {
            "./config.yaml".to_string()
        } else {
            base
        }
    };
    pub static ref CONFIG: Config = serde_yaml::from_str(&std::fs::read_to_string(&*CONFIG_PATH).expect("failed to read config")).expect("failed to parse config");
}

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub socket_path: PathBuf,
    pub node_id: String,
    pub database: PathBuf,
    pub host_prefix: PathBuf,
    #[serde(default)]
    pub topology: HashMap<String, String>,
}
