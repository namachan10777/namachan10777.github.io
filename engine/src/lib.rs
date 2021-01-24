#[macro_use]
extern crate pest_derive;
extern crate sha2;
extern crate syntect;
extern crate zip;

#[macro_use]
pub mod xml;
pub mod analysis;
pub mod convert;
pub mod ogp_renderer;
pub mod parser;

use log::info;
use std::cmp;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::io::{Seek, Write};
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Clone, Eq)]
pub struct Position {
    fname: String,
    // 1-indexed
    line: usize,
    // 1-indexed
    col: usize,
}

#[derive(Debug)]
pub enum Error {
    SyntaxError(Location),
    MissingAttribute {
        name: String,
        loc: Location,
    },
    InvalidAttributeType {
        name: String,
        expected: ValueType,
        found: ValueType,
        loc: Location,
    },
    NoSuchCmd {
        name: String,
        loc: Location,
    },
    ProcessError {
        loc: Location,
        desc: String,
    },
    FsError {
        path: PathBuf,
        desc: String,
        because: io::Error,
    },
    ImageFmtError {
        path: PathBuf,
        desc: String,
        because: image::ImageError,
    },
    CannotInterpretPathAsUTF8(PathBuf),
    ZipError {
        desc: String,
        because: zip::result::ZipError,
    },
    ZipIOError {
        desc: String,
        because: io::Error,
    },
}

impl Position {
    pub fn new(fname: &str, line: usize, col: usize) -> Position {
        Self {
            fname: fname.to_owned(),
            line,
            col,
        }
    }
}

impl PartialOrd for Position {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        if self.fname != other.fname {
            self.fname.partial_cmp(&other.fname)
        } else if self.line != other.line {
            self.line.partial_cmp(&other.line)
        } else {
            self.col.partial_cmp(&other.col)
        }
    }
}

