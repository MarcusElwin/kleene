//! Locating rows in a Hugging Face parquet dataset without downloading it.
//!
//! The datasets-server `rows` endpoint serves any slice of a split as JSON,
//! untruncated, but takes an offset, not a predicate; its `filter` endpoint
//! takes a predicate but loads a split-sized index first, which for a
//! 12 GB split can take longer than a session. So the importer reads each
//! parquet shard's footer with a range request (a few hundred kilobytes
//! for a gigabyte shard), uses the row-group statistics to find the row
//! groups that can hold the wanted rows, and fetches exactly those row
//! ranges through `rows`. Datasets this is used on: `oolongbench/oolong-synth`.

use anyhow::{anyhow, Context};
use parquet::file::metadata::ParquetMetaDataReader;
use parquet::file::statistics::Statistics;
use reqwest::header::RANGE;

/// A run of rows in one parquet row group, with the statistics the
/// importer filters on.
#[derive(Debug, Clone, PartialEq)]
pub struct RowGroupSpan {
    /// Shard file name.
    pub shard: String,
    /// Offset of the group's first row within the split.
    pub offset: u64,
    /// Rows in the group.
    pub rows: u64,
    /// Min and max of the `dataset` column, if the footer has them.
    pub dataset: Option<(String, String)>,
    /// Min and max of the `context_len` column, if the footer has them.
    pub context_len: Option<(i64, i64)>,
}

impl RowGroupSpan {
    /// Whether the group can hold rows with this dataset and context length.
    pub fn may_hold(&self, dataset: &str, context_len: i64) -> bool {
        let ds = self
            .dataset
            .as_ref()
            .is_none_or(|(lo, hi)| lo.as_str() <= dataset && dataset <= hi.as_str());
        let cl = self
            .context_len
            .is_none_or(|(lo, hi)| lo <= context_len && context_len <= hi);
        ds && cl
    }
}

/// The parquet shards of a split, in the order the datasets-server reads
/// them (by name), as `(file name, size in bytes)`.
pub async fn shards(
    client: &reqwest::Client,
    repo: &str,
    split: &str,
) -> anyhow::Result<Vec<(String, u64)>> {
    let url = format!("https://huggingface.co/api/datasets/{repo}/tree/main/data");
    let listing: Vec<serde_json::Value> = client
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .with_context(|| format!("listing {url}"))?
        .json()
        .await
        .context("parsing the shard listing")?;
    let prefix = format!("data/{split}-");
    let mut out: Vec<(String, u64)> = listing
        .iter()
        .filter_map(|e| {
            let path = e.get("path")?.as_str()?;
            let size = e.get("size")?.as_u64()?;
            path.strip_prefix(&prefix)?;
            Some((path.trim_start_matches("data/").to_string(), size))
        })
        .collect();
    out.sort();
    if out.is_empty() {
        return Err(anyhow!("no parquet shards for split {split} in {repo}"));
    }
    Ok(out)
}

async fn range(
    client: &reqwest::Client,
    url: &str,
    from: u64,
    to_inclusive: u64,
) -> anyhow::Result<Vec<u8>> {
    let bytes = client
        .get(url)
        .header(RANGE, format!("bytes={from}-{to_inclusive}"))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .with_context(|| format!("range {from}-{to_inclusive} of {url}"))?
        .bytes()
        .await?;
    if bytes.len() as u64 != to_inclusive - from + 1 {
        return Err(anyhow!(
            "{url}: asked for {} bytes, got {} (range requests not honoured?)",
            to_inclusive - from + 1,
            bytes.len()
        ));
    }
    Ok(bytes.to_vec())
}

