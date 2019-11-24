use lazy_static::lazy_static;
use regex::Regex;
use std::error::Error as StdError;
use std::io;
use std::io::BufRead;

pub const EXTM3U: &str = "#EXTM3U";
pub const EXTINF: &str = "#EXTINF:";
pub const EXTGRP: &str = "#EXTGRP:";

#[derive(Debug, Default, Clone)]
pub struct Entry {
    pub url: String,
    info: String,
    group: String,
}

impl Entry {
    pub fn name(&self) -> &str {
        self.info.splitn(2, ',').nth(1).unwrap_or("")
    }

    pub fn group(&self) -> &str {
        if !self.group.is_empty() {
            &self.group[EXTGRP.len()..]
        } else {
            lazy_static! {
                static ref RE: Regex = Regex::new(r#"group-title="([^"]*)""#).unwrap();
            }
            RE.captures(self.info())
                .map_or("", |cap| cap.get(1).unwrap().as_str())
        }
    }

    pub fn tvg_logo(&self) -> &str {
        lazy_static! {
            static ref RE: Regex = Regex::new(r#"tvg-logo="([^"]*)""#).unwrap();
        }
        RE.captures(self.info())
            .map_or("", |cap| cap.get(1).unwrap().as_str())
    }

    pub fn tvg_id(&self) -> &str {
        lazy_static! {
            static ref RE: Regex = Regex::new(r#"tvg-id="([^"]*)""#).unwrap();
        }
        RE.captures(self.info())
            .map_or("", |cap| cap.get(1).unwrap().as_str())
    }

    pub fn set_tvg_id(&mut self, tvg_id: &str) {
        lazy_static! {
            static ref RE: Regex = Regex::new(r#"tvg-id="([^"]*)""#).unwrap();
        }
        let s = self.info();
        match RE.captures(self.info()) {
            Some(cap) => {
                let m = cap.get(1).unwrap();
                self.info = [
                    EXTINF,
                    &s[..m.start()],
                    tvg_id,
                    &s[m.end()..],
                    ",",
                    self.name(),
                ]
                .join("");
            }
            None => {
                self.append_attributes(&[("tvg-id", tvg_id)]);
            }
        }
    }

    pub fn append_attributes(&mut self, attrs: &[(&str, &str)]) {
        use std::fmt::Write;
        let mut info = String::new();
        info.push_str(EXTINF);
        info.push_str(self.info());
        for (name, value) in attrs {
            write!(info, " {}=\"{}\"", name, value).unwrap();
        }
        info.push(',');
        info.push_str(self.name());
        self.info = info;
    }

    pub fn write_to(&self, out: &mut String) {
        if !self.group.is_empty() {
            out.push_str(&self.group);
            out.push('\n');
        }
        out.push_str(&self.info);
        out.push('\n');
        out.push_str(&self.url);
        out.push('\n');
    }

    fn info(&self) -> &str {
        self.info
            .splitn(2, ',')
            .nth(0)
            .map_or("", |s| &s[EXTINF.len()..])
    }
}

impl Entry {
    fn clear(&mut self) {
        self.url.clear();
        self.info.clear();
        self.group.clear();
    }
}

pub struct Playlist<R: BufRead> {
    reader: R,
    st: State,
    current: Entry,
    line_number: u32,
    buf: String,
}

enum State {
    Header,
    Body,
}

#[derive(Debug)]
pub enum Error {
    M3UError((u32, ErrorKind)),
    IoError(io::Error),
}

#[derive(Debug, PartialEq)]
pub enum ErrorKind {
    InvalidHeader,
    ExpectedInfo,
    ExpectedUrl,
    RepeatedGroup,
    InvalidUrl,
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::IoError(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ErrorKind::*;
        match self {
            Error::M3UError((line, kind)) => {
                match kind {
                    InvalidHeader => write!(f, "Invalid header")?,
                    ExpectedInfo => write!(f, "Expected EXTINF:")?,
                    ExpectedUrl => write!(f, "Expected url")?,
                    RepeatedGroup => write!(f, "Repeated EXTGRP:")?,
                    InvalidUrl => write!(f, "Invalid Url")?,
                };
                write!(f, " at line {}", line)
            }
            Error::IoError(e) => e.fmt(f),
        }
    }
}

impl StdError for Error {}

impl<R: BufRead> Playlist<R> {
    pub fn open(reader: R) -> Self {
        Self {
            reader: reader,
            st: State::Header,
            current: Entry::default(),
            buf: String::new(),
            line_number: 0,
        }
    }
    fn make_error(&self, kind: ErrorKind) -> Option<Result<Entry, Error>> {
        Some(Err(Error::M3UError((self.line_number, kind))))
    }
}

impl<R: BufRead> Iterator for Playlist<R> {
    type Item = Result<Entry, Error>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        use ErrorKind::*;
        use State::*;
        loop {
            self.line_number += 1;
            self.buf.clear();
            match self.reader.read_line(&mut self.buf) {
                Ok(0) => return None,
                Ok(_n) => {
                    self.buf.truncate(self.buf.trim_end().len());
                }
                Err(e) => return Some(Err(Error::from(e))),
            };
            if self.buf.is_empty() {
                continue;
            }
            // dbg!(&self.buf);
            match self.st {
                Header => {
                    if self.buf.starts_with(EXTM3U) {
                        self.st = Body;
                    } else {
                        return self.make_error(InvalidHeader);
                    }
                }
                Body => {
                    if self.buf.starts_with(EXTINF) {
                        if !self.current.info.is_empty() {
                            return self.make_error(ExpectedUrl);
                        }
                        std::mem::swap(&mut self.current.info, &mut self.buf);
                    } else if self.buf.starts_with(EXTGRP) {
                        if !self.current.group.is_empty() {
                            return self.make_error(RepeatedGroup);
                        }
                        std::mem::swap(&mut self.current.group, &mut self.buf);
                    } else {
                        if self.current.info.is_empty() {
                            return self.make_error(ExpectedInfo);
                        }
                        if self.buf.find("://").is_none() {
                            return self.make_error(InvalidUrl);
                        }
                        std::mem::swap(&mut self.current.url, &mut self.buf);
                        let entry = self.current.clone();
                        self.current.clear();
                        return Some(Ok(entry));
                    }
                }
            }
        }
    }
}

pub struct PlaylistWriter {
    storage: String,
}

impl PlaylistWriter {
    pub fn new() -> Self {
        let mut storage = String::new();
        storage.push_str(EXTM3U);
        storage.push('\n');
        Self { storage: storage }
    }

    pub fn push(&mut self, entry: &Entry) {
        entry.write_to(&mut self.storage);
    }
}

impl Into<String> for PlaylistWriter {
    fn into(self) -> String {
        self.storage
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use indoc::indoc;
    use matches::assert_matches;
    use ErrorKind::*;

    #[test]
    fn ok() {
        let data = indoc!(
            r#"#EXTM3U
        #EXTINF:0 tvg-logo="http://icons.org/qqq.png" tvg-id="ch1",Channel 1
        #EXTGRP:Funny
        http://url.com/foo/bar/1.m3u8
        #EXTINF:0,Channel 2
        
        #EXTGRP:Serious
        http://url.com/foo/bar/20.m3u8
        #EXTINF:0,Other channel
        #EXTGRP:Serious
        http://url.com/foo/bar/300.m3u8
        #EXTINF:0 group-title="Hilarious",Foobar
        http://url.com/foo/bar/1000.m3u8
        "#
        );
        let playlist = Playlist::open(data.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        // Test playlist parser
        assert_eq!(playlist.len(), 4);

        assert_eq!(
            playlist[0].info,
            "#EXTINF:0 tvg-logo=\"http://icons.org/qqq.png\" tvg-id=\"ch1\",Channel 1"
        );
        assert_eq!(playlist[0].url, "http://url.com/foo/bar/1.m3u8");
        assert_eq!(playlist[0].group, "#EXTGRP:Funny");

        assert_eq!(playlist[1].info, "#EXTINF:0,Channel 2");
        assert_eq!(playlist[1].url, "http://url.com/foo/bar/20.m3u8");
        assert_eq!(playlist[1].group, "#EXTGRP:Serious");

        assert_eq!(playlist[2].info, "#EXTINF:0,Other channel");
        assert_eq!(playlist[2].url, "http://url.com/foo/bar/300.m3u8");
        assert_eq!(playlist[2].group, "#EXTGRP:Serious");

        assert_eq!(
            playlist[3].info,
            "#EXTINF:0 group-title=\"Hilarious\",Foobar"
        );
        assert_eq!(playlist[3].url, "http://url.com/foo/bar/1000.m3u8");
        assert_eq!(playlist[3].group, "");

        // Test entry parser

        assert_eq!(playlist[0].name(), "Channel 1");
        assert_eq!(playlist[0].group(), "Funny");
        assert_eq!(playlist[0].tvg_id(), "ch1");
        assert_eq!(playlist[0].tvg_logo(), "http://icons.org/qqq.png");

        assert_eq!(playlist[3].name(), "Foobar");
        assert_eq!(playlist[3].group(), "Hilarious");
        assert_eq!(playlist[3].tvg_id(), "");
        assert_eq!(playlist[3].tvg_logo(), "");
    }

    #[test]
    fn bad_header() {
        let data = indoc!(
            r#"
        #EXTINF:0,Foo
        http://url.com/foo/bar/20.m3u8
        "#
        );
        let playlist = Playlist::open(data.as_bytes()).collect::<Result<Vec<_>, _>>();
        assert_matches!(playlist, Err(Error::M3UError((1, InvalidHeader))));
    }
    #[test]
    fn no_info() {
        let data = indoc!(
            r#"#EXTM3U
        http://url.com/foo/bar/20.m3u8
        "#
        );
        let playlist = Playlist::open(data.as_bytes()).collect::<Result<Vec<_>, _>>();
        assert_matches!(playlist, Err(Error::M3UError((2, ExpectedInfo))));
    }

    #[test]
    fn no_url() {
        let data = indoc!(
            r#"#EXTM3U
        #EXTINF:0,Foo
        #EXTINF:0,Bar
        "#
        );
        let playlist = Playlist::open(data.as_bytes()).collect::<Result<Vec<_>, _>>();
        assert_matches!(playlist, Err(Error::M3UError((3, ExpectedUrl))));
    }

    #[test]
    fn bad_url() {
        let data = indoc!(
            r#"#EXTM3U
        #EXTINF:0,Foobar
        iptv.com/20.m3u8
        "#
        );
        let playlist = Playlist::open(data.as_bytes()).collect::<Result<Vec<_>, _>>();
        assert_matches!(playlist, Err(Error::M3UError((3, InvalidUrl))));
    }

    #[test]
    fn tvg_id() {
        let data = indoc!(
            r#"#EXTM3U
        #EXTINF:0 tvg-id="fb",Foobar
        http://iptv.com/1.m3u8
        #EXTINF:0,Channel
        http://iptv.com/2.m3u8
        "#
        );
        let mut playlist = Playlist::open(data.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let entry = &mut playlist[0];
        assert_eq!(entry.tvg_id(), "fb");
        entry.set_tvg_id("foobar");
        assert_eq!(entry.info, "#EXTINF:0 tvg-id=\"foobar\",Foobar");
        let entry = &mut playlist[1];
        assert_eq!(entry.tvg_id(), "");
        entry.set_tvg_id("ch");
        assert_eq!(entry.info, "#EXTINF:0 tvg-id=\"ch\",Channel");
    }
}
