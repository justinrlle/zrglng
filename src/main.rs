#![feature(async_await)]

mod error;

use std::{
    path::Path,
    ops::Range,
    fs::File,
    io::Write as _,
};

use error::ErrMsg;

use hyper::{header, Client};
use futures_util::{TryStreamExt, future::join_all};

static USER_AGENT: &str = concat!("Paraget/", env!("CARGO_PKG_VERSION"));

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type HttpsClient = hyper::Client<hyper_tls::HttpsConnector<hyper::client::HttpConnector>>;

#[derive(Debug, Clone)]
struct FileInfo {
    content_length: u64,
    supports_range: bool,
}

#[derive(Debug, Clone)]
struct Options<'a> {
    parts: u64,
    url: &'a str,
    dest: &'a Path,
}

#[derive(Debug, Clone)]
struct RangeQuery {
    range: Range<u64>,
    idx: u64,
}

async fn file_info(client: &HttpsClient, url: &str) -> Result<FileInfo> {
    let req = hyper::Request::head(url)
        .header(header::USER_AGENT, USER_AGENT)
        .body(hyper::Body::empty())?;
    let res = client.request(req).await?;
    if !res.status().is_success() {
        return Err(ErrMsg::new("invalid status code").into());
    }
    let content_length = res
        .headers()
        .get(header::CONTENT_LENGTH)
        .ok_or_else(|| ErrMsg::new("no content type"))?
        .to_str()?
        .parse::<u64>()?;
    let supports_range = res
        .headers()
        .get(header::ACCEPT_RANGES)
        .map(|v| v == "bytes")
        .unwrap_or(false);
    Ok(FileInfo {
        content_length,
        supports_range,
    })
}

async fn partial_get(client: &HttpsClient, url: &str, _dest: &Path, range: RangeQuery) -> Result<(u64, bytes::Bytes)> {
    let range_header = format!("bytes={}-{}", range.range.start, range.range.end - 1);
    let range_header: &str = range_header.as_ref();
    let req = hyper::Request::get(url)
        .header(header::USER_AGENT, USER_AGENT)
        .header(header::RANGE, range_header)
        .body(hyper::Body::empty())?;
    let res = client.request(req).await?;
    if !res.status().is_success() {
        return Err(ErrMsg::new("invalid status code").into());
    }
    Ok((range.idx, res.into_body().try_concat().await?.into_bytes()))
    
}

async fn parallel_get(client: &HttpsClient, opts: &Options<'_>) -> Result<()> {
    let file_info = file_info(client, opts.url).await?;
    dbg!(&file_info);
    let parts = if file_info.supports_range { opts.parts } else { 1 };
    let ranges = get_ranges(file_info.content_length, parts);
    dbg!(ranges.clone().collect::<Vec<_>>());

    let partial_reqs = ranges
        .map(|range| partial_get(client, opts.url, opts.dest, range));

    let bodies = join_all(partial_reqs).await;
    let bodies = bodies.into_iter().collect::<Result<Vec<_>>>()?;
    let mut out_file = File::create(opts.dest)?;
    for (idx, body) in bodies.iter() {
        dbg!(idx, body.len());
        out_file.write_all(body.as_ref())?;
    }
    Ok(())
}

fn get_ranges(content_length: u64, parts: u64) -> impl Iterator<Item=RangeQuery> + Clone{
    let part_len = content_length / parts;
    
    (0..parts).map(move |idx| {
        let start = idx * part_len;
        let end = if idx + 1 < parts {
            (idx + 1) * part_len
        } else {
            content_length
        };
        RangeQuery {
            range: (start..end),
            idx,
        }
    })
}

fn dest_from_url(url: &url::Url) -> &std::path::Path {
    if let Some(segments) = url.path_segments() {
        let last_segment = segments.last().unwrap_or("index.html");
        std::path::Path::new(last_segment)
    } else {
        std::path::Path::new("index.html")
    }
}

async fn run() -> Result<()> {
    let target_url = "http://i.redd.it/f61r13m3k2931.jpg";
    let url = url::Url::parse(target_url)?;
    let opts = Options {
        parts: 4,
        url: target_url,
        dest: dest_from_url(&url),
    };
    let https = hyper_tls::HttpsConnector::new(4)?;
    let client = Client::builder().build(https);
    parallel_get(&client, &opts).await?;
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("Error: {}", err);
        std::process::exit(1);
    }
}
