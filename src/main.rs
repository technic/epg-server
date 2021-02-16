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
    time::{SystemTime, UNIX_EPOCH},
};
use urlencoded::UrlEncodedQuery;

mod db;
mod epg;
mod m3u;
mod name_match;
mod playlist;
mod utils;
mod xmltv;

use db::ProgramsDatabase;
use epg::{ChannelInfo, EpgNow, Program};
use utils::{bad_request, error_with_status, server_error};
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

impl EpgSqlServer {
    fn new(file: &str) -> Self {
        Self {
            cache: RwLock::new(LiveCache::new()),
            db: ProgramsDatabase::open(&file).expect("Failed to open database"),
        }
    }

    fn update_data<R: BufRead>(&self, xmltv: XmltvReader<R>) -> Result<(), Box<dyn Error>> {
        let t = SystemTime::now();

        // Load new data
        self.db.load_xmltv(xmltv)?;
        self.cache.write().unwrap().clear();

        println!(
            "Database transactions took {}s",
            t.elapsed().unwrap().as_secs_f32()
        );
        Ok(())
    }

    fn get_epg_day(
        &self,
        id: i64,
        date: chrono::Date<Utc>,
    ) -> Result<Vec<Program>, Box<dyn Error + Send + Sync>> {
        println!("get_epg_day {} {}", id, date);
        let a = date.and_hms(0, 0, 0).timestamp();
        let b = date.and_hms(23, 59, 59).timestamp();
        self.db.get_range(id, a, b).map_err(|e| e.into())
    }

    fn get_epg_list(
        &self,
        time: chrono::DateTime<Utc>,
        ids: Option<&[i64]>,
    ) -> Result<String, Box<dyn Error + Send + Sync>> {
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

    fn find_channel(&self, id: i64) -> Result<Option<ChannelInfo>, Box<dyn Error + Send + Sync>> {
        // FIXME: shall I ask db to perform search
        self.db
            .get_channels()
            .map(|vec| vec.iter().find(|(i, _c)| *i == id).map(|(_i, c)| c.clone()))
            .map_err(|e| e.into())
    }

    fn get_channels(&self) -> Result<Vec<(i64, ChannelInfo)>, Box<dyn Error + Send + Sync>> {
        let mut vec = self.db.get_channels()?;
        vec.sort_by(|(_, a), (_, b)| a.name.cmp(&b.name));
        Ok(vec)
    }

    fn get_channels_alias(&self) -> Result<HashMap<String, i64>, Box<dyn Error + Send + Sync>> {
        self.db
            .get_channels()
            .map(|vec| {
                vec.into_iter()
                    .map(|(id, channel)| (channel.alias, id))
                    .collect::<HashMap<_, _>>()
            })
            .map_err(|e| e.into())
    }

    fn get_channels_name(&self) -> Result<HashMap<String, i64>, Box<dyn Error + Send + Sync>> {
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
        let channel = data
            .find_channel(id)
            .map_err(server_error)?
            .unwrap_or(ChannelInfo::new());
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

        let t = SystemTime::now();

        let out = data
            .get_epg_list(time, ids.as_ref().map(Vec::as_slice))
            .map_err(server_error)?;

        println!(
            "req processed in {} sec",
            t.elapsed().unwrap().as_secs_f32()
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
            today: &'a str,
            channels: &'a [(i64, ChannelInfo)],
        }
        Ok(Response::with((
            status::Ok,
            ChannelsTemplate {
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
                    _ => {
                        // hack to support urls with wrong content-type
                        if url.ends_with("xmltv") {
                            println!("url ends with 'xmltv' assuming unzipped xml content");
                            zipped = false;
                        }
                    }
                }
            }
            let reader: Box<dyn BufRead> = if !zipped {
                Box::new(BufReader::new(result))
            } else {
                Box::new(BufReader::new(GzDecoder::new(result)))
            };
            epg_wrapper.update_data(XmltvReader::new(reader)).unwrap();
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
                use rand::Rng;
                let minute = rand::thread_rng().gen_range(0..30);
                thread::sleep(time::Duration::from_secs((3 * 60 + minute) * 60));
            }
        }
    });

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
