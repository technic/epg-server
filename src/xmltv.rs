use crate::epg::{ChannelInfo, Program};
use chrono::{prelude::*, ParseResult};
use quick_xml::events::attributes::Attributes;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::BufRead;
use std::ops::Deref;
use std::str;

struct ProgramParser {
    channel_alias: String,
    program: Program,
    field: Option<ProgramField>,
}

#[derive(PartialEq)]
enum ProgramField {
    Title,
    Category,
    Description,
}

impl str::FromStr for ProgramField {
    type Err = ();
    fn from_str(s: &str) -> Result<ProgramField, ()> {
        match s {
            "title" => Ok(ProgramField::Title),
            "category" => Ok(ProgramField::Category),
            "desc" => Ok(ProgramField::Description),
            _ => Err(()),
        }
    }
}

impl ProgramParser {
    const TAG: &'static [u8] = b"programme";

    pub fn new() -> Self {
        ProgramParser {
            channel_alias: String::new(),
            program: Program::new(),
            field: None,
        }
    }

    pub fn handle_event<R: BufRead>(
        &mut self,
        ev: &Event,
        reader: &Reader<R>,
    ) -> Option<(String, Program)> {
        let mut result = None;
        match ev {
            Event::Start(element) => {
                if element.local_name() == Self::TAG {
                    self.parse_attributes(element.attributes());
                } else {
                    self.field = str::from_utf8(element.local_name())
                        .ok()
                        .and_then(|s| s.parse().ok());
                }
            }
            Event::Text(s) => match self.field {
                Some(ProgramField::Title) => {
                    if let Ok(s) = s.unescape_and_decode(reader) {
                        self.program.title = s;
                    }
                }
                Some(ProgramField::Description) => {
                    if let Ok(s) = s.unescape_and_decode(reader) {
                        self.program.description = s;
                    }
                }
                _ => {}
            },
            Event::End(element) => {
                if element.local_name() == Self::TAG {
                    result = Some((self.channel_alias.clone(), self.program.clone()));
                    self.reset();
                }
            }
            // Both Start and End
            // FIXME: copy-paste
            Event::Empty(element) => {
                if element.local_name() == Self::TAG {
                    self.parse_attributes(element.attributes());
                } else {
                    self.field = str::from_utf8(element.local_name())
                        .ok()
                        .and_then(|s| s.parse().ok());
                }
                if element.local_name() == Self::TAG {
                    result = Some((self.channel_alias.clone(), self.program.clone()));
                    self.reset();
                }
            }
            _ => {
                panic!("unhandled event {:?}", ev);
            }
        }
        result
    }

    fn parse_attributes(&mut self, attributes: Attributes) {
        for a in attributes.filter_map(|a| a.ok()) {
            match a.key {
                b"start" => {
                    self.program.begin =
                        to_timestamp(str::from_utf8(a.value.deref()).unwrap_or("")).unwrap_or(0)
                }
                b"stop" => {
                    self.program.end = to_timestamp(str::from_utf8(a.value.deref()).unwrap_or(""))
                        .unwrap_or(self.program.begin + 60)
                }
                b"channel" => {
                    self.channel_alias = str::from_utf8(a.value.deref()).unwrap_or("").to_string();
                }
                _ => {
                    panic!(
                        "unknown attribute {}",
                        str::from_utf8(a.key).unwrap_or("???")
                    );
                }
            }
        }
    }

    fn reset(&mut self) {
        self.channel_alias = String::new();
        self.program = Program::new();
        self.field = None;
    }
}

#[derive(PartialEq)]
enum ChannelField {
    Name,
    IconUrl,
}

impl str::FromStr for ChannelField {
    type Err = ();
    fn from_str(s: &str) -> Result<ChannelField, ()> {
        match s {
            "display-name" => Ok(ChannelField::Name),
            "icon" => Ok(ChannelField::IconUrl),
            _ => Err(()),
        }
    }
}

struct ChannelParser {
    channel: ChannelInfo,
    field: Option<ChannelField>,
}

impl ChannelParser {
    const TAG: &'static [u8] = b"channel";

    pub fn new() -> Self {
        ChannelParser {
            channel: ChannelInfo::new(),
            field: None,
        }
    }

    pub fn handle_event<R: BufRead>(
        &mut self,
        ev: &Event,
        reader: &Reader<R>,
    ) -> Option<ChannelInfo> {
        let mut result = None;
        match ev {
            Event::Start(element) | Event::Empty(element) => {
                if element.local_name() == Self::TAG {
                    self.parse_attributes(element.attributes());
                    // FIXME: copy from Event::End case
                    if let Event::Empty(_) = ev {
                        result = Some(self.channel.clone());
                        self.reset();
                    }
                } else {
                    self.field = str::from_utf8(element.local_name())
                        .ok()
                        .and_then(|s| s.parse().ok());
                    if self.field == Some(ChannelField::IconUrl) {
                        if let Some(s) = get_attribute("src", element.attributes()) {
                            self.channel.icon_url = s;
                        }
                    }
                }
            }
            Event::Text(s) => {
                if let Some(ChannelField::Name) = self.field {
                    self.channel.name = s
                        .unescape_and_decode(reader)
                        .unwrap_or_else(|_| "".to_string());
                }
            }
            Event::End(element) => {
                if element.local_name() == Self::TAG {
                    result = Some(self.channel.clone());
                    self.reset();
                }
            }
            _ => {
                panic!("unexpected event {:?}", ev);
            }
        }
        result
    }

