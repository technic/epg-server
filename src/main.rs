extern crate bson;
extern crate chrono;
extern crate clap;
extern crate flate2;
extern crate iron;
extern crate mongodb;
extern crate persistent;
extern crate reqwest;
extern crate router;
extern crate timer;
extern crate urlencoded;
extern crate xml;

extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
extern crate hyper;
extern crate rusqlite;

use chrono::prelude::*;
use flate2::read::GzDecoder;
use iron::prelude::*;
use iron::status;
use iron::Error;
use reqwest::header::{HttpDate, LastModified};
use router::Router;
use std::collections::HashMap;
use std::io::Read;
use std::ops::Deref;
use std::panic;
use std::str;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use timer::Timer;
use urlencoded::UrlEncodedQuery;

mod db;
mod epg;
mod store;
mod xmltv;

use epg::{Channel, EpgNow, Program};
use rusqlite::Connection;
use xmltv::XmltvItem;
use xmltv::XmltvReader;

/// Use this function until #54361 becomes stable
fn time_elapsed(t: &SystemTime) -> f64 {
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
            .filter_map(|e| e.programs.last().and_then(|p| Some(p.end)))
            .min()
            .unwrap_or(0);
    }

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

    fn clear(&mut self) {
        self.begin = 0;
        self.end = 0;
        self.data.clear();
    }
}

struct EpgServer {
    channels: RwLock<HashMap<i64, Channel>>,
    cache: RwLock<LiveCache>,
}

impl EpgServer {
    fn new() -> Self {
        EpgServer {
            channels: RwLock::new(HashMap::new()),
            cache: RwLock::new(LiveCache::new()),
        }
    }

    fn set_data(&self, data: HashMap<i64, Channel>) {
        let mut channels = self.channels.write().unwrap();
        *channels = data;
        let mut cache = self.cache.write().unwrap();
        cache.clear();
    }

    fn update_data(&self, mut data: HashMap<i64, Channel>) {
        {
            let channels = self.channels.read().unwrap();
            let time = Utc::now().naive_utc() - chrono::Duration::days(20);
            for channel in channels.values() {
                if let Some(entry) = data.get_mut(&channel.id) {
                    entry.prepend_old_programs(&channel.programs, time.timestamp());
                }
            }
        }
        self.set_data(data);
        self.save();
    }

    fn save(&self) {
        let channels = self.channels.read().unwrap();
        store::save_to_db(&channels).unwrap();
    }

    fn get_epg_day(&self, id: i64, date: chrono::Date<Utc>) -> Option<Vec<Program>> {
        println!("get_epg_day {} {}", id, date);
        let a = date.and_hms(0, 0, 0).timestamp();
        let b = date.and_hms(23, 59, 59).timestamp();
        let channels = self.channels.read().unwrap();
        if let Some(channel) = channels.get(&id) {
            Some(channel.programs_range(a, b).to_vec())
        } else {
            None
        }
    }

    fn get_epg_now(&self, id: i64, time: chrono::DateTime<Utc>) -> Option<Vec<Program>> {
        let channels = self.channels.read().unwrap();
        if let Some(channel) = channels.get(&id) {
            Some(channel.programs_at(time.timestamp(), 3).to_vec())
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
            let channels = self.channels.read().unwrap();
            cache.set_data(
                channels
                    .values()
                    .map(|c| EpgNow {
                        channel_id: c.id,
                        programs: c.programs_at(t, 2).to_vec(),
                    })
                    .collect::<Vec<EpgNow>>(),
            );
            cache.to_json()
        }
    }
}

impl iron::typemap::Key for EpgServer {
    type Value = EpgServer;
}

struct EpgSqlServer {
    cache: RwLock<LiveCache>,
    file: String,
}

impl EpgSqlServer {
    fn new(file: &str) -> Self {
        let conn = Connection::open(&file).expect("Failed to open database");
        db::create_tables(&conn).expect("Failed to create tables");
        Self {
            cache: RwLock::new(LiveCache::new()),
            file: file.to_string(),
        }
    }

    fn update_data<R: Read>(&self, xmltv: XmltvReader<R>) {
        let t = SystemTime::now();

        let mut conn = Connection::open(&self.file).unwrap();
        db::clear_programs_tmp(&conn).unwrap();

        // Clear old epg entries from the database
        let time = Utc::now().naive_utc() - chrono::Duration::days(20);
        db::delete_before(&conn, time.timestamp()).unwrap();

        // Drop indexes to speed up insert
        db::drop_indexes(&conn).unwrap();

        let mut ins_c = 0;
        let mut ins_p = 0;
        // Convert xmltv into sql table
        {
            let tx = conn.transaction().unwrap();
            for item in xmltv {
                match item {
                    XmltvItem::Channel(channel) => {
                        db::insert_channel(tx.deref(), &channel).unwrap();
                        ins_c += 1;
                    }
                    XmltvItem::Program((id, program)) => {
                        db::insert_program(tx.deref(), id, &program).unwrap();
                        ins_p += 1;
                    }
                }
            }
            tx.commit().unwrap();
        }

        println!(
            "Loaded {} channels and {} programs into sql database",
            ins_c, ins_p
        );

        // Merge new programs data into database
        db::append_programs(&mut conn).unwrap();
        db::create_indexes(&conn).unwrap();

        print!("Database transactions took {}s", time_elapsed(&t));
    }

