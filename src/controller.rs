use std::{io::ErrorKind, os::fd::AsRawFd, path::Path};

use log::{error, info};
use tokio::{fs::File, process::Command};
use tonic::{Request, Response, Status};

use crate::{
    config::CONFIG,
    proto::{
        controller_server::Controller,
        controller_service_capability::rpc::Type as RpcType,
        controller_service_capability::Rpc,
        controller_service_capability::Type as CapabilityType,
        list_volumes_response::VolumeStatus,
        volume_capability::{access_mode::Mode, *},
        *,
    },
    store::{self, Filesystem, VolumeConfig, VolumeCreation, VolumeMode, VolumeState},
};

#[derive(Debug)]
pub struct ControllerService {}

fn parse_filesystem(from: &str) -> Result<Filesystem, Status> {
    match from {
        "ext4" => Ok(Filesystem::Ext4),
        "xfs" => Ok(Filesystem::Xfs),
        "bind" => Ok(Filesystem::Bind),
        _ => {
            return Err(Status::invalid_argument(
                "unknown fs_type, only 'ext4', 'xfs', or 'bind' allowed",
            ))
        }
    }
}

pub fn parse_volume_capability(
    capability: &VolumeCapability,
) -> Result<(VolumeConfig, Option<Filesystem>), Status> {
    let Some(mode) = &capability.access_mode else {
        return Err(Status::invalid_argument("missing access_mode"));
    };
    let Some(type_) = &capability.access_type else {
        return Err(Status::invalid_argument("missing access_type"));
    };
    let AccessType::Mount(type_) = &type_ else {
        return Err(Status::invalid_argument("only mount-type access_types allowed"));
    };
    let filesystem = if type_.fs_type.is_empty() {
        None
    } else {
        Some(parse_filesystem(&*type_.fs_type)?)
    };
    if !type_.mount_flags.is_empty() {
        return Err(Status::invalid_argument("mount_flags not supported"));
    }

    let mode = mode.mode();
    Ok((
        VolumeConfig {
            mode: match mode {
                Mode::SingleNodeWriter => VolumeMode::SingleNodeWriter,
                Mode::SingleNodeReaderOnly => VolumeMode::SingleNodeReader,
                Mode::SingleNodeSingleWriter => VolumeMode::SingleNodeSingleWriter,
                Mode::SingleNodeMultiWriter => VolumeMode::SingleNodeMultiWriter,
                _ => return Err(Status::invalid_argument("unsupported volume mode")),
            },
        },
        filesystem,
    ))
}

