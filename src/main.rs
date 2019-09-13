mod error;

use async_std::{fs, task};
use std::{ops::Range, path::PathBuf, sync::Arc};

use futures_util::try_future::try_join_all;

static USER_AGENT: &str = concat!("zrglng/", env!("CARGO_PKG_VERSION"));

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone)]
struct FileInfo {
    content_length: u64,
    supports_range: bool,
}

#[derive(Debug, Clone)]
struct RangeQuery {
    range: Range<u64>,
    idx: u64,
}

#[derive(Debug, Clone)]
struct Context {
    client: Arc<isahc::HttpClient>,
}

#[derive(Debug, Clone)]
struct PartialGetter {
    range: Range<u64>,
    idx: u64,
    url: String,
    dest: PathBuf,
}

impl PartialGetter {
    fn new(range: RangeQuery, url: &str, dest: &PathBuf) -> Self {
        let RangeQuery { range, idx } = range;
        let dest = {
            let mut dest = PathBuf::from(dest);
            let mut filename = std::ffi::OsString::from(".");
            let final_filename = dest
                .file_name()
                .expect("was supposed to be a filename")
                .to_owned();
            filename.push(final_filename);
            filename.push(format!(".part-{}", idx));
            log::info!("downloading part {} at {:?}", idx, &filename);
            dest.set_file_name(&filename);
            dest
        };
        Self {
            range,
            idx,
            url: url.to_owned(),
            dest,
        }
    }

    async fn get(self, ctx: Context) -> Result<(u64, PathBuf)> {
        let range_header = format!("bytes={}-{}", self.range.start, self.range.end - 1);

        let req = http::Request::get(self.url.as_str())
            .header(http::header::USER_AGENT, USER_AGENT)
            .header(http::header::RANGE, range_header.as_str())
            .body(())?;

        let mut res = ctx.client.send_async(req).await.map_err(|e| {
            err_of!(
                e,
                "failed partial get #{} for range: {:?}",
                self.idx,
                self.range
            )
        })?;

        if !res.status().is_success() {
            bail!("invalid status code: {}", res.status());
        }

        log::debug!("creating file");
        let mut file = fs::File::create(PathBuf::from(&self.dest))
            .await
            .map_err(|e| err_of!(e, "failed to create tmp file at {}", &self.dest.display()))?;
        log::debug!("copying chunks from req to file");
        futures_util::io::AsyncReadExt::copy_into(res.body_mut(), &mut file)
            .await
            .map_err(|e| err_of!(e, "failed to write file"))?;

        log::debug!("finished downloading part {}", self.idx);
        Ok((self.idx, self.dest))
    }
}

async fn file_info(client: &isahc::HttpClient, url: &str) -> Result<FileInfo> {
    let req = http::Request::head(url)
        .header(http::header::USER_AGENT, USER_AGENT)
        .body(())?;
    let res = client.send_async(req).await?;

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

async fn parallel_get(url: &str, dest: PathBuf, parts: u64) -> Result<()> {
    // TODO: assert that dest is a file
    let client = isahc::HttpClient::builder()
        .redirect_policy(isahc::config::RedirectPolicy::Follow)
        .tcp_keepalive(std::time::Duration::from_secs(1))
        .build()?;

    let file_info = file_info(&client, url)
        .await
        .map_err(|e| err_of!(e, "failed to do HEAD req"))?;
    log::info!(
        "file size: {}, supports range: {}",
        &file_info.content_length,
        &file_info.supports_range
    );
    let parts = if file_info.supports_range { parts } else { 1 };
    let ranges = get_ranges(file_info.content_length, parts);
    log::trace!("ranges: {:?}", ranges.clone().collect::<Vec<_>>());

    let client = Arc::new(client);
    let ctx = Context { client, };

    let partial_reqs = ranges.map(|range| {
        let partial_getter = PartialGetter::new(range, url, &dest);
        let ctx = ctx.clone();
        task::spawn(async move { partial_getter.get(ctx).await })
    });

    let mut files = try_join_all(partial_reqs)
        .await
        .map_err(|e| err_of!(e, "one of the parts failed to download"))?;
    let mut out_file = fs::File::create(&dest).await?;
    files.sort_unstable_by_key(|&(idx, _)| idx);
    for (_, path) in files {
        let file = fs::File::open(&path).await?;
        futures_util::io::AsyncReadExt::copy_into(file, &mut out_file).await?;
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
        if last_segment == "" {
            PathBuf::from("index.html")
        } else {
            PathBuf::from(last_segment)
        }
    } else {
        PathBuf::from("index.html")
    }
}

async fn run(args: Args) -> Result<()> {
    let url = url::Url::parse(args.url.as_str())?;
    let dest = args.output.unwrap_or_else(|| dest_from_url(&url));
    parallel_get(args.url.as_str(), dest, args.parts).await?;
    Ok(())
}

#[derive(structopt::StructOpt, Debug)]
struct Args {
    #[structopt(long, short, default_value = "4")]
    parts: u64,
    #[structopt(long, short, parse(from_os_str))]
    output: Option<PathBuf>,
    url: String,
}

fn main() {
    pretty_env_logger::init_timed();
    let args = <Args as structopt::StructOpt>::from_args();
    task::block_on(async {
        if let Err(err) = run(args).await {
            eprintln!("Error: {}", err);
            let sources = std::iter::successors(err.source(), |err| err.source());
            for source in sources {
                eprintln!("  caused by: {}", source);
            }
            std::process::exit(1);
        }
    })
}
