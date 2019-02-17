use chrono::prelude::*;
use epg::{Channel, Program};
use std::collections::HashMap;
use std::io::Read;
use std::str;
use std::time::SystemTime;
use xml::attribute::OwnedAttribute;
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
                if name.local_name == ProgramParser::TAG {
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
                if name.local_name == ProgramParser::TAG {
                    result = Some((self.channel_id, self.program.clone()))
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
                if name.local_name == ChannelParser::TAG {
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
            XmlEvent::EndElement { name } => if name.local_name == ProgramParser::TAG {
                result = Some(self.channel.clone());
            },
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

pub fn read_xmltv<R: Read>(source: R) -> HashMap<i64, Channel> {
    let mut channels: HashMap<i64, Channel> = HashMap::new();
    let parser = EventReader::new_with_config(source, ParserConfig::new().trim_whitespace(true));

    #[derive(Debug)]
    enum Level {
        Top,
        Channel,
        Program,
    }

    let mut level = Level::Top;
    let mut channel_handler = ChannelParser::new();
    let mut program_handler = ProgramParser::new();

    let mut i = 0;

    let t = SystemTime::now();

    for ev in parser {
        let ev = ev.expect("xml error");
        //        println!("{}", i);
        match level {
            Level::Top => match ev {
                XmlEvent::StartElement { ref name, .. } => match name.local_name.as_ref() {
                    ProgramParser::TAG => {
                        level = Level::Program;
                        program_handler.handle_event(&ev);
                    }
                    ChannelParser::TAG => {
                        level = Level::Channel;
                        channel_handler.handle_event(&ev);
                    }
                    _ => {
                        eprintln!("unknown tag {}", name.local_name);
                    }
                },
                _ => {}
            },
            Level::Channel => match ev {
                XmlEvent::EndElement { ref name, .. } => {
                    channel_handler.handle_event(&ev);
                    if name.local_name == ChannelParser::TAG {
                        level = Level::Top;
                        let channel = channel_handler.channel;
                        if !channels.contains_key(&channel.id) {
                            channels.insert(channel.id, channel);
                        } else {
                            println!("Duplicate id {}", channel.id)
                        }
                        channel_handler = ChannelParser::new();
                    }
                }
                _ => {
                    channel_handler.handle_event(&ev);
                }
            },
            Level::Program => match ev {
                XmlEvent::EndElement { ref name } => {
                    program_handler.handle_event(&ev);
                    if name.local_name == ProgramParser::TAG {
                        level = Level::Top;
                        let id = program_handler.channel_id;
                        let program = program_handler.program;
                        if id != 0 && channels.contains_key(&id) {
                            let channel = channels.get_mut(&id).unwrap();
                            channel.programs.push(program);
                            i += 1;
                        } else {
                            if id != 0 {
                                println!("Unknown id {}", id);
                            }
                        }
                        program_handler = ProgramParser::new();
                    }
                }
                _ => {
                    program_handler.handle_event(&ev);
                }
            },
        }
    }

    println!("Downloaded epg for {} channels", channels.len());
    println!("Time elapsed: {:?}", t.elapsed().unwrap());

    for mut channel in channels.values_mut() {
        channel.sort_programs()
    }
    channels
}
