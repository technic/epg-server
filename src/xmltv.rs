use chrono::prelude::*;
use epg::{Channel, Program};
use std::collections::HashMap;
use std::io::Read;
use std::str;
use std::time::SystemTime;
use xml::attribute::OwnedAttribute;
use xml::reader::Events;
use xml::reader::{EventReader, ParserConfig, XmlEvent};

struct ProgramParser {
    channel_id: i64,
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
    const TAG: &'static str = "programme";

    pub fn new() -> Self {
        ProgramParser {
            channel_id: 0,
            program: Program {
                begin: 0,
                end: 0,
                title: String::new(),
                description: String::new(),
            },
            field: None,
        }
    }

    pub fn handle_event(&mut self, ev: &XmlEvent) -> Option<(i64, Program)> {
        let mut result = None;
        match ev {
            XmlEvent::StartElement {
                name, attributes, ..
            } => {
                if name.local_name == Self::TAG {
                    self.parse_attributes(&attributes);
                } else {
                    self.field = name.local_name.parse().ok();
                }
            }
            XmlEvent::Characters(s) => match self.field {
                Some(ProgramField::Title) => {
                    self.program.title = s.to_string();
                }
                Some(ProgramField::Description) => {
                    self.program.description = s.to_string();
                }
                _ => {}
            },
            XmlEvent::EndElement { name } => {
                if name.local_name == Self::TAG {
                    result = Some((self.channel_id, self.program.clone()));
                    self.reset();
                }
            }
            _ => {
                panic!("unhandled event");
            }
        }
        result
    }

    fn parse_attributes(&mut self, attributes: &[OwnedAttribute]) {
        for a in attributes {
            match a.name.local_name.as_ref() {
                "start" => self.program.begin = to_timestamp(&a.value),
                "stop" => self.program.end = to_timestamp(&a.value),
                "channel" => self.channel_id = a.value.parse().unwrap_or(0),
                _ => {
                    panic!("unknown attribute {}", a.name.local_name);
                }
            }
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
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
    channel: Channel,
    field: Option<ChannelField>,
}

impl ChannelParser {
    const TAG: &'static str = "channel";

    pub fn new() -> Self {
        ChannelParser {
            channel: Channel {
                id: 0,
                name: String::new(),
                icon_url: String::new(),
                programs: Vec::new(),
            },
            field: None,
        }
    }

    pub fn handle_event(&mut self, ev: &XmlEvent) -> Option<Channel> {
        let mut result = None;
        match ev {
            XmlEvent::StartElement {
                name, attributes, ..
            } => {
                if name.local_name == Self::TAG {
                    self.parse_attributes(&attributes);
                } else {
                    self.field = name.local_name.parse().ok();
                    if self.field == Some(ChannelField::IconUrl) {
                        self.channel.icon_url =
                            get_attribute("src", &attributes).unwrap_or("").to_string();
                    }
                }
            }
            XmlEvent::Characters(s) => match self.field {
                Some(ChannelField::Name) => {
                    self.channel.name = s.to_string();
                }
                _ => {}
            },
            XmlEvent::EndElement { name } => {
                if name.local_name == Self::TAG {
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

    fn parse_attributes(&mut self, attributes: &[OwnedAttribute]) {
        for a in attributes {
            match a.name.local_name.as_ref() {
                "id" => {
                    self.channel.id = a.value.parse().unwrap_or(0);
                    if self.channel.id == 0 {
                        println!("bad id {}", a.value);
                    }
                }
                _ => {
                    panic!("Unknown attribute {}", a.name);
                }
            }
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }
}

fn to_timestamp(s: &str) -> i64 {
    let dt = DateTime::parse_from_str(s, "%Y%m%d%H%M%S %z");
    dt.unwrap().timestamp()
}

fn get_attribute<'a>(name: &str, attributes: &'a [OwnedAttribute]) -> Option<&'a str> {
    let mut result = None;
    for a in attributes {
        if a.name.local_name == name {
            result = Some(a.value.as_ref());
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

pub struct XmltvReader<R: Read> {
    level: Level,
    parser: Events<R>,
    channel_parser: ChannelParser,
    program_parser: ProgramParser,
}

impl<R: Read> XmltvReader<R> {
    pub fn new(source: R) -> Self {
        Self {
            level: Level::Top,
            parser: EventReader::new_with_config(source, ParserConfig::new().trim_whitespace(true))
                .into_iter(),
            channel_parser: ChannelParser::new(),
            program_parser: ProgramParser::new(),
        }
    }
}

#[derive(Debug)]
pub enum XmltvItem {
    Channel(Channel),
    Program((i64, Program)),
}

impl<R: Read> Iterator for XmltvReader<R> {
    type Item = XmltvItem;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let ev = match self.parser.next() {
                Some(ev) => ev.unwrap(),
                None => return None,
            };
            match self.level {
                Level::Top => match ev {
                    XmlEvent::StartElement { ref name, .. } => match name.local_name.as_ref() {
                        ProgramParser::TAG => {
                            self.level = Level::Program;
                            self.program_parser.handle_event(&ev);
                        }
                        ChannelParser::TAG => {
                            self.level = Level::Channel;
                            self.channel_parser.handle_event(&ev);
                        }
                        _ => {
                            eprintln!("unknown tag {}", name.local_name);
                        }
                    },
                    _ => {}
                },
                Level::Channel => {
                    let result = self.channel_parser.handle_event(&ev);
                    if let Some(channel) = result {
                        self.level = Level::Top;
                        return Some(XmltvItem::Channel(channel));
                    }
                }
                Level::Program => {
                    let result = self.program_parser.handle_event(&ev);
                    if let Some((id, program)) = result {
                        self.level = Level::Top;
                        return Some(XmltvItem::Program((id, program)));
                    }
                }
            }
        }
        None
    }
}

pub fn read_xmltv<R: Read>(source: R) -> HashMap<i64, Channel> {
    let t = SystemTime::now();

    let mut channels: HashMap<i64, Channel> = HashMap::new();
    let reader = XmltvReader::new(source);
    for item in reader {
        match item {
            XmltvItem::Channel(channel) => {
                if !channels.contains_key(&channel.id) {
                    channels.insert(channel.id, channel);
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
