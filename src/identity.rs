use tonic::{Request, Response, Status};

use crate::proto::{
    identity_server::Identity, plugin_capability::service::Type as ServiceType,
    plugin_capability::volume_expansion::Type as VolumeExpansionType, plugin_capability::Service,
    plugin_capability::Type, plugin_capability::VolumeExpansion, GetPluginCapabilitiesRequest,
    GetPluginCapabilitiesResponse, GetPluginInfoRequest, GetPluginInfoResponse, PluginCapability,
    ProbeRequest, ProbeResponse,
};

#[derive(Debug)]
pub struct IdentityService {}

#[async_trait::async_trait]
impl Identity for IdentityService {
    async fn get_plugin_info(
        &self,
        _request: Request<GetPluginInfoRequest>,
    ) -> Result<Response<GetPluginInfoResponse>, Status> {
        Ok(Response::new(GetPluginInfoResponse {
            name: "lvp".to_string(),
            vendor_version: env!("CARGO_PKG_VERSION").to_string(),
            manifest: Default::default(),
        }))
    }

    async fn get_plugin_capabilities(
        &self,
        _request: Request<GetPluginCapabilitiesRequest>,
    ) -> Result<Response<GetPluginCapabilitiesResponse>, Status> {
        Ok(Response::new(GetPluginCapabilitiesResponse {
            capabilities: vec![
                PluginCapability {
                    r#type: Some(Type::Service(Service {
                        r#type: ServiceType::ControllerService as i32,
                    })),
                },
                PluginCapability {
                    r#type: Some(Type::Service(Service {
                        r#type: ServiceType::VolumeAccessibilityConstraints as i32,
                    })),
                },
                PluginCapability {
                    r#type: Some(Type::VolumeExpansion(VolumeExpansion {
                        r#type: VolumeExpansionType::Online as i32,
                    })),
                },
            ],
        }))
    }

    async fn probe(
        &self,
        _request: Request<ProbeRequest>,
    ) -> Result<Response<ProbeResponse>, Status> {
        Ok(Response::new(ProbeResponse { ready: Some(true) }))
    }
}
