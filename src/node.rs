use std::{
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

use crate::{
    chroot::{run, run_in_chroot},
    config::{CONFIG, NODE},
    controller::parse_volume_capability,
    proto::{
        node_server::Node, node_service_capability::rpc::Type as RpcType,
        node_service_capability::Rpc, node_service_capability::Type as CapabilityType, *,
    },
    store::{self, Filesystem, VolumeMode, VolumeState},
};
use anyhow::Result;
use log::{error, info};
use tokio::fs::OpenOptions;
use tonic::{Request, Response, Status};

#[derive(Debug)]
pub struct NodeService {}

async fn mount_volume(
    loop_device: Option<&Path>,
    source: &Path,
    target: &Path,
    is_readonly: bool,
    filesystem: Filesystem,
) -> Result<Option<PathBuf>> {
    if filesystem == Filesystem::Bind {
        let mut mount_args = vec!["mount", "bind"];
        if is_readonly {
            mount_args.push("-r");
        }
        mount_args.push(source.to_str().unwrap());
        mount_args.push(target.to_str().unwrap());
        run(&mount_args).await?;
        return Ok(None);
    }
    let loop_device = if let Some(device) = loop_device {
        device.to_path_buf()
    } else {
        let output =
            run_in_chroot(&["losetup", "--show", "-L", "-f", source.to_str().unwrap()]).await?;
        info!("losetup pipe: {output}");
        // if !tokio::fs::try_exists(&pipe).await? {
        //     return Err(std::io::Error::new(ErrorKind::Other, "failed to find pipe"));
        // }
        output.into()
    };
    let mut mount_args = vec!["mount"];
    if is_readonly {
        mount_args.push("-r");
    }
    mount_args.push(loop_device.to_str().unwrap());
    mount_args.push(target.to_str().unwrap());
    run_in_chroot(&mount_args).await?;
    Ok(Some(loop_device))
}

async fn unloop_volume(target: &Path) -> Result<()> {
    run_in_chroot(&["losetup", "-d", target.to_str().unwrap()]).await?;
    Ok(())
}

async fn unmount_volume(target: &Path) -> Result<()> {
    run_in_chroot(&["umount", target.to_str().unwrap()]).await?;
    Ok(())
}

async fn expand_volume(
    source_file: &Path,
    loop_device: &Path,
    size: u64,
    filesystem: Filesystem,
) -> Result<()> {
    if filesystem == Filesystem::Bind {
        return Ok(());
    }
    // expand source volume
    let file = OpenOptions::new().write(true).open(source_file).await?;
    tokio::task::spawn_blocking(move || {
        if unsafe { libc::ftruncate(file.as_raw_fd(), size as i64) } < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
    .await??;
    // expand loop device
    run_in_chroot(&["losetup", "-c", loop_device.to_str().unwrap()]).await?;

    // expand filesystem
    match filesystem {
        Filesystem::Ext4 => {
            run(&["resize2fs", loop_device.to_str().unwrap()]).await?;
        }
        Filesystem::Xfs => {
            run(&["xfs_growfs", "-d", loop_device.to_str().unwrap()]).await?;
        }
        Filesystem::Bind => (),
    }

    Ok(())
}

#[async_trait::async_trait]
impl Node for NodeService {
    async fn node_stage_volume(
        &self,
        request: Request<NodeStageVolumeRequest>,
    ) -> Result<Response<NodeStageVolumeResponse>, Status> {
        let request = request.into_inner();
        info!("node_stage_volume = {request:#?}");

        Err(Status::unimplemented("STAGE_UNSTAGE_VOLUME not set"))
    }

    async fn node_unstage_volume(
        &self,
        request: Request<NodeUnstageVolumeRequest>,
    ) -> Result<Response<NodeUnstageVolumeResponse>, Status> {
        let request = request.into_inner();
        info!("node_unstage_volume = {request:#?}");

        Err(Status::unimplemented("STAGE_UNSTAGE_VOLUME not set"))
    }

    async fn node_publish_volume(
        &self,
        request: Request<NodePublishVolumeRequest>,
    ) -> Result<Response<NodePublishVolumeResponse>, Status> {
        let request = request.into_inner();

        if request.volume_id.is_empty() {
            return Err(Status::invalid_argument("volume_id is missing"));
        }
        if request.target_path.is_empty() {
            return Err(Status::invalid_argument("missing target_path"));
        }
        let target: PathBuf = request.target_path.into();

        let Some(capability) = &request.volume_capability else {
            return Err(Status::invalid_argument("missing volume_capability"));
        };
        let (requested_config, requested_filesystem) = parse_volume_capability(capability)?;

        let mut volume = match store::Volume::load(request.volume_id).await {
            Ok(Some(x)) => x,
            Ok(None) => return Err(Status::not_found("volume_id not found")),
            Err(e) => {
                error!("failed to load volume for deletion: {e:#}");
                return Err(Status::internal("internal failure"));
            }
        };

        if !volume.valid_configs.contains(&requested_config)
            || requested_filesystem
                .map(|x| x != volume.filesystem)
                .unwrap_or_default()
        {
            return Err(Status::already_exists("incompatible volume_capability"));
        }

        match volume.state {
            VolumeState::NodePublished => {
                if let Some(config) = &volume.published_config {
                    match config.mode {
                        VolumeMode::SingleNodeMultiWriter => (),
                        _ => {
                            if !volume.mount_paths.contains(&target) {
                                return Err(Status::failed_precondition("volume already published on node and not configured for multiwrite"));
                            }
                        } //todo: something for singlewriter?
                    }
                    if config != &requested_config {
                        return Err(Status::failed_precondition(
                            "volume attempted to mount in different mode",
                        ));
                    }
                }
            }
            VolumeState::Open => {
                return Err(Status::failed_precondition(
                    "volume not published on controller",
                ))
            }
            VolumeState::ControllerPublished => (),
        }

        if volume.mount_paths.contains(&target) {
            return Ok(Response::new(NodePublishVolumeResponse {}));
        }

        if let Err(e) = tokio::fs::create_dir_all(&target).await {
            error!(
                "failed to create volume mountdir '{}': {e}",
                target.display()
            );
            return Err(Status::internal("failed to create volume mountdir"));
        }

        let host_path = if volume.host_path.starts_with("/") {
            &volume.host_path[1..]
        } else {
            &volume.host_path
        };
        let total_path = CONFIG.host_prefix.join(host_path);

        let loop_device = match mount_volume(
            volume.loop_device.as_deref(),
            &total_path,
            &target,
            request.readonly || volume.published_readonly,
            volume.filesystem,
        )
        .await
        {
            Ok(x) => x,
            Err(e) => {
                error!(
                    "failed to mount volume '{}' to '{}': {e}",
                    total_path.display(),
                    target.display()
                );
                return Err(Status::internal("failed to mount volume"));
            }
        };

        if volume.loop_device.is_none() && loop_device.is_some() {
            volume.loop_device = loop_device;
        }
        volume.mount_paths.push(target);

        volume.state = VolumeState::NodePublished;
        volume.update().await.map_err(|e| {
            error!("failed to update volume: {e:#}");
            Status::internal("failed to update volume")
        })?;

        Ok(Response::new(NodePublishVolumeResponse {}))
    }

    async fn node_unpublish_volume(
        &self,
        request: Request<NodeUnpublishVolumeRequest>,
    ) -> Result<Response<NodeUnpublishVolumeResponse>, Status> {
        let request = request.into_inner();

        if request.volume_id.is_empty() {
            return Err(Status::invalid_argument("volume_id not found"));
        }
        if request.target_path.is_empty() {
            return Err(Status::invalid_argument("missing target_path"));
        }

        let mut volume = match store::Volume::load(request.volume_id).await {
            Ok(Some(x)) => x,
            Ok(None) => return Err(Status::not_found("volume_id not found")),
            Err(e) => {
                error!("failed to load volume for deletion: {e:#}");
                return Err(Status::internal("internal failure"));
            }
        };

        let target: PathBuf = request.target_path.into();
        if !matches!(volume.state, VolumeState::NodePublished)
            || !volume.mount_paths.contains(&target)
        {
            return Ok(Response::new(NodeUnpublishVolumeResponse {}));
        }

        if let Err(e) = unmount_volume(&target).await {
            error!("failed to unmount volume '{}': {e}", target.display());
            return Err(Status::internal("failed to unmount volume"));
        }

        volume.mount_paths.retain(|x| x != &target);
        if volume.mount_paths.is_empty() {
            volume.state = VolumeState::ControllerPublished;
            if let Some(loop_device) = volume.loop_device.take() {
                if let Err(e) = unloop_volume(&loop_device).await {
                    error!("failed to unloop volume '{}': {e}", target.display());
                    // not returning here since we've already gone too far
                }
            }
        }
        volume.update().await.map_err(|e| {
            error!("failed to save volume: {e:#}");
            Status::internal("failed to save volume")
        })?;
        if let Err(e) = tokio::fs::remove_dir(&target).await {
            error!("failed to delete target dir, ignoring: {e}");
        }

        Ok(Response::new(NodeUnpublishVolumeResponse {}))
    }

    async fn node_get_volume_stats(
        &self,
        request: Request<NodeGetVolumeStatsRequest>,
    ) -> Result<Response<NodeGetVolumeStatsResponse>, Status> {
        let request = request.into_inner();

        if request.volume_id.is_empty() {
            return Err(Status::invalid_argument("volume_id not found"));
        }
        if request.volume_path.is_empty() {
            return Err(Status::invalid_argument("volume_path not found"));
        }

        let volume = match store::Volume::load(request.volume_id).await {
            Ok(Some(x)) => x,
            Ok(None) => return Err(Status::not_found("volume_id not found")),
            Err(e) => {
                error!("failed to load volume for deletion: {e:#}");
                return Err(Status::internal("internal failure"));
            }
        };

        let volume_path: PathBuf = request.volume_path.into();

        if !matches!(volume.state, VolumeState::NodePublished)
            || !volume.mount_paths.contains(&volume_path)
        {
            return Err(Status::not_found("volume path and id not found"));
        }

        match crate::statfs::statfs(&volume_path).await {
            Err(e) => {
                error!(
                    "failed to fetch volume stats for '{}': {e}",
                    volume_path.display()
                );
                Err(Status::internal("internal failure"))
            }
            Ok(stats) => Ok(Response::new(NodeGetVolumeStatsResponse {
                usage: vec![
                    VolumeUsage {
                        available: (stats.blocks_free_unprivileged * stats.block_size) as i64,
                        total: (stats.block_count * stats.block_size) as i64,
                        used: ((stats.block_count - stats.blocks_free_unprivileged)
                            * stats.block_size) as i64,
                        unit: volume_usage::Unit::Bytes as i32,
                    },
                    VolumeUsage {
                        available: stats.inodes_free_unprivileged as i64,
                        total: stats.inodes as i64,
                        used: (stats.inodes - stats.inodes_free_unprivileged) as i64,
                        unit: volume_usage::Unit::Inodes as i32,
                    },
                ],
                volume_condition: Some(VolumeCondition {
                    abnormal: false,
                    message: String::new(),
                }),
            })),
        }
    }

    async fn node_expand_volume(
        &self,
        request: Request<NodeExpandVolumeRequest>,
    ) -> Result<Response<NodeExpandVolumeResponse>, Status> {
        let request = request.into_inner();

        if request.volume_id.is_empty() {
            return Err(Status::invalid_argument("volume_id not found"));
        }
        if request.volume_path.is_empty() {
            return Err(Status::invalid_argument("volume_path not found"));
        }

        let mut volume = match store::Volume::load(request.volume_id).await {
            Ok(Some(x)) => x,
            Ok(None) => return Err(Status::not_found("volume_id not found")),
            Err(e) => {
                error!("failed to load volume for deletion: {e:#}");
                return Err(Status::internal("internal failure"));
            }
        };

        let volume_path: PathBuf = request.volume_path.into();

        if !matches!(volume.state, VolumeState::NodePublished)
            || !volume.mount_paths.contains(&volume_path)
        {
            return Err(Status::not_found("volume path and id not found"));
        }

        let target_capacity = match request.capacity_range {
            None => 1073741824, // 1 GiB
            Some(capacity) => capacity.required_bytes as u64,
        };
        if target_capacity <= volume.size {
            return Ok(Response::new(NodeExpandVolumeResponse {
                capacity_bytes: volume.size as i64,
            }));
        }

        let Some(loop_device) = &volume.loop_device else {
            return Err(Status::not_found("loop device not found"));
        };

        let total_path = CONFIG.host_prefix.join(&volume.host_path);
        if let Err(e) = expand_volume(
            &total_path,
            &loop_device,
            target_capacity,
            volume.filesystem,
        )
        .await
        {
            error!("failed to resize volume: {e}");
            return Err(Status::internal("failed to resize volume"));
        }

        volume.size = target_capacity;
        volume.update().await.map_err(|e| {
            error!("failed to save volume: {e:#}");
            Status::internal("failed to save volume")
        })?;

        return Ok(Response::new(NodeExpandVolumeResponse {
            capacity_bytes: volume.size as i64,
        }));
    }

    async fn node_get_capabilities(
        &self,
        _request: Request<NodeGetCapabilitiesRequest>,
    ) -> Result<Response<NodeGetCapabilitiesResponse>, Status> {
        Ok(Response::new(NodeGetCapabilitiesResponse {
            capabilities: vec![
                NodeServiceCapability {
                    r#type: Some(CapabilityType::Rpc(Rpc {
                        r#type: RpcType::ExpandVolume as i32,
                    })),
                },
                NodeServiceCapability {
                    r#type: Some(CapabilityType::Rpc(Rpc {
                        r#type: RpcType::SingleNodeMultiWriter as i32,
                    })),
                },
                NodeServiceCapability {
                    r#type: Some(CapabilityType::Rpc(Rpc {
                        r#type: RpcType::GetVolumeStats as i32,
                    })),
                },
            ],
        }))
    }

    async fn node_get_info(
        &self,
        _request: Request<NodeGetInfoRequest>,
    ) -> Result<Response<NodeGetInfoResponse>, Status> {
        Ok(Response::new(NodeGetInfoResponse {
            node_id: NODE.clone(),
            max_volumes_per_node: 0,
            accessible_topology: Some(Topology {
                segments: CONFIG.topology.clone(),
            }),
        }))
    }
}
