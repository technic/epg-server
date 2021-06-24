#![forbid(non_ascii_idents)]

use askama::Template;
use chrono::prelude::*;
use flate2::bufread::GzDecoder;
use hyperx::header::HttpDate;
use iron::prelude::*;
use iron::status;
use mount::Mount;
use multipart::server::iron::Intercept;
use playlist::PlaylistModel;
use reqwest::header::{CONTENT_TYPE, LAST_MODIFIED};
use router::Router;
use serde::Serializer;
use serde_derive::Serialize;
use staticfile::Static;
use std::collections::HashMap;
use std::error::Error;
use std::io::{BufRead, BufReader};
use std::panic;
use std::path::Path;
use std::str;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time;
use std::{
    cell::Cell,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use urlencoded::UrlEncodedQuery;

mod db;
mod epg;
mod m3u;
mod name_match;
mod playlist;
mod update_status;
mod utils;
mod xmltv;

use crate::update_status::UpdateStatus;
use db::ProgramsDatabase;
use epg::{ChannelInfo, EpgNow, Program};
use utils::{bad_request, error_with_status, get_parameter, server_error};
use xmltv::XmltvReader;

struct LiveCache {
    data: HashMap<i64, EpgNow>,
    begin: i64,
    end: i64,
}

struct IteratorAdapter<I>(Cell<Option<I>>)
where
    I: Iterator,
    I::Item: serde::Serialize;

impl<I> IteratorAdapter<I>
where
    I: Iterator,
    I::Item: serde::Serialize,
{
    fn new(iterator: I) -> Self {
        Self(Cell::new(Some(iterator)))
    }
}

impl<I> serde::Serialize for IteratorAdapter<I>
where
    I: Iterator,
    I::Item: serde::Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Some(it) = self.0.replace(None) {
            serializer.collect_seq(it)
        } else {
            use serde::ser::Error;
            Err(S::Error::custom("attempt to serialize a drained iterator"))
        }
    }
}

impl LiveCache {
    fn new() -> Self {
        LiveCache {
            data: HashMap::new(),
            begin: 0,
            end: 0,
        }
    }

    fn set_data(&mut self, data: HashMap<i64, EpgNow>) {
        self.data = data;
        self.recalculate();
    }

    fn recalculate(&mut self) {
        self.begin = self
            .data
            .values()
            .filter_map(|e| e.programs.first().and_then(|p| Some(p.begin)))
            .max()
            .unwrap_or(0);

        self.end = self
            .data
            .values()
            .filter_map(|e| e.programs.first().and_then(|p| Some(p.end)))
            .min()
            .unwrap_or(0);
    }

    fn contains_time(&self, t: i64) -> bool {
        (self.begin <= t && t <= self.end) && !self.data.is_empty()
    }

