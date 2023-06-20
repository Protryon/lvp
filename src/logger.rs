use std::{
    fmt,
    task::{Context, Poll},
    time::Instant,
};

use futures::Future;
use http::{Method, Request, Response};
use http_body::Body;
use log::log;
use tonic::Code;
use tower_layer::Layer;
use tower_service::Service;

#[derive(Clone)]
pub struct LoggerLayer;

impl<S> Layer<S> for LoggerLayer {
    type Service = Logger<S>;

    fn layer(&self, service: S) -> Self::Service {
        Logger::new(service)
    }
}

#[derive(Clone)]
pub struct Logger<S> {
    inner: S,
}

impl<S> Logger<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

#[pin_project::pin_project]
pub struct LoggerFuture<S, ReqBody, ResBody>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    S::Error: fmt::Display + 'static,
{
    remote_addr: String,
    path: String,
    level: log::Level,
    method: Method,
    start: Instant,
    #[pin]
    inner: S::Future,
}

impl<S, ReqBody, ResBody> Future for LoggerFuture<S, ReqBody, ResBody>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    S::Error: fmt::Display + 'static,
{
    type Output = <S::Future as Future>::Output;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match this.inner.poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(response)) => {
                let code = if let Some(status) = response
                    .headers()
                    .get("grpc-status")
                    .and_then(|x| x.to_str().ok()?.trim().parse::<i32>().ok())
                {
                    Code::from_i32(status)
                } else {
                    Code::Ok
                };
                let message = if let Some(message) = response
                    .headers()
                    .get("grpc-message")
                    .and_then(|x| x.to_str().ok())
                {
                    message
                } else {
                    ""
                };

                log!(
                    *this.level,
                    "[{}] {} {} -> {:?} {} [{:.02} ms]",
                    this.remote_addr,
                    this.method,
                    this.path,
                    code,
                    message,
                    this.start.elapsed().as_secs_f64() * 1000.0
                );
                Poll::Ready(Ok(response))
            }
            Poll::Ready(Err(e)) => {
                log!(
                    *this.level,
                    "[{}] {} {} -> FAIL {} [{:.02} ms]",
                    this.remote_addr,
                    this.method,
                    this.path,
                    e,
                    this.start.elapsed().as_secs_f64() * 1000.0
                );
                Poll::Ready(Err(e))
            }
        }
    }
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for Logger<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    ReqBody: Body,
    ResBody: Body,
    ResBody::Error: fmt::Display + 'static,
    S::Error: fmt::Display + 'static,
{
    type Response = Response<ResBody>;
    type Error = S::Error;
    type Future = LoggerFuture<S, ReqBody, ResBody>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let start = Instant::now();

        let path = req.uri().path().to_string();

        let remote_addr = if let Some(forwarded) = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|x| x.to_str().ok())
        {
            forwarded.to_string()
        } else {
            "unknown".to_string()
        };
        let method = req.method().clone();
        let future = self.inner.call(req);
        let level = log::Level::Info;

        LoggerFuture {
            start,
            level,
            method,
            remote_addr,
            path,
            inner: future,
        }
    }
}
