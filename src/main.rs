use askama::Template;
use chrono::prelude::*;
use flate2::read::GzDecoder;
use hyperx::header::HttpDate;
use iron::prelude::*;
use iron::status;
use mount::Mount;
use multipart::server::iron::Intercept;
use playlist::PlaylistModel;
use reqwest::header::{CONTENT_TYPE, LAST_MODIFIED};
use router::Router;
use serde_derive::Serialize;
use staticfile::Static;
use std::collections::HashMap;
use std::error::Error;
use std::io::{BufRead, BufReader};
use std::panic;
use std::str;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time;
use std::time::{SystemTime, UNIX_EPOCH};
use urlencoded::UrlEncodedQuery;

mod db;
mod epg;
mod m3u;
mod name_match;
mod playlist;
mod xmltv;

use db::ProgramsDatabase;
use epg::{ChannelInfo, EpgNow, Program};
use xmltv::XmltvReader;

/// Use this function until #54361 becomes stable
fn time_elapsed(t: SystemTime) -> f64 {
    let d = t.elapsed().unwrap();
    d.as_secs() as f64 + d.subsec_micros() as f64 * 1e-6
}

struct LiveCache {
    data: Vec<EpgNow>,
    begin: i64,
    end: i64,
}

impl LiveCache {
    fn new() -> Self {
        LiveCache {
            data: Vec::new(),
            begin: 0,
            end: 0,
        }
    }

    fn set_data(&mut self, data: Vec<EpgNow>) {
        self.data = data;
        self.recalculate();
    }

    fn recalculate(&mut self) {
        self.begin = self
            .data
            .iter()
            .filter_map(|e| e.programs.first().and_then(|p| Some(p.begin)))
            .max()
            .unwrap_or(0);

        self.end = self
            .data
            .iter()
            .filter_map(|e| e.programs.first().and_then(|p| Some(p.end)))
            .min()
            .unwrap_or(0);
    }

    fn contains_time(&self, t: i64) -> bool {
        (self.begin <= t && t <= self.end) && !self.data.is_empty()
    }

    fn to_json(&self) -> String {
        #[derive(Serialize)]
        struct JsonResponse<'a> {
            data: &'a [EpgNow],
        }
        serde_json::to_string(&JsonResponse { data: &self.data }).unwrap()
    }

    fn clear(&mut self) {
        self.begin = 0;
        self.end = 0;
        self.data.clear();
    }
}

struct EpgSqlServer {
    cache: RwLock<LiveCache>,
    db: ProgramsDatabase,
}

impl EpgSqlServer {
    fn new(file: &str) -> Self {
        Self {
            cache: RwLock::new(LiveCache::new()),
            db: ProgramsDatabase::open(&file).expect("Failed to open database"),
        }
    }

    fn update_data<R: BufRead>(&self, xmltv: XmltvReader<R>) {
        let t = SystemTime::now();

        // Load new data
        self.db.load_xmltv(xmltv).unwrap();
        self.cache.write().unwrap().clear();

        println!("Database transactions took {}s", time_elapsed(t));
    }

    fn get_epg_day(&self, id: i64, date: chrono::Date<Utc>) -> Option<Vec<Program>> {
        println!("get_epg_day {} {}", id, date);
        let a = date.and_hms(0, 0, 0).timestamp();
        let b = date.and_hms(23, 59, 59).timestamp();
        Some(self.db.get_range(id, a, b).unwrap())
    }

    fn get_epg_list(&self, time: chrono::DateTime<Utc>) -> String {
        let t = time.timestamp();
        let cache = self.cache.read().unwrap();
        if cache.contains_time(t) {
            println!("Using value from cache");
            cache.to_json()
        } else {
            drop(cache);
            let mut cache = self.cache.write().unwrap();
            cache.set_data(self.db.get_at(t, 2).unwrap());
            cache.to_json()
        }
    }

    fn find_channel(&self, id: i64) -> Option<ChannelInfo> {
        // FIXME: shall I ask db to perform search
        self.db
            .get_channels()
            .unwrap()
            .iter()
            .find(|(i, _c)| *i == id)
            .map(|(_i, c)| c.clone())
    }

    fn get_channels(&self) -> Vec<(i64, ChannelInfo)> {
        let mut vec = self.db.get_channels().unwrap();
        vec.sort_by(|(_, a), (_, b)| a.name.cmp(&b.name));
        vec
    }

    fn get_channels_alias(&self) -> HashMap<String, i64> {
        self.db
            .get_channels()
            .unwrap()
            .into_iter()
            .map(|(id, channel)| (channel.alias, id))
            .collect::<HashMap<_, _>>()
    }

    fn get_channels_name(&self) -> HashMap<String, i64> {
        self.db
            .get_channels()
            .unwrap()
            .into_iter()
            .map(|(id, channel)| (channel.name, id))
            .collect::<HashMap<_, _>>()
    }
}

