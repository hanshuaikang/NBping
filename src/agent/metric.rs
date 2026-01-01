use prometheus::{CounterVec, HistogramVec, HistogramOpts, Opts, Registry, TextEncoder};
use std::sync::Arc;

/// Prometheus 指标收集器
#[derive(Debug, Clone)]
pub struct PrometheusMetrics {
    /// ping 延迟直方图指标
    ping_duration_histogram: HistogramVec,
    /// ping 请求总数（按状态分组）
    ping_requests_total: CounterVec,
    /// Prometheus 注册表
    registry: Arc<Registry>,
}

impl PrometheusMetrics {
    /// 创建新的 Prometheus 指标收集器
    pub fn new() -> Result<Self, prometheus::Error> {
        // 创建注册表
        let registry = Arc::new(Registry::new());

        // 定义延迟的 buckets (秒): 1ms, 5ms, 10ms, 50ms, 100ms, 500ms, 1s, 5s, 10s, +Inf
        let buckets = vec![
            0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0,
        ];

        // 创建直方图指标
        let ping_duration_histogram = HistogramVec::new(
            HistogramOpts::new(
                "nbping_ping_duration_seconds",
                "Histogram of ping durations in seconds",
            )
                .buckets(buckets),
            &["target", "ip"], // label 名称
        )?;

        // 创建请求总数计数器
        let ping_requests_total = CounterVec::new(
            Opts::new(
                "nbping_ping_requests_total",
                "Total number of ping requests",
            ),
            &["target", "ip", "status"],
        )?;

        // 注册指标
        registry.register(Box::new(ping_duration_histogram.clone()))?;
        registry.register(Box::new(ping_requests_total.clone()))?;

        Ok(Self {
            ping_duration_histogram,
            ping_requests_total,
            registry,
        })
    }

    /// 记录成功的 ping（记录到直方图）
    pub fn record_ping_success(&self, target: &str, ip: &str, rtt_ms: f64) {
        let rtt_seconds = rtt_ms / 1000.0;

        self.ping_requests_total
            .with_label_values(&[target, ip, "success"])
            .inc();

        // 为直方图添加 labels 并观察值
        self.ping_duration_histogram
            .with_label_values(&[target, ip])
            .observe(rtt_seconds);
    }

    /// 记录超时的 ping（不记录到直方图，但可以在这里添加其他指标）
    pub fn record_ping_timeout(&self, target: &str, ip: &str) {
        self.ping_requests_total
            .with_label_values(&[target, ip, "timeout"])
            .inc();
    }

    /// 记录错误的 ping
    pub fn record_ping_error(&self, target: &str, ip: &str) {
        self.ping_requests_total
            .with_label_values(&[target, ip, "error"])
            .inc();
    }

    /// 获取 Prometheus 格式的指标数据
    pub fn gather(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();

        encoder.encode_to_string(&metric_families).unwrap_or_else(|e| {
            eprintln!("Error encoding metrics: {}", e);
            String::new()
        })
    }

}

impl Default for PrometheusMetrics {
    fn default() -> Self {
        Self::new().expect("Failed to create PrometheusMetrics")
    }
}

/// HTTP 服务器，用于暴露 /metrics 端点
pub mod http_server {
    use super::*;
    use hyper::service::service_fn;
    use hyper::{Method, Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;
    use hyper_util::server::conn::auto::Builder;
    use http_body_util::Full;
    use hyper::body::Bytes;
    use std::convert::Infallible;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::net::TcpListener;

    /// 启动 Prometheus metrics HTTP 服务器，支持优雅关闭
    pub async fn start_metrics_server(
        metrics: Arc<PrometheusMetrics>,
        addr: SocketAddr,
        mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let listener = TcpListener::bind(addr).await?;

        loop {
            tokio::select! {
                // 接受新连接
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, _)) => {
                            let metrics = metrics.clone();
                            
                            tokio::task::spawn(async move {
                                let io = TokioIo::new(stream);
                                let service = service_fn(move |req| {
                                    handle_request(req, metrics.clone())
                                });

                                if let Err(err) = Builder::new(hyper_util::rt::TokioExecutor::new())
                                    .serve_connection(io, service)
                                    .await
                                {
                                    eprintln!("Error serving connection: {:?}", err);
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("Failed to accept connection: {}", e);
                        }
                    }
                }
                // 接收关闭信号
                _ = &mut shutdown_rx => {
                    println!("Metrics server shutting down gracefully");
                    break;
                }
            }
        }

        Ok(())
    }

    /// 处理 HTTP 请求
    async fn handle_request(
        req: Request<hyper::body::Incoming>,
        metrics: Arc<PrometheusMetrics>,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        match (req.method(), req.uri().path()) {
            (&Method::GET, "/metrics") => {
                let metrics_output = metrics.gather();
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "text/plain; charset=utf-8")
                    .body(Full::new(Bytes::from(metrics_output)))
                    .unwrap())
            }
            (&Method::GET, "/") => {
                let body = r#"<html>
<head><title>NPing Metrics</title></head>
<body>
<h1>NPing Metrics</h1>
<p><a href='/metrics'>Metrics</a></p>
</body>
</html>"#;
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "text/html")
                    .body(Full::new(Bytes::from(body)))
                    .unwrap())
            }
            _ => {
                Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Full::new(Bytes::from("Not Found")))
                    .unwrap())
            }
        }
    }
}
