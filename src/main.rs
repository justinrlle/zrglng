#![feature(async_await)]

use hyper::{Client, header};


type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone)]
struct FileInfo {
    content_length: u32,
    supports_range: bool,
}

async fn file_info(client: &Client, url: &str) -> Result<FileInfo> {
    let req = hyper::Request::head(url)
        .header("User-Agent", "Paraget/0.1.0")
        .body(())?;
    let res = client.request(req).await?;
    if !res.status().is_success() {
        return Err(ErrMsg::new("invalid status code").into());
    }
    let content_length = res.headers()
        .get(header::CONTENT_TYPE)
        .ok_or_else(|| ErrMsg::new("no content type"))?
        .parse::<u32>()?;
    let supports_range = res.headers()
        .get(header::ACCEPT_RANGE)
        .map(|v| v == "bytes")
        .unwrap_or(false);
    Ok(FileInfo {
        content_length,
        supports_range,
    })
}


fn main() {
    println!("Hello, world!");
}

#[derive(Debug, Clone)]
struct ErrMsg(String);

impl ErrMsg {
    fn new<T: std::convert::Into<String>>(msg: T) -> Self {
        Self(msg.into())
    }
}

impl std::fmt::Display for ErrMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ErrMsg {}