    fn to_json(&self, ids: Option<&[i64]>) -> Result<String, serde_json::Error> {
        serde_json::to_string(&match ids {
            Some(ids) => serde_json::json!({
                 "data": IteratorAdapter::new(
                    ids.iter().filter_map(|id| self.data.get(id)),
                )
            }),
            None => serde_json::json!({ "data": IteratorAdapter::new(
               self.data.values()),
            }),
        })
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

type ServerResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

impl EpgSqlServer {
    fn new(file: &str) -> Self {
        Self {
            cache: RwLock::new(LiveCache::new()),
            db: ProgramsDatabase::open(&file).expect("Failed to open database"),
        }
    }

    fn update_data<R: BufRead>(&self, xmltv: XmltvReader<R>) -> ServerResult<()> {
        let t = Instant::now();

        // Load new data
        self.db.load_xmltv(xmltv)?;
        self.cache.write().unwrap().clear();

        println!("Database transactions took {:?}", t.elapsed());
        Ok(())
    }

    fn get_epg_day(&self, id: i64, date: chrono::Date<Utc>) -> ServerResult<Vec<Program>> {
        println!("get_epg_day {} {}", id, date);
        let a = date.and_hms(0, 0, 0).timestamp();
        let b = date.and_hms(23, 59, 59).timestamp();
        self.db.get_range(id, a, b).map_err(|e| e.into())
    }

    fn get_epg_list(
        &self,
        time: chrono::DateTime<Utc>,
        ids: Option<&[i64]>,
    ) -> ServerResult<String> {
        let t = time.timestamp();
        let cache = self.cache.read().unwrap();
        if cache.contains_time(t) {
            println!("Using value from cache");
            cache.to_json(ids).map_err(|e| e.into())
        } else {
            drop(cache);
            let mut cache = self.cache.write().unwrap();
            cache.set_data(self.db.get_at(t, 2)?);
            cache.to_json(ids).map_err(|e| e.into())
        }
    }

    fn find_channel(&self, id: i64) -> ServerResult<Option<ChannelInfo>> {
        // FIXME: shall I ask db to perform search
        self.db
            .get_channels()
            .map(|vec| vec.iter().find(|(i, _c)| *i == id).map(|(_i, c)| c.clone()))
            .map_err(|e| e.into())
    }

    fn find_channel_by_alias(&self, alias: &str) -> ServerResult<Option<(i64, ChannelInfo)>> {
        self.db.get_channel_by_alias(alias).map_err(|e| e.into())
    }

    fn get_channels(&self) -> ServerResult<Vec<(i64, ChannelInfo)>> {
        let mut vec = self.db.get_channels()?;
        vec.sort_by(|(_, a), (_, b)| a.name.cmp(&b.name));
        Ok(vec)
    }

    fn get_channels_alias(&self) -> ServerResult<HashMap<String, i64>> {
        self.db
            .get_channels()
            .map(|vec| {
                vec.into_iter()
                    .map(|(id, channel)| (channel.alias, id))
                    .collect::<HashMap<_, _>>()
            })
            .map_err(|e| e.into())
    }

    fn get_channels_name(&self) -> ServerResult<HashMap<String, i64>> {
        self.db
            .get_channels()
            .map(|vec| {
                vec.into_iter()
                    .map(|(id, channel)| (channel.name, id))
                    .collect::<HashMap<_, _>>()
            })
            .map_err(|e| e.into())
    }
}

struct EpgUpdaterWorker {
    epg_db: Arc<EpgSqlServer>,
    url: String,
    /// Timestamp of recently parsed xmltv data
    last_modified: HttpDate,
}

impl EpgUpdaterWorker {
    fn new(epg_db: Arc<EpgSqlServer>, url: String) -> Self {
        let last_modified: HttpDate = epg_db
            .db
            .get_last_update()
            .unwrap_or_else(|err| {
                eprintln!("Error in get status {}", err);
                None
            })
            .map_or(UNIX_EPOCH, |st| st.last_modified.into())
            .into();
        println!("Last update has file modified at {}", last_modified);
        Self {
            epg_db,
            url,
            last_modified,
        }
    }

    fn run(mut self) -> thread::JoinHandle<()> {
        use rand::Rng;
        thread::spawn(move || loop {
            self.update();
            let minute = rand::thread_rng().gen_range(0..30);
            thread::sleep(time::Duration::from_secs((3 * 60 + minute) * 60));
        })
    }

    fn update(&mut self) {
        // Catch panics, so that `run()` continues to retry even when thread panics
        let st = match panic::catch_unwind(|| self.perform_update()) {
            Ok(Ok(t)) => {
                self.last_modified = t;
                UpdateStatus::new_ok(Utc::now(), SystemTime::from(self.last_modified).into())
            }
            Ok(Err(e)) => {
                eprintln!("Failed to update epg {}", e);
                UpdateStatus::new_fail(Utc::now(), e.to_string())
            }
            Err(_) => {
                eprintln!("Panic in update epg!");
                UpdateStatus::new_fail(Utc::now(), "Panic!".to_string())
            }
        };
        self.epg_db
            .db
            .insert_update_status(st)
            .unwrap_or_else(|e| eprintln!("Error in insert status {}", e));
    }

    fn perform_update(&self) -> ServerResult<HttpDate> {
        static APP_USER_AGENT: &str =
            concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

        println!("check for new epg");
        let client = reqwest::blocking::Client::builder()
            .user_agent(APP_USER_AGENT)
            .gzip(true)
            .build()?;
        let result = client.get(&self.url).send()?;
        let t = result
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| HttpDate::from_str(s).ok())
            .unwrap_or(HttpDate::from(SystemTime::now()));
        println!("last modified {}", t);
        if t > self.last_modified {
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
                    _ => {
                        // hack to support urls with wrong content-type
                        if self.url.ends_with("xmltv") {
                            println!("url ends with 'xmltv' assuming unzipped xml content");
                            zipped = false;
                        }
                    }
                }
            }
            let buf_reader = BufReader::new(result);
            let reader: Box<dyn BufRead> = if !zipped {
                Box::new(buf_reader)
            } else {
                Box::new(BufReader::new(GzDecoder::new(buf_reader)))
            };
            self.epg_db.update_data(XmltvReader::new(reader))?;
            println!("updated epg data");
        } else {
            println!("already up to date");
        }
        Ok(t)
    }
}

impl iron::typemap::Key for EpgSqlServer {
    type Value = EpgSqlServer;
}