    fn parse_attributes(&mut self, attributes: Attributes) {
        for a in attributes.filter_map(|a| a.ok()) {
            match a.key {
                b"id" => {
                    self.channel.alias = str::from_utf8(a.value.deref()).unwrap_or("").to_string();
                    if self.channel.alias.is_empty() {
                        println!(
                            "bad alias {}",
                            str::from_utf8(a.value.deref()).unwrap_or("???")
                        );
                    }
                }
                _ => {
                    panic!(
                        "Unknown attribute {}",
                        str::from_utf8(a.key).unwrap_or("???")
                    );
                }
            }
        }
    }

    fn reset(&mut self) {
        self.channel = ChannelInfo::new();
        self.field = None;
    }
}

fn to_timestamp(s: &str) -> ParseResult<i64> {
    if s.find(' ').is_some() {
        DateTime::parse_from_str(s, "%Y%m%d%H%M%S %z").map(|dt| std::cmp::max(dt.timestamp(), 0))
    } else {
        NaiveDateTime::parse_from_str(s, "%Y%m%d%H%M%S").map(|dt| std::cmp::max(dt.timestamp(), 0))
    }
}

fn get_attribute(name: &str, attributes: Attributes) -> Option<String> {
    let mut result = None;
    for a in attributes.filter_map(|a| a.ok()) {
        if a.key == name.as_bytes() {
            result = str::from_utf8(a.value.deref()).map(|s| s.to_string()).ok();
        }
    }
    result
}

#[derive(Debug)]
enum Level {
    Top,
    Channel,
    Program,
}

pub struct XmltvReader<R: BufRead> {
    level: Level,
    parser: Reader<R>,
    buf: Vec<u8>,
    channel_parser: ChannelParser,
    program_parser: ProgramParser,
}

impl<R: BufRead> XmltvReader<R> {
    pub fn new(source: R) -> Self {
        let mut reader = Reader::from_reader(source);
        reader.trim_text(true);
        Self {
            level: Level::Top,
            parser: reader,
            buf: Vec::with_capacity(2048),
            channel_parser: ChannelParser::new(),
            program_parser: ProgramParser::new(),
        }
    }
}

#[derive(Debug)]
pub enum XmltvItem {
    Channel(ChannelInfo),
    Program((String, Program)),
}

impl<R: BufRead> Iterator for XmltvReader<R> {
    type Item = Result<XmltvItem, quick_xml::Error>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // We do not borrow from buffer, clear it or it can grow up to the file size
        self.buf.clear();
        loop {
            let ev = match self.parser.read_event(&mut self.buf) {
                Ok(Event::Eof) => return None,
                Ok(ev) => ev,
                Err(e) => {
                    println!("Xml parser error: {}", e);
                    return Some(Err(e));
                }
            };
            match self.level {
                Level::Top => match ev {
                    Event::Start(ref element) | Event::Empty(ref element) => {
                        match element.local_name() {
                            ProgramParser::TAG => {
                                self.level = Level::Program;
                                self.program_parser.handle_event(&ev, &self.parser);
                            }
                            ChannelParser::TAG => {
                                self.level = Level::Channel;
                                self.channel_parser.handle_event(&ev, &self.parser);
                            }
                            _ => {
                                if let Ok(tag) = str::from_utf8(element.local_name()) {
                                    eprintln!("unknown tag {}", tag);
                                } else {
                                    eprintln!("unknown tag {:?}", element.local_name());
                                }
                            }
                        }
                    }
                    _ => {}
                },
                Level::Channel => {
                    let result = self.channel_parser.handle_event(&ev, &self.parser);
                    if let Some(channel) = result {
                        self.level = Level::Top;
                        return Some(Ok(XmltvItem::Channel(channel)));
                    }
                }
                Level::Program => {
                    let result = self.program_parser.handle_event(&ev, &self.parser);
                    if let Some(pair) = result {
                        self.level = Level::Top;
                        return Some(Ok(XmltvItem::Program(pair)));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use chrono::FixedOffset;

    #[test]
    fn test_date() {
        let hour = 3600;
        assert_eq!(
            to_timestamp("20200530181000 +0200").unwrap(),
            FixedOffset::east(2 * hour)
                .ymd(2020, 05, 30)
                .and_hms(18, 10, 00)
                .timestamp()
        );
        assert_eq!(
            to_timestamp("20200530164500").unwrap(),
            Utc.ymd(2020, 05, 30).and_hms(16, 45, 00).timestamp()
        );
    }
}