impl Ord for Position {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        if self.fname != other.fname {
            self.fname.cmp(&other.fname)
        } else if self.line != other.line {
            self.line.cmp(&other.line)
        } else {
            self.col.cmp(&other.col)
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum Location {
    Span(Position, Position),
    At(Position),
    Generated,
}

impl Location {
    pub fn merge(&self, other: &Self) -> Self {
        match (self, other) {
            (Location::Span(a1, a2), Location::Span(b1, b2)) => {
                Location::Span(cmp::min(a1, b1).clone(), cmp::max(a2, b2).clone())
            }
            (Location::At(a), Location::At(b)) => {
                Location::Span(cmp::min(a, b).clone(), cmp::max(a, b).clone())
            }
            (Location::Span(a1, a2), Location::At(b)) => {
                Location::Span(cmp::min(a1, b).clone(), cmp::max(a2, b).clone())
            }
            (Location::At(a), Location::Span(b1, b2)) => {
                Location::Span(cmp::min(a, b1).clone(), cmp::max(a, b2).clone())
            }
            (loc, Location::Generated) => loc.clone(),
            (Location::Generated, loc) => loc.clone(),
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Location::Generated => write!(f, "nowhere:nowhere:nowhere"),
            Location::At(p) => write!(f, "{}:{}:{}", p.fname, p.line, p.col),
            Location::Span(p1, p2) => {
                if p1.fname == p2.fname {
                    write!(
                        f,
                        "{}:{}:{} -- {}:{}",
                        p1.fname, p1.line, p1.col, p2.line, p2.col
                    )
                } else {
                    write!(
                        f,
                        "{}:{}:{} -- {}:{}:{}",
                        p1.fname, p1.line, p1.col, p2.fname, p2.line, p2.col
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod test_loc {
    use super::*;
    #[test]
    fn test_pos() {
        assert!(Position::new("", 1, 1) == Position::new("", 1, 1));
        assert!(Position::new("", 1, 2) > Position::new("", 1, 1));
    }

    #[test]
    fn test_merge() {
        let p1 = Position::new("", 1, 1);
        let p2 = Position::new("", 2, 1);
        let p3 = Position::new("", 3, 1);
        let p4 = Position::new("", 4, 1);
        let s1 = Location::Span(p1.clone(), p2.clone());
        let s2 = Location::Span(p2.clone(), p3.clone());
        assert_eq!(
            s1.merge(&Location::At(p3.clone())),
            Location::Span(p1.clone(), p3.clone())
        );
        assert_eq!(s1.merge(&s2), Location::Span(p1.clone(), p3.clone()));
        assert_eq!(
            Location::At(p4.clone()).merge(&Location::At(p2.clone())),
            Location::Span(p2.clone(), p4.clone())
        );
    }
}
#[derive(PartialEq, Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Text(Vec<TextElemAst>),
    List(Vec<ValueAst>),
}

#[derive(Debug, PartialEq, Hash, Eq, Clone)]
pub enum ValueType {
    Any,
    Int,
    Float,
    Str,
    Text,
    ListOf(Box<ValueType>),
}

impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            ValueType::Any => write!(f, "any"),
            ValueType::Int => write!(f, "int"),
            ValueType::Float => write!(f, "float"),
            ValueType::Str => write!(f, "string"),
            ValueType::Text => write!(f, "text"),
            ValueType::ListOf(element_type) => write!(f, "{} list", element_type),
        }
    }
}

impl ValueType {
    pub fn is_list(&self) -> bool {
        matches!(self, ValueType::ListOf(_))
    }
}

impl Value {
    pub fn str(&self) -> Option<&str> {
        if let Value::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }

    pub fn int(&self) -> Option<i64> {
        if let Value::Int(i) = self {
            Some(*i)
        } else {
            None
        }
    }

    pub fn text(&self) -> Option<&[TextElemAst]> {
        if let Value::Text(t) = self {
            Some(t)
        } else {
            None
        }
    }

    pub fn float(&self) -> Option<f64> {
        if let Value::Float(f) = self {
            Some(*f)
        } else {
            None
        }
    }

    pub fn list(&self) -> Option<&[ValueAst]> {
        if let Value::List(l) = self {
            Some(l)
        } else {
            None
        }
    }

    pub fn is_instanceof(&self, typ: &ValueType) -> bool {
        match (self, typ) {
            (_, ValueType::Any) => true,
            (Value::Int(_), ValueType::Int) => true,
            (Value::Float(_), ValueType::Float) => true,
            (Value::Str(_), ValueType::Str) => true,
            (Value::Text(_), ValueType::Text) => true,
            (Value::List(elems), ValueType::ListOf(elem_type)) => {
                elems.iter().all(|(elem, _)| elem.is_instanceof(&elem_type))
            }
            _ => false,
        }
    }

    fn weak_type(&self) -> ValueType {
        match self {
            Value::Int(_) => ValueType::Int,
            Value::Float(_) => ValueType::Float,
            Value::Str(_) => ValueType::Str,
            Value::Text(_) => ValueType::Text,
            Value::List(_) => ValueType::ListOf(Box::new(ValueType::Any)),
        }
    }

    fn merge_list_types(
        a: Vec<HashSet<ValueType>>,
        b: Vec<HashSet<ValueType>>,
    ) -> Vec<HashSet<ValueType>> {
        let mut dist = Vec::new();
        let mut a_iter = a.into_iter();
        let mut b_iter = b.into_iter();
        loop {
            match (a_iter.next(), b_iter.next()) {
                (Some(mut hash_a), Some(hash_b)) => {
                    hash_a.extend(hash_b.into_iter());
                    dist.push(hash_a);
                }
                (Some(hash_a), None) => {
                    dist.push(hash_a);
                    dist.append(&mut a_iter.collect());
                    break;
                }
                (None, Some(hash_b)) => {
                    dist.push(hash_b);
                    dist.append(&mut b_iter.collect());
                    break;
                }
                (None, None) => {
                    break;
                }
            }
        }
        dist
    }

    fn flat_list_types(inner: &[(Value, Location)]) -> Vec<HashSet<ValueType>> {
        let mut head = HashSet::new();
        let mut last = Vec::new();
        for (elem, _) in inner {
            head.insert(elem.weak_type());
            if let Some(children) = elem.list() {
                last = Self::merge_list_types(last, Self::flat_list_types(children));
            }
        }
        let mut head = vec![head];
        head.append(&mut last);
        head
    }

    fn gen_nested_list_type(nest_level: usize, leaf_type: ValueType) -> ValueType {
        if nest_level == 0 {
            leaf_type
        } else {
            ValueType::ListOf(Box::new(Self::gen_nested_list_type(
                nest_level - 1,
                leaf_type,
            )))
        }
    }

    fn unification(list_types: Vec<HashSet<ValueType>>) -> ValueType {
        for (i, types) in list_types.iter().enumerate() {
            // unification中止
            if types.len() > 1 {
                return Self::gen_nested_list_type(i, ValueType::Any);
            }
        }
        if let Some(leaf_type) = list_types.last().unwrap().iter().next() {
            Self::gen_nested_list_type(list_types.len() - 1, leaf_type.to_owned())
        } else {
            ValueType::ListOf(Box::new(ValueType::Any))
        }
    }

    pub fn value_type(&self) -> ValueType {
        match self {
            Value::List(l) => {
                let list_types = Self::flat_list_types(l.as_slice());
                Self::unification(list_types)
            }
            _ => self.weak_type(),
        }
    }
}

#[cfg(test)]
mod test_value {
    use super::*;
    #[test]
    fn test_is_instanceof() {
        assert!(Value::Text(vec![]).is_instanceof(&ValueType::Text));
        assert!(Value::List(vec![]).is_instanceof(&ValueType::ListOf(Box::new(ValueType::Str))));
        assert!(!Value::List(vec![(Value::Int(0), Location::Generated)])
            .is_instanceof(&ValueType::ListOf(Box::new(ValueType::Str))));
        assert!(Value::List(vec![
            (
                Value::List(vec![(Value::Int(0), Location::Generated)]),
                Location::Generated
            ),
            (Value::List(vec![]), Location::Generated)
        ])
        .is_instanceof(&ValueType::ListOf(Box::new(ValueType::ListOf(Box::new(
            ValueType::Int
        ))))));
        assert!(!Value::List(vec![
            (
                Value::List(vec![(Value::Int(0), Location::Generated)]),
                Location::Generated
            ),
            (Value::List(vec![]), Location::Generated)
        ])
        .is_instanceof(&ValueType::ListOf(Box::new(ValueType::ListOf(Box::new(
            ValueType::Str
        ))))));
    }

    #[test]
    fn test_merge() {
        let a = vec![
            vec![ValueType::Int].into_iter().collect::<HashSet<_>>(),
            vec![ValueType::Int].into_iter().collect::<HashSet<_>>(),
        ];
        let b = vec![
            vec![ValueType::Str].into_iter().collect::<HashSet<_>>(),
            vec![ValueType::Int].into_iter().collect::<HashSet<_>>(),
        ];
        let expected = vec![
            vec![ValueType::Str, ValueType::Int]
                .into_iter()
                .collect::<HashSet<_>>(),
            vec![ValueType::Int].into_iter().collect::<HashSet<_>>(),
        ];
        assert_eq!(Value::merge_list_types(a, b), expected);

        let a = vec![
            vec![ValueType::Int].into_iter().collect::<HashSet<_>>(),
            vec![ValueType::Int].into_iter().collect::<HashSet<_>>(),
        ];
        let b = vec![vec![ValueType::Str].into_iter().collect::<HashSet<_>>()];
        let expected = vec![
            vec![ValueType::Str, ValueType::Int]
                .into_iter()
                .collect::<HashSet<_>>(),
            vec![ValueType::Int].into_iter().collect::<HashSet<_>>(),
        ];
        assert_eq!(Value::merge_list_types(a, b), expected);

        let a = vec![vec![ValueType::Int]
            .into_iter()
            .collect::<HashSet<ValueType>>()];
        let b = vec![
            vec![ValueType::Str].into_iter().collect::<HashSet<_>>(),
            vec![ValueType::Int].into_iter().collect::<HashSet<_>>(),
        ];
        let expected = vec![
            vec![ValueType::Str, ValueType::Int]
                .into_iter()
                .collect::<HashSet<_>>(),
            vec![ValueType::Int].into_iter().collect::<HashSet<_>>(),
        ];
        assert_eq!(Value::merge_list_types(a, b), expected);
    }

    #[test]
    fn test_flat_list_types() {
        let list = vec![(
            Value::List(vec![
                (Value::Int(0), Location::Generated),
                (Value::Float(2.87), Location::Generated),
                (
                    Value::List(vec![
                        (Value::Int(1), Location::Generated),
                        (Value::Float(3.14), Location::Generated),
                    ]),
                    Location::Generated,
                ),
            ]),
            Location::Generated,
        )];
        let expected = vec![
            vec![ValueType::ListOf(Box::new(ValueType::Any))]
                .into_iter()
                .collect::<HashSet<_>>(),
            vec![
                ValueType::ListOf(Box::new(ValueType::Any)),
                ValueType::Int,
                ValueType::Float,
            ]
            .into_iter()
            .collect::<HashSet<_>>(),
            vec![ValueType::Int, ValueType::Float]
                .into_iter()
                .collect::<HashSet<_>>(),
        ];
        assert_eq!(Value::flat_list_types(list.as_slice()), expected);

        let list = vec![(
            Value::List(vec![
                (
                    Value::List(vec![(Value::Int(1), Location::Generated)]),
                    Location::Generated,
                ),
                (Value::List(vec![]), Location::Generated),
            ]),
            Location::Generated,
        )];
        let expected = vec![
            vec![ValueType::ListOf(Box::new(ValueType::Any))]
                .into_iter()
                .collect::<HashSet<_>>(),
            vec![ValueType::ListOf(Box::new(ValueType::Any))]
                .into_iter()
                .collect::<HashSet<_>>(),
            vec![ValueType::Int].into_iter().collect::<HashSet<_>>(),
        ];
        assert_eq!(Value::flat_list_types(list.as_slice()), expected);
    }

    #[test]
    fn test_unification() {
        let target = vec![
            vec![ValueType::ListOf(Box::new(ValueType::Any))]
                .into_iter()
                .collect::<HashSet<_>>(),
            vec![
                ValueType::ListOf(Box::new(ValueType::Any)),
                ValueType::Int,
                ValueType::Float,
            ]
            .into_iter()
            .collect::<HashSet<_>>(),
            vec![ValueType::Int, ValueType::Float]
                .into_iter()
                .collect::<HashSet<_>>(),
        ];
        assert_eq!(
            Value::unification(target),
            ValueType::ListOf(Box::new(ValueType::Any))
        );

        let target = vec![
            vec![ValueType::ListOf(Box::new(ValueType::Any))]
                .into_iter()
                .collect::<HashSet<_>>(),
            vec![ValueType::ListOf(Box::new(ValueType::Any))]
                .into_iter()
                .collect::<HashSet<_>>(),
            vec![ValueType::Int, ValueType::Float]
                .into_iter()
                .collect::<HashSet<_>>(),
        ];
        assert_eq!(
            Value::unification(target),
            ValueType::ListOf(Box::new(ValueType::ListOf(Box::new(ValueType::Any))))
        );

        let target = vec![
            vec![ValueType::ListOf(Box::new(ValueType::Any))]
                .into_iter()
                .collect::<HashSet<_>>(),
            vec![ValueType::ListOf(Box::new(ValueType::Any))]
                .into_iter()
                .collect::<HashSet<_>>(),
            vec![ValueType::Int].into_iter().collect::<HashSet<_>>(),
        ];
        assert_eq!(
            Value::unification(target),
            ValueType::ListOf(Box::new(ValueType::ListOf(Box::new(ValueType::Int))))
        );
    }
}

pub mod value_utils {
    use super::*;

    type Attrs = HashMap<String, ValueAst>;

    fn access<'a>(attrs: &'a Attrs, name: &str, loc: &Location) -> Result<&'a Value, Error> {
        attrs
            .get(name)
            .ok_or(Error::MissingAttribute {
                name: name.to_owned(),
                loc: loc.to_owned(),
            })
            .map(|(v, _)| v)
    }

    pub fn verify_str<'a>(
        attrs: &'a Attrs,
        name: &str,
        loc: &Location,
    ) -> Result<Option<&'a str>, Error> {
        if let Some((v, _)) = attrs.get(name) {
            v.str()
                .ok_or(Error::InvalidAttributeType {
                    name: name.to_owned(),
                    loc: loc.to_owned(),
                    expected: ValueType::Str,
                    found: v.value_type(),
                })
                .map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn get_str<'a>(attrs: &'a Attrs, name: &str, loc: &Location) -> Result<&'a str, Error> {
        let v = access(attrs, name, loc)?;
        v.str().ok_or(Error::InvalidAttributeType {
            name: name.to_owned(),
            loc: loc.to_owned(),
            expected: ValueType::Str,
            found: v.value_type(),
        })
    }

