use crate::epg::ChannelInfo;
use crate::m3u;
use crate::m3u::Playlist;
use crate::m3u::PlaylistWriter;
use crate::name_match::VecMatcher;
use crate::utils::{bad_request, server_error};
use crate::EpgSqlServer;
use askama::Template;
use async_std::task;
use io::Read;
use iron::prelude::*;
use iron::status;
use lazy_static::lazy_static;
use multipart::server::save::DataReader;
use multipart::server::Entries;
use router::Router;
use serde_derive::Serialize;
use std::collections::HashMap;
use std::io;
use std::iter::FromIterator;
use std::time::Instant;
use urlencoded::UrlEncodedBody;

pub struct PlaylistModel {}

const SIM_GOOD: f32 = 0.7;
const SIM_POSSIBLE: f32 = 0.45;

struct ProcessedItem {
    entry: m3u::Entry,
    name: String,
    sim: f32,
}

#[derive(Debug, Serialize)]
struct SearchResultItem {
    name: String,
    alias: String,
}

fn process<R: io::BufRead>(
    buf: R,
    channels: &[ChannelInfo],
) -> Result<Vec<ProcessedItem>, m3u::Error> {
    let t = Instant::now();

    let mut result = Vec::new();
    let dataset = channels.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    let mut corpus = VecMatcher::new(&dataset, 2);
    for elem in Playlist::open(buf) {
        let mut elem = elem?;
        let ret = corpus.search_best(elem.name(), SIM_GOOD);
        if let Some((index, mut sim)) = ret {
            if (sim - 1.0).abs() < 1e-5 {
                sim = 1.0
            }
            elem.set_tvg_id(&channels[index].alias);
            result.push(ProcessedItem {
                entry: elem,
                name: channels[index].name.clone(),
                sim: sim,
            })
        } else {
            elem.set_tvg_id("");
            result.push(ProcessedItem {
                entry: elem,
                name: String::new(),
                sim: 0.0,
            });
        }
    }

    println!("playlist processed in {:?}", t.elapsed());
    Ok(result)
}

/// Searches channels with similar name in the database
fn find(name: &str, channels: &[ChannelInfo]) -> Vec<SearchResultItem> {
    let dataset = channels.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    let mut corpus = VecMatcher::new(&dataset, 2);
    let ret = corpus.search(name, SIM_POSSIBLE, 10);
    ret.iter()
        .map(|(index, _sim)| {
            let c = &channels[*index];
            SearchResultItem {
                name: c.name.clone(),
                alias: c.alias.clone(),
            }
        })
        .collect()
}

fn replace_tvg<R: io::BufRead>(
    buf: R,
    replace: HashMap<String, String>,
    channels: &[ChannelInfo],
) -> Result<String, m3u::Error> {
    let aliases = HashMap::<&str, &str>::from_iter(
        channels.iter().map(|c| (c.name.as_str(), c.alias.as_str())),
    );
    let mut result = PlaylistWriter::new();
    let playlist = Playlist::open(buf);
    for entry in playlist {
        let mut entry = entry?;
        if let Some(name) = replace.get(entry.name()) {
            if name.is_empty() {
                entry.set_tvg_id("");
            } else if let Some(tvg) = aliases.get(name.as_str()) {
                entry.set_tvg_id(tvg);
            }
        }
        result.push(&entry);
    }
    Ok(result.into())
}

#[derive(Debug)]
struct ErrorMessage(String);

impl From<&str> for ErrorMessage {
    fn from(s: &str) -> Self {
        ErrorMessage(s.to_string())
    }
}

impl std::fmt::Display for ErrorMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ErrorMessage {}

static RECAPTCHA_KEY: &str = "g-recaptcha-response";
lazy_static! {
    static ref RECAPTCHA_PUBLIC: String = dotenv::var("RECAPTCHA_PUBLIC").unwrap_or(String::new());
    static ref RECAPTCHA_PRIVATE: String =
        dotenv::var("RECAPTCHA_PRIVATE").unwrap_or(String::new());
}

impl PlaylistModel {
    pub fn new() -> Router {
        let mut router = Router::new();
        router.get("/index.html", PlaylistModel::welcome_page, "welcome_page");
        router.post(
            "/index.html",
            PlaylistModel::upload_playlist,
            "upload_playlist",
        );
        router.post("/find", PlaylistModel::find_matches, "find_matches");
        router.post(
            "/get_m3u",
            PlaylistModel::download_playlist,
            "download_playlist",
        );
        router
    }

