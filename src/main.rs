mod error;

use async_std::{fs, task};
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use futures::future::try_join_all;

static USER_AGENT: &str = concat!("zrglng/", env!("CARGO_PKG_VERSION"));

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone)]
struct FileInfo {
    content_length: u64,
    supports_range: bool,
}

#[derive(Debug, Clone)]
struct Options {
    parts: u64,
    url: String,
    dest: PathBuf,
}

#[derive(Debug, Clone)]
struct RangeQuery {
    range: Range<u64>,
    idx: u64,
}

async fn file_info(url: &str) -> Result<FileInfo> {
    let req = http::Request::head(url)
        .header(http::header::USER_AGENT, USER_AGENT)
        .body(())?;
    let res = isahc::send_async(req).await?;

    if !res.status().is_success() {
        bail!("invalid status code");
    }
    let content_length = res
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| err!("no content type"))?
        .parse::<u64>()?;
    let supports_range = res
        .headers()
        .get(http::header::ACCEPT_RANGES)
        .map(|v| v == "bytes")
        .unwrap_or(false);
    Ok(FileInfo {
        content_length,
        supports_range,
    })
}

async fn partial_get(url: &str, dest: &Path, range: RangeQuery) -> Result<(u64, PathBuf)> {
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
    log::info!("downloading part {} at {:?}", range.idx, &filename);
    tmp_path.set_file_name(&filename);
    let range_header = format!("bytes={}-{}", range.range.start, range.range.end - 1);
    let range_header: &str = range_header.as_ref();

    let req = http::Request::get(url)
        .header(http::header::USER_AGENT, USER_AGENT)
        .header(http::header::RANGE, range_header)
        .body(())?;

    let mut res = isahc::send_async(req)
        .await
        .map_err(|e| err_of!(e, "failed partial get for range: {:?}", range.range))?;

    if !res.status().is_success() {
        bail!("invalid status code");
    }
    log::debug!("creating file");
    let mut file = fs::File::create(PathBuf::from(&tmp_path))
        .await
        .map_err(|e| err_of!(e, "failed to create tmp file at {}", &tmp_path.display()))?;
    log::debug!("copying chunks from req to file");
    futures::io::AsyncReadExt::copy_into(res.body_mut(), &mut file)
        .await
        .map_err(|e| err_of!(e, "failed to write file"))?;

    log::debug!("finished downloading part {}", range.idx);
    Ok((range.idx, tmp_path))
}

async fn parallel_get(opts: &Options) -> Result<()> {
    let file_info = file_info(opts.url.as_str())
        .await
        .map_err(|e| err_of!(e, "failed to do HEAD req"))?;
    log::info!("file size: {}, supports range: {}", &file_info.content_length, &file_info.supports_range);
    let parts = if file_info.supports_range {
        opts.parts
    } else {
        1
    };
    let ranges = get_ranges(file_info.content_length, parts);
    log::trace!("ranges: {:?}", ranges.clone().collect::<Vec<_>>());

    let partial_reqs = ranges.map(|range| {
        let opts: Options = opts.clone();
        task::spawn(async move { partial_get(opts.url.as_str(), &opts.dest, range).await })
    });

    let mut files = try_join_all(partial_reqs)
        .await
        .map_err(|e| err_of!(e, "failed to download a part"))?;
    let mut out_file = fs::File::create(&opts.dest).await?;
    files.sort_unstable_by_key(|&(idx, _)| idx);
    for (_, path) in files {
        let file = fs::File::open(&path).await?;
        futures::io::AsyncReadExt::copy_into(file, &mut out_file).await?;
        fs::remove_file(&path).await?;
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

fn dest_from_url(url: &url::Url) -> PathBuf {
    if let Some(segments) = url.path_segments() {
        let last_segment = segments.last().unwrap_or("index.html");
        PathBuf::from(last_segment)
    } else {
        PathBuf::from("index.html")
    }
}

async fn run() -> Result<()> {
    let target_url = "http://i.redd.it/f61r13m3k2931.jpg";
    let url = url::Url::parse(target_url)?;
    let opts = Options {
        parts: 2,
        url: target_url.to_owned(),
        dest: dest_from_url(&url),
    };
    parallel_get(&opts).await?;
    Ok(())
}

fn main() {
    pretty_env_logger::init_timed();
    task::block_on(async {
        if let Err(err) = run().await {
            eprintln!("Error: {}", err);
            let sources = std::iter::successors(err.source(), |err| err.source());
            for source in sources {
                eprintln!("  caused by: {}", source);
            }
            std::process::exit(1);
        }
    })
}
