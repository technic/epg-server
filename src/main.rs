extern crate chrono;
extern crate flate2;
extern crate iron;
extern crate router;
extern crate persistent;
extern crate urlencoded;
extern crate reqwest;
extern crate xml;

extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;

use chrono::prelude::*;
use flate2::read::GzDecoder;
use iron::prelude::*;
use iron::status;
use router::Router;
use std::collections::HashMap;
use std::fmt;
use std::str;
use std::sync::RwLock;
use std::time::SystemTime;
use urlencoded::UrlEncodedQuery;
use xml::attribute::OwnedAttribute;
use xml::reader::{EventReader, ParserConfig, XmlEvent};

#[derive(Clone, Serialize)]
struct Program {
    begin: i64,
    end: i64,
    title: String,
    description: String,
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}-{}|{}",
            Utc.timestamp(self.begin, 0).format("%H:%M"),
            Utc.timestamp(self.end, 0).format("%H:%M"),
            self.title
        )
    }
}

struct ProgramParser {
    channel_id: i32,
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

    pub fn handle_event(&mut self, ev: &XmlEvent) -> Option<(i32, Program)> {
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
                "channel" => self.channel_id = a.value.parse().unwrap(),
                _ => {
                    panic!("unknown attribute {}", a.name.local_name);
                }
            }
        }
    }
}

struct Channel {
    id: i32,
    name: String,
    icon_url: String,
    programs: Vec<Program>,
}

impl Channel {
    pub fn sort_programs(&mut self) {
        self.programs.sort_by(|a, b| a.begin.cmp(&b.begin));
    }

    fn programs_range(&self, from: i64, to: i64) -> &[Program] {
        let index_from = self.programs
            .binary_search_by(|p| p.begin.cmp(&from))
            .unwrap_or_else(|i| i);

        let index_to = self.programs
            .binary_search_by(|p| p.begin.cmp(&to))
            .unwrap_or_else(|i| i);

        &self.programs[index_from..index_to]
    }

    fn programs_at(&self, from: i64, count: usize) -> &[Program] {
        let idx = self.programs
            .binary_search_by(|p| p.begin.cmp(&from))
            .unwrap_or_else(|i| i);
        use std::cmp;
        let a = if idx > 0 {
            idx-1
        } else {
            idx
        };
        let b = cmp::min(a + count, self.programs.len());
        &self.programs[a..b]
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
        let result = None;
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
            XmlEvent::EndElement { name } => if name.local_name == ProgramParser::TAG {},
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
                    self.channel.id = a.value.parse().unwrap();
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


struct LiveCache {
    data: Vec<EpgNow>,
    begin: i64,
    end: i64,
}

impl LiveCache {
    fn contains_time(&self, t: i64) -> bool {
        (self.begin <= t && t <= self.end) && self.data.len() > 0
    }

    fn to_json(&self) -> String {
        #[derive(Serialize)]
        struct JsonResponse<'a> {
            data: &'a [EpgNow],
        }
        serde_json::to_string(&JsonResponse { data: &self.data }).unwrap()
    }
}

#[derive(Serialize)]
struct EpgNow {
    channel_id: i32,
    programs: Vec<Program>,
}

struct EpgServer {
    channels: HashMap<i32, Channel>,
    cache: RwLock<LiveCache>,
}

impl EpgServer {
    fn new() -> Self {
        EpgServer {
            channels: HashMap::new(),
            cache: RwLock::new(LiveCache {
                data: Vec::new(),
                begin: 0,
                end: 0,
            }),
        }
    }

    fn get_epg_day(&self, id: i32, date: chrono::Date<Utc>) -> Option<&[Program]> {
        println!("get_epg_day {} {}", id, date);
        let a = date.and_hms(0, 0, 0).timestamp();
        let b = date.and_hms(23, 59, 59).timestamp();
        if let Some(channel) = self.channels.get(&id) {
            Some(channel.programs_range(a, b))
        } else {
            None
        }
    }

    fn get_epg_now(&self, id: i32, time: chrono::DateTime<Utc>) -> Option<&[Program]> {
        if let Some(channel) = self.channels.get(&id) {
            Some(channel.programs_at(time.timestamp(), 3))
        } else {
            None
        }
    }

