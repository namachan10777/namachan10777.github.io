use super::convert::Context;
use super::{Cmd, Location, Parsed, TextElemAst};
use super::{Error, Value};
use chrono::{DateTime, FixedOffset, Utc, TimeZone, NaiveDate};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use syntect::parsing::SyntaxSet;

type ArticleHeading = (PathBuf, Vec<TextElemAst>);

type ArticleInfo = (
    Location,
    Option<ArticleHeading>,
    Option<ArticleHeading>,
    String,
);

pub struct Report {
    per_article: HashMap<PathBuf, ArticleInfo>,
    titles: HashMap<PathBuf, Vec<(PathBuf, Vec<TextElemAst>)>>,
    ss: SyntaxSet,
}

impl Report {
    pub fn get_context<'a>(&'a self, p: &'a Path) -> Option<Context<'a>> {
        if let Some((loc, prev, next, sha256)) = &self.per_article.get(p) {
            Some(Context {
                location: loc.to_owned(),
                level: 1,
                prev,
                next,
                titles: &self.titles,
                ss: &self.ss,
                sha256,
                path: p,
            })
        } else {
            None
        }
    }
}

fn extract_title(cmd: &(Cmd, Location)) -> Result<Vec<TextElemAst>, Error> {
    let (cmd, loc) = cmd;
    Ok(crate::value_utils::get_text(&cmd.attrs, "title", loc)?.to_vec())
}

fn extract_date(cmd: &(Cmd, Location)) -> Result<DateTime<FixedOffset>, Error> {
    if cmd.0.name == "article" {
        if let Some((data_val, loc)) = cmd.0.attrs.get("date") {
            if let Value::Str(date_str) = data_val {
                let utc = NaiveDate::parse_from_str(date_str, "%Y-%m-%d").map_err(|_| Error::ProcessError {
                    loc: loc.to_owned(),
                    desc: "invalid date format".to_owned(),
                })?.and_hms(0, 0, 0);
                Ok(DateTime::from_utc(utc, TimeZone::from_offset(&FixedOffset::east(9 * 3600))))
            } else {
                Err(Error::InvalidAttributeType {
                    name: "date".to_owned(),
                    loc: loc.to_owned(),
                    expected: crate::ValueType::Str,
                    found: data_val.value_type(),
                })
            }
        } else {
            Err(Error::MissingAttribute {
                name: "date".to_owned(),
                loc: cmd.1.to_owned(),
            })
        }
    }
    else {
        Ok(DateTime::parse_from_rfc3339("1970-01-01T00:00:00Z").unwrap())
    }
}

type Titles = HashMap<PathBuf, Vec<(PathBuf, Vec<TextElemAst>)>>;

fn calc_sorted_titles(parsed: &Parsed) -> Result<Titles, Error> {
    let mut ret = HashMap::new();
    for (p, f) in parsed {
        if let super::File::Tml(cmd, _) = f {
            let title = extract_title(cmd)?;
            let date = extract_date(cmd)?;
            ret.entry(p.parent().unwrap())
                .or_insert_with(Vec::new)
                .push((p.to_owned(), date, title));
        }
    }
    Ok(ret
        .into_iter()
        .map(|(p, mut titles)| {
            titles.sort_by(|(_, a, _), (_, b, _)| a.cmp(&b));
            (
                p.to_owned(),
                titles.into_iter().map(|(p, _, title)| (p, title)).collect(),
            )
        })
        .collect())
}

type Prevs = HashMap<PathBuf, (PathBuf, Vec<TextElemAst>)>;
type Nexts = HashMap<PathBuf, (PathBuf, Vec<TextElemAst>)>;

fn prevs_and_nexts(parsed: &Parsed) -> Result<(Prevs, Nexts, Titles), Error> {
    let mut prevs = HashMap::new();
    let mut nexts = HashMap::new();
    let titles = calc_sorted_titles(parsed)?;
    for titles in titles.values() {
        let mut prev: Option<(&Path, &Vec<TextElemAst>)> = None;
        for (path, title) in titles {
            if let Some((prev_path, prev_title)) = prev {
                prevs.insert(prev_path.to_owned(), (path.to_owned(), title.clone()));
                nexts.insert(
                    path.to_owned(),
                    (prev_path.to_owned(), prev_title.to_owned()),
                );
            }
            prev = Some((path, title));
        }
    }
    Ok((prevs, nexts, titles))
}

fn calc_sha256(path: &Path, src: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(src);
    hasher.update(path.as_os_str().to_string_lossy().as_bytes());
    hex::encode(hasher.finalize().as_slice())
}

pub fn analyze(parsed: &Parsed) -> Result<Report, Error> {
    let (prevs, nexts, titles) = prevs_and_nexts(parsed)?;
    let mut per_article = HashMap::new();
    for (path, file) in parsed {
        if let super::File::Tml(cmd, src) = file {
            per_article.insert(
                path.to_owned(),
                (
                    cmd.1.to_owned(),
                    prevs.get(path).cloned(),
                    nexts.get(path).cloned(),
                    calc_sha256(path, src),
                ),
            );
        }
    }
    Ok(Report {
        per_article,
        ss: SyntaxSet::load_defaults_nonewlines(),
        titles,
    })
}
