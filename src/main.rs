#![feature(async_await)]

use hyper::{Client, header};


type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone)]
struct FileInfo {
    content_length: u32,
    supports_range: bool,
}

async fn file_info<C>(client: &Client<C>, url: &str) -> Result<FileInfo>
where
    C: hyper::client::connect::Connect + Sync + 'static,
    C::Transport: 'static,
    C::Future: 'static
{

    let req = hyper::Request::head(url)
        .header("User-Agent", "Paraget/0.1.0")
        .body(hyper::Body::empty())?;
    let res = client.request(req).await?;
    if !res.status().is_success() {
        return Err(ErrMsg::new("invalid status code").into());
    }
    let content_length = res.headers()
        .get(header::CONTENT_LENGTH)
        .ok_or_else(|| ErrMsg::new("no content type"))?
        .to_str()?
        .parse::<u32>()?;
    let supports_range = res.headers()
        .get(header::ACCEPT_RANGES)
        .map(|v| v == "bytes")
        .unwrap_or(false);
    Ok(FileInfo {
        content_length,
        supports_range,
    })
}

async fn parallel_get<C>(client: &Client<C>, url: &str) -> Result<()>
where
    C: hyper::client::connect::Connect + Sync + 'static,
    C::Transport: 'static,
    C::Future: 'static
{
    let file_info = file_info(client, url).await?;
    dbg!(file_info);
    Ok(())
}

async fn run() -> Result<()> {
    let target_url = "https://i.redd.it/f61r13m3k2931.jpg";
    match dbg!(url::Url::parse(target_url)?.scheme()) {
        "https" => {
            let https = hyper_tls::HttpsConnector::new(1)?;
            let client = Client::builder().build(https);
            parallel_get(&client, target_url).await?;
        },
        "http" => {
            let http = hyper::client::HttpConnector::new(1);
            let client = Client::builder().build(http);
            parallel_get(&client, target_url).await?;
        },
        s => {
            return Err(ErrMsg::new(format!("unsuported scheme: {}", s)).into());
        }
    };
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("Error: {}", err);
        std::process::exit(1);
    }
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