impl iron::typemap::Key for EpgSqlServer {
    type Value = EpgSqlServer;
}

fn bad_request<E: 'static + Error + Send>(error: E) -> IronError {
    let m = (status::BadRequest, error.description().to_string());
    IronError::new(error, m)
}

fn main() {
    const VERSION: &str = env!("CARGO_PKG_VERSION");

    let args = clap::App::new("epg server")
        .version(VERSION)
        .author("technic93")
        .about("Serves xmltv in json format")
        .arg(
            clap::Arg::with_name("port")
                .long("port")
                .env("APP_PORT")
                .takes_value(true)
                .default_value("3000")
                .help("The port to listen to"),
        )
        .arg(
            clap::Arg::with_name("url")
                .long("url")
                .env("APP_URL")
                .takes_value(true)
                .help("xmltv download url"),
        )
        .arg(
            clap::Arg::with_name("db_path")
                .long("db")
                .env("APP_DB")
                .takes_value(true)
                .default_value("./epg.db")
                .help("path to sqlite database"),
        )
        .get_matches();

    let port = {
        let s = args.value_of("port").unwrap();
        s.parse::<i32>().unwrap_or_else(|e| {
            eprintln!("Bad port argument '{}', {}.", s, e);
            std::process::exit(1);
        })
    };

    let url = args
        .value_of("url")
        .unwrap_or_else(|| {
            eprintln!("Missing url argument");
            std::process::exit(1);
        })
        .to_owned();

    let db_path = {
        fn terminate<T>(e: Box<dyn Error>) -> T {
            eprintln!("Invalid path to database: {}", e);
            std::process::exit(1);
        };
        let path = Path::new(args.value_of("db_path").unwrap());
        if !path.is_file() {
            println!("Creating empty database file");
            std::fs::File::create(path)
                .map_err(|e| e.into())
                .unwrap_or_else(terminate);
        }
        std::fs::canonicalize(path)
            .map_err(|e| e.into())
            .unwrap_or_else(terminate)
            .to_str()
            .map(|s| s.to_owned())
            .ok_or("non utf-8".into())
            .unwrap_or_else(terminate)
    };

    println!("epg server starting");

    fn update_epg(last_t: HttpDate, epg_wrapper: &Arc<EpgSqlServer>, url: &str) -> HttpDate {
        println!("check for new epg");
        let client = reqwest::Client::builder().build().unwrap();
        let result = client.get(url).send().unwrap();
        let t = result
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| HttpDate::from_str(s).ok())
            .unwrap_or(HttpDate::from(SystemTime::now()));
        println!("last modified {}", t);
        if t > last_t {
            println!("loading xmltv");
            let mut zipped = true;
            use mime::Mime;
            if let Some(content_type) = result
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| Mime::from_str(s).ok())
            {
                println!("{:?}", content_type);
                match (content_type.type_(), content_type.subtype()) {
                    (_, mime::XML) => zipped = false,
                    _ => {}
                }
            }
            let reader: Box<dyn BufRead> = if !zipped {
                Box::new(BufReader::new(result))
            } else {
                Box::new(BufReader::new(GzDecoder::new(result)))
            };
            epg_wrapper.update_data(XmltvReader::new(reader));
            println!("updated epg data");
        } else {
            println!("already up to date");
        }
        t
    }

    let app = Arc::new(EpgSqlServer::new(&db_path));

    let _child = thread::spawn({
        let app = app.clone();
        move || {
            let mut last_changed = HttpDate::from(UNIX_EPOCH);
            loop {
                let result = panic::catch_unwind(|| update_epg(last_changed, &app, &url));
                match result {
                    Ok(t) => last_changed = t,
                    Err(_) => println!("Panic in update_epg!"),
                }
                thread::sleep(time::Duration::from_secs(3 * 60 * 60));
            }
        }
    });

    use iron::mime::Mime;
    use std::path::Path;

    let mut router = Router::new();
    router.get("/epg_day", get_epg_day, "get_epg_day");
    router.get("/epg_list", get_epg_list, "get_epg_list");
    router.get("/programs.html", get_epg_html, "get_epg_html");
    router.get("/channels", get_channel_ids, "get_channel_ids");
    router.get("/channels.html", get_channels_html, "get_channels_html");
    router.get("/channels_names", get_channel_names, "get_channel_names");

    let mut mount = Mount::new();
    mount.mount("/", router);
    mount.mount("static/", Static::new(Path::new("static/")));
    mount.mount("/m3u", PlaylistModel::new());
    mount.mount("/m3u/static/", Static::new(Path::new("static/")));
    let mut chain = Chain::new(mount);

    // chain.link_after(mount);
    // FIXME: superfluous nested Arc
    chain.link_before(persistent::Read::<EpgSqlServer>::one(app));
    chain.link_before(Intercept::default());

    fn get_epg_day(req: &mut Request) -> IronResult<Response> {
        let data = req.get::<persistent::Read<EpgSqlServer>>().unwrap();
        let params = req.get_ref::<UrlEncodedQuery>().map_err(bad_request)?;

        if let (Some(day), Some(id)) = (
            params.get("day").and_then(|l| l.last()),
            params.get("id").and_then(|l| l.last()),
        ) {
            let id: i64 = id.parse().map_err(bad_request)?;

            let date = NaiveDate::parse_from_str(day, "%Y.%m.%d")
                .map(|d| Utc.from_utc_date(&d))
                .map_err(bad_request)?;

            if let Some(list) = data.get_epg_day(id, date) {
                #[derive(Serialize)]
                struct Data {
                    data: Vec<Program>,
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

    fn get_epg_html(req: &mut Request) -> IronResult<Response> {
        let data = req.get::<persistent::Read<EpgSqlServer>>().unwrap();
        let params = req.get_ref::<UrlEncodedQuery>().map_err(bad_request)?;
        let invalid = || Ok(Response::with((status::BadRequest, "Missing parameters")));
        let id = match params.get("id").and_then(|l| l.last()) {
            Some(v) => v.parse::<i64>().map_err(bad_request)?,
            None => return invalid(),
        };
        let day = match params.get("day").and_then(|l| l.last()) {
            Some(v) => NaiveDate::parse_from_str(v, "%Y.%m.%d")
                .map(|d| Utc.from_utc_date(&d))
                .map_err(bad_request)?,
            None => Utc::now().date(),
        };
        if let Some(list) = data.get_epg_day(id, day) {
            #[derive(Template)]
            #[template(path = "programs.html")]
            struct ChannelsTemplate<'a> {
                id: i64,
                date: &'a str,
                prev: &'a str,
                next: &'a str,
                channel: &'a str,
                programs: &'a [Program],
            }
            let channel = data.find_channel(id).unwrap_or(ChannelInfo::new());
            Ok(Response::with((
                status::Ok,
                ChannelsTemplate {
                    id,
                    channel: &channel.name,
                    date: &format!("{}", day.format("%A, %d %B %Y")),
                    next: &format!("{}", (day + chrono::Duration::days(1)).format("%Y.%m.%d")),
                    prev: &format!("{}", (day - chrono::Duration::days(1)).format("%Y.%m.%d")),
                    programs: &list,
                },
            )))
        } else {
            Ok(Response::with((status::NotFound, "channel not found")))
        }
    }

    fn get_epg_list(req: &mut Request) -> IronResult<Response> {
        let data = req.get::<persistent::Read<EpgSqlServer>>().unwrap();
        let time = req
            .get_ref::<UrlEncodedQuery>()
            .ok()
            .and_then(|params| params.get("time"))
            .and_then(|l| l.last())
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|ts| Some(Utc.timestamp(ts, 0)))
            .unwrap_or_else(Utc::now);

        let t = SystemTime::now();
        let out = data.get_epg_list(time);
        let d = t.elapsed().unwrap();
        println!(
            "req processed in {} sec",
            d.as_secs() as f64 + d.subsec_nanos() as f64 * 1e-9
        );
        Ok(Response::with((
            status::Ok,
            "application/json".parse::<Mime>().unwrap(),
            out,
        )))
    }

    fn get_channel_ids(req: &mut Request) -> IronResult<Response> {
        let data = req.get::<persistent::Read<EpgSqlServer>>().unwrap();
        #[derive(Serialize)]
        struct Data {
            data: HashMap<String, i64>,
        }
        let out = serde_json::to_string(&Data {
            data: data.get_channels_alias(),
        })
        .unwrap();
        Ok(Response::with((
            status::Ok,
            "application/json".parse::<Mime>().unwrap(),
            out,
        )))
    }

    fn get_channel_names(req: &mut Request) -> IronResult<Response> {
        let data = req.get::<persistent::Read<EpgSqlServer>>().unwrap();
        #[derive(Serialize)]
        struct Data {
            data: HashMap<String, i64>,
        }
        let out = serde_json::to_string(&Data {
            data: data.get_channels_name(),
        })
        .unwrap();
        Ok(Response::with((
            status::Ok,
            "application/json".parse::<Mime>().unwrap(),
            out,
        )))
    }

    fn get_channels_html(req: &mut Request) -> IronResult<Response> {
        let data = req.get::<persistent::Read<EpgSqlServer>>().unwrap();

        #[derive(Template)]
        #[template(path = "channels.html")]
        struct ChannelsTemplate<'a> {
            today: &'a str,
            channels: &'a [(i64, ChannelInfo)],
        }
        Ok(Response::with((
            status::Ok,
            ChannelsTemplate {
                today: &format!("{}", Utc::today().format("%Y.%m.%d")),
                channels: &data.get_channels(),
            },
        )))
    }

    Iron::new(chain)
        .http(format!("localhost:{}", port))
        .unwrap();
}