    fn get_epg_day(&self, id: i64, date: chrono::Date<Utc>) -> Option<Vec<Program>> {
        println!("get_epg_day {} {}", id, date);
        let a = date.and_hms(0, 0, 0).timestamp();
        let b = date.and_hms(23, 59, 59).timestamp();
        let conn = Connection::open(&self.file).unwrap();
        Some(db::get_range(&conn, id, a, b).unwrap())
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
            let conn = Connection::open(&self.file).unwrap();
            cache.set_data(db::get_at(&conn, t, 2).unwrap());
            cache.to_json()
        }
    }
}

impl iron::typemap::Key for EpgSqlServer {
    type Value = EpgSqlServer;
}

macro_rules! try_handler {
    ($e:expr) => {
        match $e {
            Ok(x) => x,
            Err(x) => {
                return Ok(Response::with((
                    status::InternalServerError,
                    x.description(),
                )));
            }
        }
    };
    ($e:expr, $error:expr) => {
        match $e {
            Ok(x) => x,
            Err(x) => return Ok(Repsonse::with((
                    $error,
                    x.description(),
                )));
        }
    };
}

fn bad_request<E: 'static + Error + Send>(error: E) -> IronError {
    let m = (status::BadRequest, error.description().to_string());
    IronError::new(error, m)
}

fn main() {
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    let args = clap::App::new("epg server")
        .version(VERSION)
        .author("technic93")
        .about("Serves xmltv in json format")
        .arg(
            clap::Arg::with_name("port")
                .long("port")
                .takes_value(true)
                .default_value("3000")
                .help("The port to listen to"),
        )
        .arg(
            clap::Arg::with_name("url")
                .long("url")
                .takes_value(true)
                .help("xmltv download url"),
        )
        .get_matches();

    let port = {
        let s = args.value_of("port").unwrap();
        s.parse::<i32>().unwrap_or_else(|e| {
            eprintln!("Bad port argument '{}', {}.", s, e);
            std::process::exit(1);
        })
    };

    println!("epg server starting");
    let url = args
        .value_of("url")
        .unwrap_or_else(|| {
            eprintln!("Missing url argument");
            std::process::exit(1);
        })
        .to_owned();

    fn update_epg(last_t: HttpDate, epg_wrapper: &Arc<EpgSqlServer>, url: &str) -> HttpDate {
        println!("check for new epg");
        let client = reqwest::Client::builder().gzip(false).build().unwrap();
        let result = client.get(url).send().unwrap();
        let t = (result.headers().get::<LastModified>().unwrap().deref() as &HttpDate).clone();
        println!("last modified {}", t);
        if t > last_t {
            let gz = GzDecoder::new(result);
            println!("loading xmltv");
            let reader = XmltvReader::new(gz);
            epg_wrapper.update_data(reader);
            println!("updated epg data");
        } else {
            println!("already up to date");
        }
        t
    }

    let epg_wrapper = Arc::new(EpgSqlServer::new("epg.db"));

    //    // Secondly, load epg contained in the persistent database
    //    epg_wrapper.set_data(store::load_db().unwrap());

    // Finally, update epg from the url
    let mut last_changed = update_epg(HttpDate::from(UNIX_EPOCH), &epg_wrapper, &url);

    let timer = Timer::new();
    let _guard = timer.schedule_repeating(chrono::Duration::hours(3), {
        let epg_wrapper = epg_wrapper.clone();
        move || {
            let result = panic::catch_unwind(|| update_epg(last_changed, &epg_wrapper, &url));
            match result {
                Ok(t) => last_changed = t,
                Err(_) => println!("Panic in update_epg!"),
            }
        }
    });

    use iron::mime::Mime;
    //    let content_type = "application/json".parse::<Mime>().unwrap();

    let mut router = Router::new();
    router.get("/epg_day", get_epg_day, "get_epg_day");
    router.get("/epg_list", get_epg_list, "get_epg_list");
    let mut chain = Chain::new(router);
    // FIXME: superfluous nested Arc
    chain.link_before(persistent::Read::<EpgSqlServer>::one(epg_wrapper));

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

    fn get_epg_list(req: &mut Request) -> IronResult<Response> {
        let data = req.get::<persistent::Read<EpgSqlServer>>().unwrap();
        let time = req
            .get_ref::<UrlEncodedQuery>()
            .ok()
            .and_then(|params| params.get("time"))
            .and_then(|l| l.last())
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|ts| Some(Utc.timestamp(ts, 0)))
            .unwrap_or(Utc::now());

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

    //    fn get_epg_live(req: &mut Request) -> IronResult<Response> {
    //        let data = req.get::<persistent::State<EpgServer>>().unwrap();
    //    }

    Iron::new(chain)
        .http(format!("localhost:{}", port))
        .unwrap();
}