/// The row groups of one shard, from its footer alone, with `first_row`
/// the split offset of the shard's first row.
pub async fn row_groups(
    client: &reqwest::Client,
    repo: &str,
    shard: &str,
    size: u64,
    first_row: u64,
) -> anyhow::Result<Vec<RowGroupSpan>> {
    let url = format!("https://huggingface.co/datasets/{repo}/resolve/main/data/{shard}");
    if size < 12 {
        return Err(anyhow!("{shard}: too small to be a parquet file"));
    }
    let tail = range(client, &url, size - 8, size - 1).await?;
    if &tail[4..] != b"PAR1" {
        return Err(anyhow!("{shard}: not a parquet file"));
    }
    let footer_len = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as u64;
    if footer_len + 8 > size {
        return Err(anyhow!("{shard}: footer longer than the file"));
    }
    let footer = range(client, &url, size - 8 - footer_len, size - 9).await?;
    let meta = ParquetMetaDataReader::decode_metadata(&footer)
        .with_context(|| format!("{shard}: decoding the parquet footer"))?;
    let mut out = vec![];
    let mut offset = first_row;
    for rg in meta.row_groups() {
        let mut dataset = None;
        let mut context_len = None;
        for col in rg.columns() {
            let name = col.column_path().string();
            match (name.as_str(), col.statistics()) {
                ("dataset", Some(Statistics::ByteArray(s))) => {
                    if let (Some(lo), Some(hi)) = (s.min_opt(), s.max_opt()) {
                        if let (Ok(lo), Ok(hi)) = (lo.as_utf8(), hi.as_utf8()) {
                            dataset = Some((lo.to_string(), hi.to_string()));
                        }
                    }
                }
                ("context_len", Some(Statistics::Int64(s))) => {
                    if let (Some(lo), Some(hi)) = (s.min_opt(), s.max_opt()) {
                        context_len = Some((*lo, *hi));
                    }
                }
                _ => {}
            }
        }
        let rows = rg.num_rows().max(0) as u64;
        out.push(RowGroupSpan {
            shard: shard.to_string(),
            offset,
            rows,
            dataset,
            context_len,
        });
        offset += rows;
    }
    Ok(out)
}

/// `length` rows of a split from `offset`, as the datasets-server returns
/// them (each element is the row object). At most 100 per call.
pub async fn rows(
    client: &reqwest::Client,
    repo: &str,
    split: &str,
    offset: u64,
    length: u64,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut out = vec![];
    let mut at = offset;
    let end = offset + length;
    while at < end {
        let n = (end - at).min(100);
        let url = "https://datasets-server.huggingface.co/rows";
        let page: serde_json::Value = client
            .get(url)
            .query(&[
                ("dataset", repo),
                ("config", "default"),
                ("split", split),
                ("offset", &at.to_string()),
                ("length", &n.to_string()),
            ])
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .with_context(|| format!("rows {at}..{} of {repo} {split}", at + n))?
            .json()
            .await
            .context("parsing a rows page")?;
        let got = page
            .get("rows")
            .and_then(|r| r.as_array())
            .ok_or_else(|| anyhow!("rows page without rows: {page}"))?;
        if got.is_empty() {
            break;
        }
        for r in got {
            if let Some(t) = r.get("truncated_cells").and_then(|t| t.as_array()) {
                if !t.is_empty() {
                    return Err(anyhow!("row {at}: cells truncated by the server: {t:?}"));
                }
            }
            out.push(r.get("row").cloned().unwrap_or_else(|| r.clone()));
        }
        at += got.len() as u64;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_group_statistics_bound_the_search() {
        let g = RowGroupSpan {
            shard: "s".into(),
            offset: 0,
            rows: 29,
            dataset: Some(("spam".into(), "trec_coarse".into())),
            context_len: Some((16384, 65536)),
        };
        assert!(g.may_hold("trec_coarse", 32768));
        assert!(g.may_hold("spam", 65536));
        assert!(!g.may_hold("trec_coarse", 131072));
        assert!(!g.may_hold("yahoo", 32768));
        let unknown = RowGroupSpan {
            dataset: None,
            context_len: None,
            ..g
        };
        assert!(unknown.may_hold("anything", 1));
    }
}
