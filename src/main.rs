use std::{
    io::ErrorKind,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use crate::{
    config::CONFIG,
    proto::{
        controller_server::ControllerServer, identity_server::IdentityServer,
        node_server::NodeServer,
    },
};
use controller::ControllerService;
use futures::Stream;
use hyper::server::accept::Accept;
use hyper_unix_connector::UnixConnector;
use identity::IdentityService;
use log::{error, info};
use node::NodeService;
use tokio::net::{UnixListener, UnixStream};
use tonic::transport::Server;

mod chroot;
mod config;
mod controller;
mod identity;
mod logger;
mod node;
mod proto;
mod statfs;
mod store;

struct StreamWrapper(UnixConnector);

impl Stream for StreamWrapper {
    type Item = Result<UnixStream, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0)
            .poll_accept(cx)
            .map_err(|e| match e.downcast::<std::io::Error>() {
                Ok(e) => e,
                Err(e) => std::io::Error::new(ErrorKind::Other, e.to_string()),
            })
    }
}

#[tokio::main]
async fn main() {
    env_logger::Builder::new()
        .parse_env(env_logger::Env::default().default_filter_or("info"))
        .init();
    lazy_static::initialize(&CONFIG);
    if let Err(e) = store::init_client().await {
        error!("failed to connect to etcd: {e:#}");
        std::process::exit(1);
    }

    if let Err(e) = chroot::create().await {
        error!("failed to create chroot: {e:#}");
        std::process::exit(1);
    }

    let service = Server::builder()
        .tcp_keepalive(Some(Duration::from_secs(5)))
        .max_concurrent_streams(50)
        .layer(logger::LoggerLayer)
        .add_service(IdentityServer::new(IdentityService {}))
        .add_service(NodeServer::new(NodeService {}))
        .add_service(ControllerServer::new(ControllerService {}));

    info!("listening on {}", CONFIG.socket_path.display());

    if tokio::fs::try_exists(&CONFIG.socket_path).await.unwrap() {
        tokio::fs::remove_file(&CONFIG.socket_path).await.unwrap();
    }

    service
        .serve_with_incoming(StreamWrapper(
            UnixListener::bind(&CONFIG.socket_path).unwrap().into(),
        ))
        .await
        .unwrap();
}