async fn make_volume(path: &Path, size: u64, filesystem: Filesystem) -> std::io::Result<()> {
    if tokio::fs::try_exists(path).await? {
        return Err(std::io::Error::new(
            ErrorKind::AlreadyExists,
            "volume file already exists",
        ));
    }
    if filesystem == Filesystem::Bind {
        tokio::fs::create_dir_all(path).await?;
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let file = File::create(path).await?;
    tokio::task::spawn_blocking(move || {
        if unsafe { libc::ftruncate(file.as_raw_fd(), size as i64) } < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
    .await??;
    match filesystem {
        Filesystem::Ext4 => {
            let status = Command::new("mkfs.ext4").arg(path).spawn()?.wait().await?;
            if !status.success() {
                return Err(std::io::Error::new(
                    ErrorKind::Other,
                    &*format!("mkfs.ext4 exited with status {status}"),
                ));
            }
        }
        Filesystem::Xfs => {
            let status = Command::new("mkfs.xfs").arg(path).spawn()?.wait().await?;
            if !status.success() {
                return Err(std::io::Error::new(
                    ErrorKind::Other,
                    &*format!("mkfs.xfs exited with status {status}"),
                ));
            }
        }
        Filesystem::Bind => unreachable!(),
    }

    Ok(())
}

#[async_trait::async_trait]
impl Controller for ControllerService {
    async fn create_volume(
        &self,
        request: Request<CreateVolumeRequest>,
    ) -> Result<Response<CreateVolumeResponse>, Status> {
        let request = request.into_inner();

        if request.name.is_empty() {
            return Err(Status::invalid_argument("missing name"));
        }
        if let Some(requirements) = &request.accessibility_requirements {
            for requirement in &requirements.requisite {
                for (key, value) in &requirement.segments {
                    if key != "node" || value != &CONFIG.node_id {
                        return Err(Status::resource_exhausted(
                            "invalid accessibility_requirements, only allowed node=<node id>",
                        ));
                    }
                }
            }
        }

        if request.volume_capabilities.is_empty() {
            return Err(Status::invalid_argument("no capabilities specified"));
        }

        let mut host_base_path = None::<String>;
        let mut filesystem = None::<Filesystem>;
        for (name, value) in &request.parameters {
            match &**name {
                "host_base_path" => host_base_path = Some(value.clone()),
                "fs_type" => filesystem = Some(parse_filesystem(value)?),
                _ => {
                    return Err(Status::invalid_argument(format!(
                        "unknown parameter {name}"
                    )))
                }
            }
        }
        let Some(host_base_path) = host_base_path else {
            return Err(Status::invalid_argument(format!("missing host_base_path")));
        };

        let mut valid_configs = vec![];
        for capability in &request.volume_capabilities {
            let (config, fs) = parse_volume_capability(capability)?;
            valid_configs.push(config);
            if filesystem.is_none() && fs.is_some() {
                filesystem = fs;
            } else if filesystem.is_some() && fs.is_some() && filesystem != fs {
                return Err(Status::invalid_argument(
                    "conflicting filesystems specified",
                ));
            }
        }
        let filesystem = filesystem.unwrap_or_default();

        if request.name.contains("/..")
            || request.name.contains("../")
            || request.name == ".."
            || request.name == "."
            || request.name == "./"
        {
            return Err(Status::invalid_argument(format!("invalid volume name")));
        }
        let host_base_path = if host_base_path.ends_with("/") {
            &host_base_path[..host_base_path.len() - 1]
        } else {
            &host_base_path
        };
        let host_base_path = if host_base_path.starts_with("/") {
            &host_base_path[1..]
        } else {
            &host_base_path
        };
        let host_path = format!("{}/{}", host_base_path, request.name);

        let new_volume = store::Volume {
            name: request.name,
            size: match request.capacity_range {
                None => 1073741824, // 1 GiB
                Some(capacity) => capacity.required_bytes as u64,
            },
            assigned_node_id: CONFIG.node_id.clone(),
            state: VolumeState::Open,
            published_readonly: false,
            published_config: None,
            host_path,
            filesystem,
            valid_configs,
            loop_device: None,
            mount_paths: vec![],
        };
        let creation = new_volume.create().await.map_err(|e| {
            error!("failed to save volume for creation: {e:#}");
            Status::internal("internal failure")
        })?;
        match creation {
            VolumeCreation::AlreadyExists => {
                let Some(existing) = store::Volume::load(new_volume.name.clone()).await.map_err(|e| {
                    error!("failed to load volume for reconciliation: {e:#}");
                    Status::internal("internal failure")
                })? else {
                    // race conditioned
                    return Err(Status::already_exists("volume name already exists"));
                };
                if !(existing.valid_configs == new_volume.valid_configs
                    && existing.filesystem == new_volume.filesystem
                    && existing.host_path == new_volume.host_path
                    && existing.assigned_node_id == new_volume.assigned_node_id
                    && existing.size == new_volume.size)
                {
                    return Err(Status::already_exists("volume name already exists"));
                }
                return Ok(Response::new(CreateVolumeResponse {
                    volume: Some(Volume {
                        capacity_bytes: new_volume.size as i64,
                        volume_id: new_volume.name,
                        volume_context: Default::default(),
                        content_source: None,
                        accessible_topology: vec![Topology {
                            segments: [("node".to_string(), new_volume.assigned_node_id)]
                                .into_iter()
                                .collect(),
                        }],
                    }),
                }));
            }
            VolumeCreation::Success => (),
        }
        //TODO: validate volume size?

        let total_path = CONFIG.host_prefix.join(&new_volume.host_path);
        info!("making new volume @ '{}'", total_path.display());
        if let Err(e) = make_volume(&total_path, new_volume.size, filesystem).await {
            error!(
                "failed to make new volume file: {e:#} @ {}",
                total_path.display()
            );
            new_volume.delete().await.map_err(|e| {
                error!("failed to delete failed volume: {e:#}");
                Status::internal("internal failure")
            })?;

            return Err(Status::internal("internal failure"));
        }

        Ok(Response::new(CreateVolumeResponse {
            volume: Some(Volume {
                capacity_bytes: new_volume.size as i64,
                volume_id: new_volume.name,
                volume_context: Default::default(),
                content_source: None,
                accessible_topology: vec![Topology {
                    segments: [("node".to_string(), new_volume.assigned_node_id)]
                        .into_iter()
                        .collect(),
                }],
            }),
        }))
    }

    async fn delete_volume(
        &self,
        request: Request<DeleteVolumeRequest>,
    ) -> Result<Response<DeleteVolumeResponse>, Status> {
        let request = request.into_inner();

        if request.volume_id.is_empty() {
            return Err(Status::invalid_argument("missing volume_id"));
        }

        let volume = store::Volume::load(request.volume_id).await.map_err(|e| {
            error!("failed to load volume for deletion: {e:#}");
            Status::internal("internal failure")
        })?;
        if let Some(volume) = volume {
            if !matches!(volume.state, VolumeState::Open) {
                return Err(Status::failed_precondition(format!(
                    "volume is in use, state = {:?}",
                    volume.state
                )));
            }
            let total_path = CONFIG.host_prefix.join(&volume.host_path);
            if volume.filesystem == Filesystem::Bind {
                tokio::fs::remove_dir_all(&total_path).await
            } else {
                tokio::fs::remove_file(&total_path).await
            }
            .map_err(|e| {
                error!("failed to delete volume file: {e:#}");
                Status::internal("internal failure")
            })?;

            volume.delete().await.map_err(|e| {
                error!("failed to delete volume: {e:#}");
                Status::internal("internal failure")
            })?;
        }
        Ok(Response::new(DeleteVolumeResponse {}))
    }

    async fn controller_publish_volume(
        &self,
        request: Request<ControllerPublishVolumeRequest>,
    ) -> Result<Response<ControllerPublishVolumeResponse>, Status> {
        let request = request.into_inner();

        if request.volume_id.is_empty() {
            return Err(Status::invalid_argument("missing volume_id"));
        }
        if request.node_id.is_empty() {
            return Err(Status::invalid_argument("missing node_id"));
        }
        if request.volume_capability.is_none() {
            return Err(Status::invalid_argument("no capabilities specified"));
        }

        let mut volume = match store::Volume::load(request.volume_id).await {
            Ok(Some(x)) => x,
            Ok(None) => return Err(Status::not_found("volume_id not found")),
            Err(e) => {
                error!("failed to load volume for deletion: {e:#}");
                return Err(Status::internal("internal failure"));
            }
        };
        if volume.assigned_node_id != request.node_id {
            return Err(Status::not_found(format!(
                "volume is locked to node {}, cannot move volume through publish",
                volume.assigned_node_id
            )));
        }
        let Some(capability) = &request.volume_capability else {
            return Err(Status::invalid_argument("missing volume_capability"));
        };
        let (config, filesystem) = parse_volume_capability(capability)?;

        match volume.state {
            VolumeState::NodePublished => {
                return Err(Status::failed_precondition(
                    "volume currently mounted on node",
                ));
            }
            VolumeState::ControllerPublished => {
                if !volume.valid_configs.contains(&config)
                    || filesystem
                        .map(|x| x != volume.filesystem)
                        .unwrap_or_default()
                    || volume.published_readonly != request.readonly
                {
                    return Err(Status::already_exists("incompatible volume_capability"));
                }
                //todo: do we need to update the volume state?
                return Ok(Response::new(ControllerPublishVolumeResponse {
                    publish_context: Default::default(),
                }));
            }
            VolumeState::Open => {
                if !volume.valid_configs.contains(&config) {
                    return Err(Status::invalid_argument("incompatible volume_capability"));
                }
                volume.published_config = Some(config);
                volume.state = VolumeState::ControllerPublished;
                volume.published_readonly = request.readonly;
                volume.update().await.map_err(|e| {
                    error!("failed to update volume: {e:#}");
                    Status::internal("failed to update volume")
                })?;
            }
        }

        return Ok(Response::new(ControllerPublishVolumeResponse {
            publish_context: Default::default(),
        }));
    }

    async fn controller_unpublish_volume(
        &self,
        request: Request<ControllerUnpublishVolumeRequest>,
    ) -> Result<Response<ControllerUnpublishVolumeResponse>, Status> {
        let request = request.into_inner();

        if request.volume_id.is_empty() {
            return Err(Status::invalid_argument("missing volume_id"));
        }

        let mut volume = match store::Volume::load(request.volume_id).await {
            Ok(Some(x)) => x,
            Ok(None) => return Ok(Response::new(ControllerUnpublishVolumeResponse {})),
            // Ok(None) => return Err(Status::not_found("volume_id not found")),
            Err(e) => {
                error!("failed to load volume for deletion: {e:#}");
                return Err(Status::internal("internal failure"));
            }
        };

        match volume.state {
            VolumeState::NodePublished => {
                return Err(Status::failed_precondition(
                    "volume currently mounted on node",
                ))
            }
            VolumeState::Open => return Ok(Response::new(ControllerUnpublishVolumeResponse {})),
            VolumeState::ControllerPublished => (),
        }

        volume.state = VolumeState::Open;
        volume.published_config = None;
        volume.published_readonly = false;
        volume.update().await.map_err(|e| {
            error!("failed to update volume: {e:#}");
            Status::internal("failed to update volume")
        })?;

        Ok(Response::new(ControllerUnpublishVolumeResponse {}))
    }

    async fn validate_volume_capabilities(
        &self,
        request: Request<ValidateVolumeCapabilitiesRequest>,
    ) -> Result<Response<ValidateVolumeCapabilitiesResponse>, Status> {
        let request = request.into_inner();

        if request.volume_id.is_empty() {
            return Err(Status::invalid_argument("missing volume_id"));
        }
        if request.volume_capabilities.is_empty() {
            return Err(Status::invalid_argument("no capabilities specified"));
        }

        let volume = match store::Volume::load(request.volume_id).await {
            Ok(Some(x)) => x,
            Ok(None) => return Err(Status::not_found("volume_id not found")),
            Err(e) => {
                error!("failed to load volume for deletion: {e:#}");
                return Err(Status::internal("internal failure"));
            }
        };
        for capability in &request.volume_capabilities {
            let (config, filesystem) = parse_volume_capability(capability)?;
            if !volume.valid_configs.contains(&config)
                || filesystem
                    .map(|x| x != volume.filesystem)
                    .unwrap_or_default()
            {
                return Err(Status::already_exists("incompatible volume_capability"));
            }
        }

        Ok(Response::new(ValidateVolumeCapabilitiesResponse {
            confirmed: Some(validate_volume_capabilities_response::Confirmed {
                volume_context: Default::default(),
                volume_capabilities: request.volume_capabilities.clone(),
                parameters: Default::default(),
            }),
            message: String::new(),
        }))
    }

    async fn list_volumes(
        &self,
        request: Request<ListVolumesRequest>,
    ) -> Result<Response<ListVolumesResponse>, Status> {
        let request = request.into_inner();

        let mut entries = vec![];
        let mut volumes = store::Volume::list().await.map_err(|e| {
            error!("failed to list volumes: {e:#}");
            Status::internal("failed to list volumes")
        })?;
        volumes.sort_by(|x, y| x.name.cmp(&y.name));

        if !request.starting_token.is_empty() {
            if !volumes.iter().any(|x| x.name == request.starting_token) {
                return Err(Status::aborted("invalid starting_token"));
            }
        }

        let mut next_token = String::new();
        for volume in volumes {
            if !request.starting_token.is_empty() && volume.name <= request.starting_token {
                continue;
            }
            entries.push(crate::proto::list_volumes_response::Entry {
                volume: Some(Volume {
                    capacity_bytes: volume.size as i64,
                    volume_id: volume.name.clone(),
                    volume_context: Default::default(),
                    content_source: None,
                    accessible_topology: vec![Topology {
                        segments: [("node".to_string(), volume.assigned_node_id.clone())]
                            .into_iter()
                            .collect(),
                    }],
                }),
                status: Some(VolumeStatus {
                    published_node_ids: if matches!(volume.state, VolumeState::NodePublished) {
                        vec![volume.assigned_node_id]
                    } else {
                        vec![]
                    },
                    volume_condition: None,
                }),
            });
            if request.max_entries > 0 && request.max_entries as usize <= entries.len() {
                next_token = volume.name;
                break;
            }
        }

        Ok(Response::new(ListVolumesResponse {
            entries,
            next_token,
        }))
    }

    async fn get_capacity(
        &self,
        request: Request<GetCapacityRequest>,
    ) -> Result<Response<GetCapacityResponse>, Status> {
        let request = request.into_inner();

        if let Some(requirement) = &request.accessible_topology {
            for (key, value) in &requirement.segments {
                if key != "node" || value != &CONFIG.node_id {
                    return Err(Status::resource_exhausted(
                        "invalid accessibility_requirements, only allowed node=<node id>",
                    ));
                }
            }
        }

        let mut host_base_path = None::<String>;
        for (name, value) in &request.parameters {
            match &**name {
                "host_base_path" => host_base_path = Some(value.clone()),
                _ => {
                    return Err(Status::invalid_argument(format!(
                        "unknown parameter {name}"
                    )))
                }
            }
        }
        let Some(host_base_path) = host_base_path else {
            return Ok(Response::new(GetCapacityResponse {
                available_capacity: 0,
                maximum_volume_size: None,
                minimum_volume_size: None,
            }));
        };

        let stats = crate::statfs::statfs(&Path::new(&host_base_path))
            .await
            .map_err(|e| {
                error!("failed to get fs stats: {e:#}");
                Status::internal("failed to get fs stats")
            })?;

        Ok(Response::new(GetCapacityResponse {
            available_capacity: (stats.blocks_free_unprivileged * stats.block_size) as i64,
            maximum_volume_size: None,
            minimum_volume_size: None,
        }))
    }

    async fn controller_get_capabilities(
        &self,
        _request: Request<ControllerGetCapabilitiesRequest>,
    ) -> Result<Response<ControllerGetCapabilitiesResponse>, Status> {
        Ok(Response::new(ControllerGetCapabilitiesResponse {
            capabilities: vec![
                ControllerServiceCapability {
                    r#type: Some(CapabilityType::Rpc(Rpc {
                        r#type: RpcType::CreateDeleteVolume as i32,
                    })),
                },
                ControllerServiceCapability {
                    r#type: Some(CapabilityType::Rpc(Rpc {
                        r#type: RpcType::PublishUnpublishVolume as i32,
                    })),
                },
                ControllerServiceCapability {
                    r#type: Some(CapabilityType::Rpc(Rpc {
                        r#type: RpcType::GetVolume as i32,
                    })),
                },
                ControllerServiceCapability {
                    r#type: Some(CapabilityType::Rpc(Rpc {
                        r#type: RpcType::ListVolumes as i32,
                    })),
                },
                ControllerServiceCapability {
                    r#type: Some(CapabilityType::Rpc(Rpc {
                        r#type: RpcType::ListVolumesPublishedNodes as i32,
                    })),
                },
                ControllerServiceCapability {
                    r#type: Some(CapabilityType::Rpc(Rpc {
                        r#type: RpcType::SingleNodeMultiWriter as i32,
                    })),
                },
                ControllerServiceCapability {
                    r#type: Some(CapabilityType::Rpc(Rpc {
                        r#type: RpcType::GetCapacity as i32,
                    })),
                },
                ControllerServiceCapability {
                    r#type: Some(CapabilityType::Rpc(Rpc {
                        r#type: RpcType::PublishReadonly as i32,
                    })),
                },
            ],
        }))
    }

    async fn create_snapshot(
        &self,
        request: Request<CreateSnapshotRequest>,
    ) -> Result<Response<CreateSnapshotResponse>, Status> {
        let request = request.into_inner();
        info!("create_snapshot = {request:#?}");

        Err(Status::unimplemented(
            "controller doesn't have CREATE_DELETE_SNAPSHOT capability",
        ))
    }

    async fn delete_snapshot(
        &self,
        request: Request<DeleteSnapshotRequest>,
    ) -> Result<Response<DeleteSnapshotResponse>, Status> {
        let request = request.into_inner();
        info!("delete_snapshot = {request:#?}");

        Err(Status::unimplemented(
            "controller doesn't have CREATE_DELETE_SNAPSHOT capability",
        ))
    }

    async fn list_snapshots(
        &self,
        request: Request<ListSnapshotsRequest>,
    ) -> Result<Response<ListSnapshotsResponse>, Status> {
        let request = request.into_inner();
        info!("list_snapshots = {request:#?}");

        Err(Status::unimplemented(
            "controller doesn't have LIST_SNAPSHOTS capability",
        ))
    }

    async fn controller_expand_volume(
        &self,
        request: Request<ControllerExpandVolumeRequest>,
    ) -> Result<Response<ControllerExpandVolumeResponse>, Status> {
        let request = request.into_inner();
        info!("controller_expand_volume = {request:#?}");

        Err(Status::unimplemented("controller requires node expansion"))
    }

    async fn controller_get_volume(
        &self,
        request: Request<ControllerGetVolumeRequest>,
    ) -> Result<Response<ControllerGetVolumeResponse>, Status> {
        let request = request.into_inner();

        if request.volume_id.is_empty() {
            return Err(Status::invalid_argument("missing volume_id"));
        }

        let volume = match store::Volume::load(request.volume_id).await {
            Ok(Some(x)) => x,
            Ok(None) => return Err(Status::not_found("volume_id not found")),
            Err(e) => {
                error!("failed to load volume for deletion: {e:#}");
                return Err(Status::internal("internal failure"));
            }
        };
        let raw_volume = Volume {
            capacity_bytes: volume.size as i64,
            volume_id: volume.name.clone(),
            volume_context: Default::default(),
            content_source: None,
            accessible_topology: vec![Topology {
                segments: [("node".to_string(), volume.assigned_node_id.clone())]
                    .into_iter()
                    .collect(),
            }],
        };
        let status = crate::proto::controller_get_volume_response::VolumeStatus {
            published_node_ids: if matches!(volume.state, VolumeState::NodePublished) {
                vec![volume.assigned_node_id]
            } else {
                vec![]
            },
            volume_condition: None,
        };

        Ok(Response::new(ControllerGetVolumeResponse {
            volume: Some(raw_volume),
            status: Some(status),
        }))
    }
}