    fn get_epg_list(&self, time: chrono::DateTime<Utc>) -> String {
        let t = time.timestamp();
        let cache = self.cache.read().unwrap();
        println!("{} < {} < {}", cache.begin, t, cache.end);
        if cache.contains_time(t) {
            println!("Using value from cache");
            cache.to_json()
        } else {
            drop(cache);
            let mut cache = self.cache.write().unwrap();
            cache.data = self.channels
                .values()
                .map(|c| EpgNow {
                    channel_id: c.id,
                    programs: c.programs_at(t, 2).to_vec(),
                })
                .collect::<Vec<EpgNow>>();

            cache.begin = cache
                .data
                .iter()
                .map(|e| e.programs.first().and_then(|p| Some(p.begin)))
                .map(|ts| ts.unwrap_or(0))
                .max()
                .unwrap_or(0);

            cache.end = cache
                .data
                .iter()
                .map(|e| e.programs.last().and_then(|p| Some(p.end)))
                .map(|ts| ts.unwrap_or(<i64>::max_value()))
                .min()
                .unwrap_or(0);

            cache.to_json()
        }
    }
}

use iron::typemap::Key;
impl Key for EpgServer {
    type Value = EpgServer;
}


macro_rules! try_handler {
    ($e:expr) => {
        match $e {
            Ok(x) => x,
            Err(e) => {
                return Ok(Response::with((
                    status::InternalServerError,
                    format!("{:?}", e),
                )))
            }
        }
    };
    ($e:expr, $error:expr) => {
        match $e {
            Ok(x) => x,
            Err(x) => return Ok(Repsonse::with(($error, format!("{:?}", e)))),
        }
    };
}

fn main() {
    println!("epg server starting");

    let result = reqwest::get("http://epg.it999.ru/edem.xml.gz").expect("epg download failed");
    let gz = GzDecoder::new(result);

    let mut channels: HashMap<i32, Channel> = HashMap::new();

    let parser = EventReader::new_with_config(gz, ParserConfig::new().trim_whitespace(true));

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
                        if channels.contains_key(&id) {
                            let channel = channels.get_mut(&id).unwrap();
                            channel.programs.push(program);
                            i += 1;
                        } else {
                            println!("Unknown id {}", id)
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

    use iron::mime::Mime;
//    let content_type = "application/json".parse::<Mime>().unwrap();
    let mut epg_cache = EpgServer::new();
    epg_cache.channels = channels;

    let mut router = Router::new();
    router.get("/epg_day", get_epg_day, "get_epg_day");
    router.get("/epg_list", get_epg_list, "get_epg_list");
    let mut chain = Chain::new(router);
    chain.link(persistent::Read::<EpgServer>::both(epg_cache));

    fn get_epg_day(req: &mut Request) -> IronResult<Response> {
        println!("get_epg_day");
        let data = req.get::<persistent::Read<EpgServer>>().unwrap();
        let params = try_handler!(req.get_ref::<UrlEncodedQuery>());

        if let (Some(day), Some(id)) = (
            params.get("day").and_then(|l| l.last()),
            params.get("id").and_then(|l| l.last()),
        ) {
            let id: i32 = try_handler!(id.parse());

            println!("{}", day);
            let mut date;
            let v = day.split(".").collect::<Vec<&str>>();
            if v.len() == 3 {
                let y: i32 = try_handler!(v[0].parse());
                let m: u32 = try_handler!(v[1].parse());
                let d: u32 = try_handler!(v[2].parse());
                date = Utc.ymd(y, m, d);
            } else {
                return Ok(Response::with((
                    status::BadRequest,
                    format!("Bad day {}", day),
                )));
            }

            println!("{}", date);

            if let Some(list) = data.get_epg_day(id, date) {
                #[derive(Serialize)]
                struct Data<'a> {
                    data: &'a [Program],
                }
                let response = Data { data: list };
                let out = serde_json::to_string(&response).unwrap();
                Ok(Response::with((
                    status::Ok,
                    "application/json".parse::<Mime>().unwrap(),
                    out,
                )))
            } else {
                Ok(Response::with((status::BadRequest, "channel not found")))
            }
        } else {
            Ok(Response::with((status::BadRequest, "Invalid parameters")))
        }
    }

    fn get_epg_list(req: &mut Request) -> IronResult<Response> {
        let data = req.get::<persistent::Read<EpgServer>>().unwrap();
        let params = try_handler!(req.get_ref::<UrlEncodedQuery>());
        if let Some(time) = params
            .get("time")
            .and_then(|l| l.last())
            .and_then(|s| s.parse::<i64>().ok())
        {
            let t = SystemTime::now();
            let out = data.get_epg_list(Utc.timestamp(time, 0));
            let d = t.elapsed().unwrap();
            println!("req processed in {} sec", d.as_secs() as f64 + d.subsec_nanos() as f64 * 1e-9);
            Ok(Response::with((
                status::Ok,
                "application/json".parse::<Mime>().unwrap(),
                out,
            )))
        } else {
            Ok(Response::with((status::BadRequest, "Invalid parameters")))
        }
    }

    //    fn get_epg_live(req: &mut Request) -> IronResult<Response> {
    //        let data = req.get::<persistent::State<EpgServer>>().unwrap();
    //    }

    Iron::new(chain).http("0.0.0.0:3000").unwrap();
}
