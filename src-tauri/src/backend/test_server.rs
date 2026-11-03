use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

#[derive(Clone)]
pub(crate) struct Request {
    pub method: String,
    pub path: String,
    pub authorization: String,
    pub body: serde_json::Value,
}
pub(crate) struct Server {
    pub endpoint: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) async fn serve<F, Fut>(handler: F) -> Server
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = String> + Send + 'static,
{
    serve_status(move |request| {
        let response = handler(request);
        async move { (200, response.await) }
    })
    .await
}
pub(crate) async fn serve_status<F, Fut>(handler: F) -> Server
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = (u16, String)> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let records = requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut first = String::new();
            reader.read_line(&mut first).await.unwrap();
            let mut len = 0;
            let mut authorization = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                if line == "\r\n" {
                    break;
                }
                let (name, value) = line.split_once(':').unwrap();
                if name.eq_ignore_ascii_case("content-length") {
                    len = value.trim().parse().unwrap();
                }
                if name.eq_ignore_ascii_case("authorization") {
                    authorization = value.trim().to_owned();
                }
            }
            assert!(len <= 65536);
            let mut bytes = vec![0; len];
            reader.read_exact(&mut bytes).await.unwrap();
            let request = Request {
                method: first.split_whitespace().next().unwrap().into(),
                path: first.split_whitespace().nth(1).unwrap().into(),
                authorization,
                body: if bytes.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                },
            };
            records.lock().unwrap().push(request.clone());
            let (status, body) = handler(request).await;
            let reply = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = reader.get_mut().write_all(reply.as_bytes()).await;
        }
    });
    Server {
        endpoint,
        requests,
        task,
    }
}
