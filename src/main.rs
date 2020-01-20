use std::{ops::Range, path::PathBuf, sync::Arc};

use anyhow::{anyhow, bail, Context as _, Result};
use tokio::{fs, prelude::*, task};
use futures_util::{future::try_join_all, StreamExt};

static USER_AGENT: &str = concat!("zrglng/", env!("CARGO_PKG_VERSION"));

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
    client: Arc<reqwest::Client>,
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

        let res = ctx
            .client
            .get(self.url.as_str())
            .header(reqwest::header::RANGE, range_header.as_str())
            .send()
            .await
            .with_context(|| {
                format!(
                    "failed partial get #{} for range: {:?}",
                    self.idx, self.range
                )
            })?;

        if !res.status().is_success() {
            bail!("invalid status code: {}", res.status());
        }

        log::debug!("creating file");
        let mut file = fs::File::create(PathBuf::from(&self.dest))
            .await
            .with_context(|| format!("failed to create tmp file at {}", &self.dest.display()))?;
        log::debug!("copying chunks from req to file");

        let mut bytes_stream = res.bytes_stream();
        let mut count = 0;

        while let Some(bytes) = bytes_stream.next().await {
            let bytes =
                bytes.with_context(|| format!("failed to read from body at byte {}", count))?;
            count += bytes.len();
            log::info!(
                "part {}: {}/{} bytes",
                self.idx,
                count,
                self.range.end - self.range.start
            );

            file.write_all(&bytes)
                .await
                .with_context(|| format!("failed to write to file at byte {}", count))?;
        }

        log::debug!("finished downloading part {}", self.idx);
        Ok((self.idx, self.dest))
    }
}

async fn file_info(client: &reqwest::Client, url: &str) -> Result<FileInfo> {
    let res = client.head(url).send().await?;

    if !res.status().is_success() {
        bail!("invalid status code");
    }
    let content_length = res
        .content_length()
        .ok_or_else(|| anyhow!("no content type"))?;
    let supports_range = res
        .headers()
        .get(reqwest::header::ACCEPT_RANGES)
        .map(|v| v == "bytes")
        .unwrap_or(false);
    Ok(FileInfo {
        content_length,
        supports_range,
    })
}

async fn parallel_get(url: &str, dest: PathBuf, parts: u64) -> Result<()> {
    // TODO: assert that dest is a file
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .use_native_tls()
        .build()?;

    let file_info = file_info(&client, url)
        .await
        .context("failed to do HEAD request")?;
    log::info!(
        "file size: {}, supports range: {}",
        &file_info.content_length,
        &file_info.supports_range
    );
    let parts = if file_info.supports_range { parts } else { 1 };
    let ranges = get_ranges(file_info.content_length, parts);
    log::trace!("ranges: {:?}", ranges.clone().collect::<Vec<_>>());

    let client = Arc::new(client);
    let ctx = Context { client };

    let partial_reqs = ranges.map(|range| {
        let partial_getter = PartialGetter::new(range, url, &dest);
        let ctx = ctx.clone();
        task::spawn(async move { partial_getter.get(ctx).await })
    });

    let files = try_join_all(partial_reqs)
        .await
        .context("one of the parts failed to download")?;
    let mut out_file = fs::File::create(&dest).await?;
    let mut files = files.into_iter().collect::<Result<Vec<_>>>()?;
    files.sort_unstable_by_key(|&(idx, _)| idx);
    for (_, path) in files {
        let mut file = fs::File::open(&path).await?;
        tokio::io::copy(&mut file, &mut out_file).await?;
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
    let mut runtime = tokio::runtime::Runtime::new().expect("could not create tokio runtime");
    runtime.block_on(async {
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
