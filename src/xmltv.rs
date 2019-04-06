use chrono::prelude::*;
use core::borrow::Borrow;
use epg::ChannelInfo;
use epg::{Channel, Program};
use quick_xml::events::attributes::Attribute;
use quick_xml::events::attributes::Attributes;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::BufRead;
use std::ops::Deref;
use std::str;
use std::time::SystemTime;

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
                    if let Some(s) = s.unescape_and_decode(reader).ok() {
                        self.program.title = s.to_string();
                    }
                }
                Some(ProgramField::Description) => {
                    if let Some(s) = s.unescape_and_decode(reader).ok() {
                        self.program.description = s.to_string();
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
                    self.program.begin = to_timestamp(str::from_utf8(a.value.deref()).unwrap_or(""))
                }
                b"stop" => {
                    self.program.end = to_timestamp(str::from_utf8(a.value.deref()).unwrap_or(""))
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
            Event::Start(element) => {
                if element.local_name() == Self::TAG {
                    self.parse_attributes(element.attributes());
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
            Event::Text(s) => match self.field {
                Some(ChannelField::Name) => {
                    self.channel.name = str::from_utf8(s).unwrap_or("").to_string();
                }
                _ => {}
            },
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

fn to_timestamp(s: &str) -> i64 {
    let dt = DateTime::parse_from_str(s, "%Y%m%d%H%M%S %z");
    dt.unwrap().timestamp()
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
            buf: Vec::new(),
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
    type Item = XmltvItem;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let ev = match self.parser.read_event(&mut self.buf) {
                Ok(Event::Eof) => return None,
                Ok(ev) => ev,
                Err(e) => {
                    println!("Xml parser error: {}", e);
                    return None;
                }
            };
            match self.level {
                Level::Top => match ev {
                    Event::Start(ref element) => match element.local_name() {
                        ProgramParser::TAG => {
                            self.level = Level::Program;
                            self.program_parser.handle_event(&ev, &self.parser);
                        }
                        ChannelParser::TAG => {
                            self.level = Level::Channel;
                            self.channel_parser.handle_event(&ev, &self.parser);
                        }
                        _ => {
                            eprintln!("unknown tag {:?}", element.local_name());
                        }
                    },
                    _ => {}
                },
                Level::Channel => {
                    let result = self.channel_parser.handle_event(&ev, &self.parser);
                    if let Some(channel) = result {
                        self.level = Level::Top;
                        return Some(XmltvItem::Channel(channel));
                    }
                }
                Level::Program => {
                    let result = self.program_parser.handle_event(&ev, &self.parser);
                    if let Some(pair) = result {
                        self.level = Level::Top;
                        return Some(XmltvItem::Program(pair));
                    }
                }
            }
        }
    }
}

/*
pub fn read_xmltv<R: BufRead>(source: R) -> HashMap<i64, Channel> {
    let t = SystemTime::now();

    let mut channels: HashMap<i64, Channel> = HashMap::new();
    let reader = XmltvReader::new(source);
    for item in reader {
        match item {
            XmltvItem::Channel(channel) => {
                if !channels.contains_key(&channel.id) {
                    channels.insert(channel.id, Channel::from_info(channel));
                } else {
                    println!("Duplicate id {}", channel.id)
                }
            }
            XmltvItem::Program((id, program)) => {
                if id != 0 && channels.contains_key(&id) {
                    let channel = channels.get_mut(&id).unwrap();
                    channel.programs.push(program);
                } else {
                    if id != 0 {
                        println!("Unknown id {}", id);
                    }
                }
            }
        }
    }

    for mut channel in channels.values_mut() {
        channel.sort_programs()
    }

    println!("Loaded epg for {} channels", channels.len());
    println!(
        "Total {} programs",
        channels.values().fold(0, |tot, c| tot + c.programs.len())
    );
    println!("Time elapsed: {:?}", t.elapsed().unwrap());

    channels
}
*/