    pub fn verify_int<'a>(
        attrs: &'a Attrs,
        name: &str,
        loc: &Location,
    ) -> Result<Option<i64>, Error> {
        if let Some((v, _)) = attrs.get(name) {
            v.int()
                .ok_or(Error::InvalidAttributeType {
                    name: name.to_owned(),
                    loc: loc.to_owned(),
                    expected: ValueType::Str,
                    found: v.value_type(),
                })
                .map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn get_int<'a>(attrs: &'a Attrs, name: &str, loc: &Location) -> Result<i64, Error> {
        let v = access(attrs, name, loc)?;
        v.int().ok_or(Error::InvalidAttributeType {
            name: name.to_owned(),
            loc: loc.to_owned(),
            expected: ValueType::Int,
            found: v.value_type(),
        })
    }

    pub fn verify_float<'a>(
        attrs: &'a Attrs,
        name: &str,
        loc: &Location,
    ) -> Result<Option<f64>, Error> {
        if let Some((v, _)) = attrs.get(name) {
            v.float()
                .ok_or(Error::InvalidAttributeType {
                    name: name.to_owned(),
                    loc: loc.to_owned(),
                    expected: ValueType::Str,
                    found: v.value_type(),
                })
                .map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn get_float<'a>(attrs: &'a Attrs, name: &str, loc: &Location) -> Result<f64, Error> {
        let v = access(attrs, name, loc)?;
        v.float().ok_or(Error::InvalidAttributeType {
            name: name.to_owned(),
            loc: loc.to_owned(),
            expected: ValueType::Float,
            found: v.value_type(),
        })
    }

    pub fn verify_text<'a>(
        attrs: &'a Attrs,
        name: &str,
        loc: &Location,
    ) -> Result<Option<&'a [TextElemAst]>, Error> {
        if let Some((v, _)) = attrs.get(name) {
            v.text()
                .ok_or(Error::InvalidAttributeType {
                    name: name.to_owned(),
                    loc: loc.to_owned(),
                    expected: ValueType::Str,
                    found: v.value_type(),
                })
                .map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn get_text<'a>(
        attrs: &'a Attrs,
        name: &str,
        loc: &Location,
    ) -> Result<&'a [TextElemAst], Error> {
        let v = access(attrs, name, loc)?;
        v.text().ok_or(Error::InvalidAttributeType {
            name: name.to_owned(),
            loc: loc.to_owned(),
            expected: ValueType::Float,
            found: v.value_type(),
        })
    }

    pub fn verify_list<'a>(
        attrs: &'a Attrs,
        name: &str,
        loc: &Location,
        element_type: &ValueType,
    ) -> Result<Option<&'a [ValueAst]>, Error> {
        let typ = ValueType::ListOf(Box::new(element_type.to_owned()));
        if let Some((v, _)) = attrs.get(name) {
            if !v.is_instanceof(&typ) {
                Err(Error::InvalidAttributeType {
                    name: name.to_owned(),
                    loc: loc.to_owned(),
                    expected: ValueType::Str,
                    found: v.value_type(),
                })
            } else {
                Ok(v.list())
            }
        } else {
            Ok(None)
        }
    }
    pub fn get_list<'a>(
        attrs: &'a Attrs,
        name: &str,
        loc: &Location,
        element_type: &ValueType,
    ) -> Result<&'a [ValueAst], Error> {
        let v = access(attrs, name, loc)?;
        let typ = ValueType::ListOf(Box::new(element_type.to_owned()));
        if !v.is_instanceof(&typ) {
            return Err(Error::InvalidAttributeType {
                name: name.to_owned(),
                loc: loc.to_owned(),
                expected: typ,
                found: v.value_type(),
            });
        }
        Ok(v.list().unwrap())
    }
}