fn create_router() -> Router {
    use iron::mime::Mime;

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

            let list = data.get_epg_day(id, date).map_err(server_error)?;
            #[derive(Serialize)]
            struct Data {
                data: Vec<Program>,
            }
            let response = Data { data: list };
            let out = serde_json::to_string(&response)
                .map_err(|e| error_with_status(e, status::InternalServerError))?;
            Ok(Response::with((
                status::Ok,
                "application/json".parse::<Mime>().unwrap(),
                out,
            )))
        } else {
            Ok(Response::with((status::BadRequest, "Invalid parameters")))
        }
    }

    fn get_epg_html(req: &mut Request) -> IronResult<Response> {
        let data = req.get::<persistent::Read<EpgSqlServer>>().unwrap();
        let params = req.get_ref::<UrlEncodedQuery>().map_err(bad_request)?;
        let invalid = || Ok(Response::with((status::BadRequest, "Missing parameters")));
        let not_found = || Ok(Response::with((status::NotFound, "Not found")));

        let opt = match get_parameter(&params, "id") {
            Some(v) => {
                let id = v.parse::<i64>().map_err(bad_request)?;
                data.find_channel(id)
                    .map_err(server_error)?
                    .map(|info| (id, info))
            }
            None => match get_parameter(&params, "alias") {
                Some(v) => data.find_channel_by_alias(v).map_err(server_error)?,
                None => return invalid(),
            },
        };
        let (id, channel) = if let Some(found) = opt {
            found
        } else {
            return not_found();
        };

        let day = match get_parameter(&params, "day") {
            Some(v) => NaiveDate::parse_from_str(v, "%Y.%m.%d")
                .map(|d| Utc.from_utc_date(&d))
                .map_err(bad_request)?,
            None => Utc::now().date(),
        };
        let list = data.get_epg_day(id, day).map_err(server_error)?;
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
    }

    fn get_epg_list(req: &mut Request) -> IronResult<Response> {
        let data = req.get::<persistent::Read<EpgSqlServer>>().unwrap();
        let opt_query = req.get_ref::<UrlEncodedQuery>().ok();

        let time = opt_query
            .and_then(|query| query.get("time"))
            .and_then(|l| l.last())
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|ts| Some(Utc.timestamp(ts, 0)))
            .unwrap_or_else(Utc::now);

        let ids = opt_query
            .and_then(|query| query.get("ids"))
            .and_then(|l| l.last())
            .map(|s| {
                s.split(',')
                    .map(|id| id.parse::<i64>())
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()
            .map_err(bad_request)?;

        let t = Instant::now();

        let out = data
            .get_epg_list(time, ids.as_ref().map(Vec::as_slice))
            .map_err(server_error)?;

        println!("req processed in {:?}", t.elapsed());
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
            data: data.get_channels_alias().map_err(server_error)?,
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
            data: data.get_channels_name().map_err(server_error)?,
        })
        .map_err(|e| error_with_status(e, status::InternalServerError))?;
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
            update: &'a Option<UpdateStatus>,
            today: &'a str,
            channels: &'a [(i64, ChannelInfo)],
        }
        Ok(Response::with((
            status::Ok,
            ChannelsTemplate {
                update: &data
                    .db
                    .get_last_update()
                    .map_err(|e| server_error(Box::new(e)))?,
                today: &format!("{}", Utc::today().format("%Y.%m.%d")),
                channels: &data.get_channels().map_err(server_error)?,
            },
        )))
    }

    fn redirect_to_channels_html(req: &mut Request) -> IronResult<Response> {
        Ok(Response::with((
            status::Found,
            iron::modifiers::Redirect(router::url_for!(req, "get_channels_html")),
        )))
    }

    let mut router = Router::new();
    router.get("/epg_day", get_epg_day, "get_epg_day");
    router.get("/epg_list", get_epg_list, "get_epg_list");
    router.get("/programs.html", get_epg_html, "get_epg_html");
    router.get("/channels", get_channel_ids, "get_channel_ids");
    router.get("/channels.html", get_channels_html, "get_channels_html");
    router.get("/channels_names", get_channel_names, "get_channel_names");
    router.get("/", redirect_to_channels_html, "home");
    router
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

    let app = Arc::new(EpgSqlServer::new(&db_path));

    let worker = EpgUpdaterWorker::new(app.clone(), url);
    let _child = worker.run();

    let mut mount = Mount::new();
    mount.mount("/", create_router());
    mount.mount("static/", Static::new(Path::new("static/")));
    mount.mount("/m3u", PlaylistModel::new());
    mount.mount("/m3u/static/", Static::new(Path::new("static/")));
    let mut chain = Chain::new(mount);
    chain.link_before(persistent::Read::<EpgSqlServer>::one(app));
    chain.link_before(Intercept::default());

    Iron::new(chain)
        .http(format!("localhost:{}", port))
        .unwrap();
}