    fn get_entry<'a>(entries: &'a Entries, key: &str) -> IronResult<DataReader<'a>> {
        let entry = entries
            .fields
            .get(key)
            .and_then(|v| v.first())
            .ok_or_else(|| ErrorMessage(format!("Missing {}", key)))
            .map_err(bad_request)?;
        entry.data.readable().map_err(bad_request)
    }

    fn welcome_page(_req: &mut Request) -> IronResult<Response> {
        #[derive(Template)]
        #[template(path = "playlist.html")]
        struct HomeTemplate {
            recaptcha_public: &'static str,
        }
        Ok(Response::with((
            status::Ok,
            HomeTemplate {
                recaptcha_public: &RECAPTCHA_PUBLIC,
            },
        )))
    }

    fn upload_playlist(req: &mut Request) -> IronResult<Response> {
        let data = req.get::<persistent::Read<EpgSqlServer>>().unwrap();
        let entries = req
            .extensions
            .get::<Entries>()
            .ok_or_else(|| ErrorMessage::from("No parameters"))
            .map_err(bad_request)?;

        // recaptcha
        let mut captcha = String::new();
        Self::get_entry(&entries, RECAPTCHA_KEY)?
            .read_to_string(&mut captcha)
            .map_err(bad_request)?;
        if let Err(e) = task::block_on(recaptcha::verify(&RECAPTCHA_PRIVATE, &captcha, None)) {
            println!("captcha error {}", e);
            return Ok(Response::with((status::Forbidden, "")));
        }

        let file = Self::get_entry(&entries, "playlistFile")?;
        let channels = data
            .get_channels()
            .map_err(server_error)?
            .into_iter()
            .map(|(_, c)| c)
            .collect::<Vec<_>>();
        let channels = process(file, &channels).map_err(bad_request)?;
        let mut playlist = PlaylistWriter::new();
        for c in channels.iter() {
            playlist.push(&c.entry)
        }
        let buf: String = playlist.into();
        #[derive(Template)]
        #[template(path = "playlist_table.html")]
        struct PlaylistTemplate<'a> {
            sim_good: f32,
            playlist: &'a str,
            channels: &'a [ProcessedItem],
        }
        Ok(Response::with((
            status::Ok,
            PlaylistTemplate {
                sim_good: SIM_GOOD,
                playlist: &buf,
                channels: &channels,
            },
        )))
    }

    fn find_matches(req: &mut Request) -> IronResult<Response> {
        use iron::mime::Mime;
        let server = req.get::<persistent::Read<EpgSqlServer>>().unwrap();
        let name = req
            .get_ref::<UrlEncodedBody>()
            .ok()
            .and_then(|params| params.get("name"))
            .and_then(|l| l.last())
            .ok_or_else(|| ErrorMessage::from("Invalid parameters"))
            .map_err(bad_request)?;

        #[derive(Serialize)]
        struct Json {
            data: Vec<SearchResultItem>,
        }
        let channels = server
            .get_channels()
            .map_err(server_error)?
            .into_iter()
            .map(|(_, c)| c)
            .collect::<Vec<_>>();
        let out = serde_json::to_string(&Json {
            data: dbg!(find(name, &channels)),
        })
        .map_err(bad_request)?;
        Ok(Response::with((
            status::Ok,
            "application/mpegurl".parse::<Mime>().unwrap(),
            out,
        )))
    }

    fn download_playlist(req: &mut Request) -> IronResult<Response> {
        use iron::mime::Mime;
        let server = req.get::<persistent::Read<EpgSqlServer>>().unwrap();
        let entries = req
            .extensions
            .get::<Entries>()
            .ok_or_else(|| ErrorMessage::from("No parameters"))
            .map_err(bad_request)?;

        // recaptcha
        let mut captcha = String::new();
        Self::get_entry(&entries, RECAPTCHA_KEY)?
            .read_to_string(&mut captcha)
            .map_err(bad_request)?;
        if let Err(e) = task::block_on(recaptcha::verify(&RECAPTCHA_PRIVATE, &captcha, None)) {
            println!("captcha error {}", e);
            return Ok(Response::with((status::Forbidden, "")));
        }

        let file = Self::get_entry(&entries, "playlistFile")?;
        let changes = Self::get_entry(&entries, "changes")?;

        let replace: HashMap<String, String> =
            serde_json::from_reader(changes).map_err(bad_request)?;
        let channels = server
            .get_channels()
            .map_err(server_error)?
            .into_iter()
            .map(|(_, c)| c)
            .collect::<Vec<_>>();
        let out = replace_tvg(file, replace, &channels).map_err(bad_request)?;
        Ok(Response::with((
            status::Ok,
            "application/mpegurl".parse::<Mime>().unwrap(),
            out,
        )))
    }
}