type ValueAst = (Value, Location);

#[derive(PartialEq, Debug, Clone)]
pub struct Cmd {
    name: String,
    attrs: HashMap<String, ValueAst>,
    inner: Vec<TextElemAst>,
}

#[derive(PartialEq, Debug, Clone)]
pub enum TextElem {
    Cmd(Cmd),
    Plain(String),
    Str(String),
}

type TextElemAst = (TextElem, Location);

pub enum File {
    Tml((Cmd, Location), String),
    Image(image::DynamicImage, Vec<u8>),
    Blob(Vec<u8>),
}

pub type Parsed = HashMap<PathBuf, File>;

fn enumerate_all_file_paths<P>(dir_path: P) -> Vec<PathBuf>
where
    P: AsRef<Path>,
{
    let mut paths = Vec::new();
    for child in fs::read_dir(dir_path).unwrap() {
        if let Ok(child) = child {
            if let Ok(metadata) = child.metadata() {
                if metadata.is_dir() {
                    paths.append(&mut enumerate_all_file_paths(&child.path()));
                } else {
                    paths.push(child.path().to_owned());
                }
            }
        }
    }
    paths
}

pub fn compile_and_write<W: Write + Seek, P>(writer: &mut W, dir_path: P) -> Result<(), Error>
where
    P: AsRef<Path>,
{
    let mut files = HashMap::new();
    let source_paths: Vec<PathBuf> = enumerate_all_file_paths(&dir_path);
    for p in &source_paths {
        if p.extension() == Some(OsStr::new("tml")) {
            info!("add compile target {:?}", p);
            let source = fs::read_to_string(p).map_err(|e| Error::FsError {
                path: p.to_owned(),
                desc: "Cannot read tml file".to_owned(),
                because: e,
            })?;
            let fname = p
                .as_os_str()
                .to_str()
                .ok_or_else(|| Error::CannotInterpretPathAsUTF8(p.to_owned()))?;
            let ast = parser::parse(fname, &source)?;
            files.insert(
                p.strip_prefix(&dir_path)
                    .unwrap()
                    .with_extension("html")
                    .to_owned(),
                File::Tml(ast, source),
            );
        } else if p.extension() == Some(OsStr::new("webp")) {
            let binary = fs::read(p).map_err(|e| Error::FsError {
                path: p.to_owned(),
                desc: "Cannot read blob file".to_owned(),
                because: e,
            })?;
            let img =
                image::load_from_memory(binary.as_slice()).map_err(|e| Error::ImageFmtError {
                    path: p.to_owned(),
                    desc: "Cannot load image".to_owned(),
                    because: e,
                })?;
            files.insert(
                p.strip_prefix(&dir_path).unwrap().to_owned(),
                File::Image(img, binary),
            );
        } else {
            info!("add blob {:?}", p);
            let binary = fs::read(p).map_err(|e| Error::FsError {
                path: p.to_owned(),
                desc: "Cannot read blob file".to_owned(),
                because: e,
            })?;
            files.insert(
                p.strip_prefix(&dir_path).unwrap().to_owned(),
                File::Blob(binary),
            );
        }
    }
    let report = analysis::analyze(&files)?;
    let generated_files = convert::generate_category_pages(&report, &report.css)?
        .into_iter()
        .map(|(p, xml)| (p, xml.to_string().into_bytes()));
    let mut out = files
        .into_iter()
        .map(|(p, file)| match file {
            File::Blob(binary) => Ok((p, binary)),
            File::Tml(cmd, _) => {
                let xml = convert::root(report.get_context(&p).unwrap(), cmd.0)?;
                Ok((p, xml.pretty_print().into_bytes()))
            }
            File::Image(_, binary) => Ok((p, binary)),
        })
        .collect::<Result<HashMap<_, _>, Error>>()?;
    out.extend(generated_files);
    let dist_writer = io::BufWriter::new(writer);
    let mut dist_zip = zip::ZipWriter::new(dist_writer);
    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o444);
    for (p, bin) in out {
        info!("saving {:?}", p);
        dist_zip
            .start_file_from_path(&p, options)
            .map_err(|e| Error::ZipError {
                desc: "Failed to create file in zip archive".to_owned(),
                because: e,
            })?;
        dist_zip.write_all(&bin).map_err(|e| Error::ZipIOError {
            desc: "Failed to write to zip archive".to_owned(),
            because: e,
        })?;
    }
    dist_zip.finish().map_err(|e| Error::ZipError {
        desc: "Failed to flush zip file".to_owned(),
        because: e,
    })?;
    Ok(())
}
