use std::path::PathBuf;

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
    pub static ref NODE: String = {
        let env_var = std::env::var("NODE").unwrap_or_default();
        if env_var.is_empty() {
            std::fs::read_to_string("/etc/hostname").ok().unwrap_or_else(|| "unknown".to_string())
        } else {
            env_var
        }
    };
    pub static ref NAMESPACE: String = {
        std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace").expect("failed to read K8S namespace, is the service account linked?").trim().to_string()
    };
}

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub socket_path: PathBuf,
    pub database: PathBuf,
    pub host_prefix: PathBuf,
}
