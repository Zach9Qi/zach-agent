//! 测试用最小 HTTP/1.1 服务：在回环地址上按脚本回应一次请求，用于验证传输层对状态码、
//! 内容类型、分块 SSE 与中途断开的处理。
//!
//! 传输层的契约只能用真实套接字观察（reqwest 没有可注入的传输替身），因此这是本测试
//! 二进制里唯一做 IO 的夹具；每个用例各起一个服务、只处理一次连接，互不干扰，也不依赖
//! 任何等待时长推断顺序。

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

/// 响应正文的发送方式。
pub(crate) enum Body {
    /// 一次性正文，带 `Content-Length`。
    Full(String),
    /// 分块传输：依次写出各片段；`complete` 为假时不发终止块直接关闭，模拟中途断开。
    Chunked { chunks: Vec<String>, complete: bool },
}

/// 一次响应的脚本。
pub(crate) struct Script {
    pub(crate) status: u16,
    pub(crate) content_type: &'static str,
    pub(crate) body: Body,
}

/// 服务端收到的请求：请求行与头（原样、小写化前）以及正文。
pub(crate) struct Captured {
    pub(crate) head: Vec<String>,
    pub(crate) body: Vec<u8>,
}

impl Captured {
    /// 按名称（不区分大小写）取请求头的值。
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.head.iter().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }
}

/// 起一个只服务一次请求的本地 HTTP 服务，返回根地址（如 `http://127.0.0.1:12345`）
/// 与收到的请求。
pub(crate) async fn serve(script: Script) -> (String, oneshot::Receiver<Captured>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定回环端口");
    let base = format!("http://{}", listener.local_addr().expect("本地地址"));
    let (sender, receiver) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("接受连接");
        let captured = read_request(&mut socket).await;
        let _ = sender.send(captured);
        write_response(&mut socket, script).await;
    });
    (base, receiver)
}

async fn read_request(socket: &mut TcpStream) -> Captured {
    let mut reader = BufReader::new(socket);
    let mut head = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("读取请求头");
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().expect("Content-Length 为数字");
        }
        head.push(line.trim_end().to_owned());
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).await.expect("读取请求正文");
    Captured { head, body }
}

async fn write_response(socket: &mut TcpStream, script: Script) {
    let mut head = format!(
        "HTTP/1.1 {} Mock\r\nContent-Type: {}\r\nX-Test: mock\r\nConnection: close\r\n",
        script.status, script.content_type
    );
    match script.body {
        Body::Full(body) => {
            head.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
            socket.write_all(head.as_bytes()).await.expect("写响应");
        }
        Body::Chunked { chunks, complete } => {
            head.push_str("Transfer-Encoding: chunked\r\n\r\n");
            socket.write_all(head.as_bytes()).await.expect("写响应头");
            for chunk in chunks {
                let framed = format!("{:x}\r\n{chunk}\r\n", chunk.len());
                socket.write_all(framed.as_bytes()).await.expect("写分块");
                socket.flush().await.expect("刷新分块");
            }
            if complete {
                socket.write_all(b"0\r\n\r\n").await.expect("写终止块");
            }
        }
    }
    socket.shutdown().await.ok();
}
