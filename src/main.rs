#![feature(async_await, error_iter)]

mod error;

use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use futures_util::{future::join_all, StreamExt};
use hyper::header;

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
        bail!("invalid status code");
    }
    let content_length = res
        .headers()
        .get(header::CONTENT_LENGTH)
        .ok_or_else(|| err!("no content type"))?
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

async fn partial_get(
    client: &HttpsClient,
    url: &str,
    dest: &Path,
    range: RangeQuery,
) -> Result<(u64, PathBuf)> {
    let mut tmp_path = PathBuf::from(dest);
    let filename = {
        let mut filename = std::ffi::OsString::from(".");
        let final_filename = tmp_path
            .file_name()
            .ok_or_else(|| err!("expected a file, got a directory"))?
            .to_owned();
        filename.push(final_filename);
        filename.push(format!(".part-{}", range.idx));
        filename
    };
    dbg!(&filename, range.idx);
    tmp_path.set_file_name(&filename);
    let range_header = format!("bytes={}-{}", range.range.start, range.range.end - 1);
    let range_header: &str = range_header.as_ref();
    let req = hyper::Request::get(url)
        .header(header::USER_AGENT, USER_AGENT)
        .header(header::RANGE, range_header)
        .body(hyper::Body::empty())?;
    let res = client.request(req).await.map_err(|e| {
        err_of!(
            e,
            "failed to do patial get of following bytes: {:?}",
            range.range
        )
    })?;
    if !res.status().is_success() {
        bail!("invalid status code");
    }
    eprintln!("creating file");
    let file = tokio::fs::File::create(PathBuf::from(&tmp_path))
        .await
        .map_err(|e| err_of!(e, "failed to create tmp file at {}", &tmp_path.display()))?;
    eprintln!("copying chunks from req to file");
    let file = res
        .into_body()
        .fold(Ok(file), |try_file, chunk| write_chunk(chunk, try_file))
        .await
        .map_err(|e| err_of!(e, "failed to write file"))?;
    eprintln!("finished downloading part {}", range.idx);
    Ok((range.idx, tmp_path))
}

async fn write_chunk(
    chunk: hyper::Result<hyper::Chunk>,
    try_file: Result<tokio::fs::File>,
) -> Result<tokio::fs::File> {
    if let Ok(mut file) = try_file {
        let bytes = chunk?.into_bytes();
        dbg!(bytes.len());
        use tokio::io::AsyncWriteExt;
        file.write_all(&bytes)
            .await
            .map_err(|e| err_of!(e, "failed to write a chunk"))?;
        Ok(file)
    } else {
        try_file
    }
}

async fn parallel_get(client: &HttpsClient, opts: &Options<'_>) -> Result<()> {
    let file_info = file_info(client, opts.url)
        .await
        .map_err(|e| err_of!(e, "failed to do HEAD req"))?;
    dbg!(&file_info);
    let parts = if file_info.supports_range {
        opts.parts
    } else {
        1
    };
    let ranges = get_ranges(file_info.content_length, parts);
    dbg!(ranges.clone().collect::<Vec<_>>());

    let partial_reqs = ranges.map(|range| partial_get(client, opts.url, opts.dest, range));

    let paths = join_all(partial_reqs).await;
    let paths = paths.into_iter().collect::<Result<Vec<_>>>()?;
    //let mut out_file = tokio::fs::File::create(opts.dest).await?;
    for (idx, path) in paths.iter() {
        dbg!(idx, path.display());
        // out_file.write_all(body.as_ref())?;
    }
    Ok(())
}

fn get_ranges(content_length: u64, parts: u64) -> impl Iterator<Item = RangeQuery> + Clone {
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
        parts: 1,
        url: target_url,
        dest: dest_from_url(&url),
    };
    let https = hyper_tls::HttpsConnector::new(4)?;
    let client = hyper::Client::builder().build(https);
    parallel_get(&client, &opts).await?;
    Ok(())
}

fn main() {
    env_logger::init();
    tokio_main();
}

#[tokio::main]
async fn tokio_main() {
    if let Err(err) = run().await {
        eprintln!("Error: {}", err);
        let sources = std::iter::successors(err.source(), |err| err.source());
        for source in sources {
            eprintln!("  caused by: {}", source);
        }
        std::process::exit(1);
    }
}
