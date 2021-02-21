use crate::epg::{ChannelInfo, EpgNow, Program};
use crate::update_status::UpdateStatus;
use crate::xmltv::XmltvItem;
use crate::xmltv::XmltvReader;
use chrono::prelude::*;
use error_chain::ChainedError;
use failure::Fail;
use rusqlite::{types::ToSql, OptionalExtension};
use rusqlite::{Connection, Result, NO_PARAMS};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::error::Error;
use std::io::BufRead;
use std::path::PathBuf;
use std::{fmt, fs};

pub struct ProgramsDatabase {
    file: String,
}

impl ProgramsDatabase {
    pub fn open(file: &str) -> Result<Self> {
        let conn = Connection::open(&file)?;
        conn.execute_batch("pragma journal_mode=WAL")?;
        conn.execute_batch("pragma cache_size=10000")?;
        conn.execute(
            "create table if not exists channels \
             (id integer primary key, alias text unique, name text, icon_url text)",
            NO_PARAMS,
        )?;
        conn.execute(
            "create table if not exists programs (
             id integer primary key autoincrement,
             channel integer,
             begin integer,
             end integer,
             title text,
             description text
             )",
            NO_PARAMS,
        )?;
        conn.execute(
            "create table if not exists programs1 (
             id integer primary key autoincrement,
             channel integer,
             begin integer,
             end integer,
             title text,
             description text
             )",
            NO_PARAMS,
        )?;
        let db = Self {
            file: file.to_string(),
        };

        #[derive(Debug)]
        struct MigrantError {
            message: String,
        }

        impl fmt::Display for MigrantError {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "{}", self.message)
            }
        }

        impl Error for MigrantError {
            fn description(&self) -> &str {
                &self.message
            }
        }

        db.run_migrations()
            .or_else(|e| {
                if e.is_migration_complete() {
                    println!("All migrations complete!");
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .map_err(|e| {
                println!("Migration failed: {}", e.display_chain());
                rusqlite::Error::UserFunctionError(Box::new(MigrantError {
                    message: e.description().to_string(),
                }))
            })?;
        Ok(db)
    }

    fn run_migrations(&self) -> std::result::Result<(), migrant_lib::errors::Error> {
        let path = fs::canonicalize(PathBuf::from(&self.file))?;
        let settings = migrant_lib::Settings::configure_sqlite()
            .database_path(path)?
            .build()?;
        let mut config = migrant_lib::Config::with_settings(&settings);
        config.setup()?;
        config.use_cli_compatible_tags(true);
        macro_rules! make_migration {
            ($tag:expr) => {
                migrant_lib::EmbeddedMigration::with_tag($tag)
                    .up(include_str!(concat!("../migrations/", $tag, "/up.sql")))
                    .down(include_str!(concat!("../migrations/", $tag, "/down.sql")))
                    .boxed()
            };
        }
        config.use_migrations(&[
            make_migration!("20190325100907_channel-alias"),
            make_migration!("20210221123809_update-log"),
        ])?;
        let config = config.reload()?;
        migrant_lib::list(&config)?;
        println!("Applying migrations ...");
        migrant_lib::Migrator::with_config(&config)
            .all(true)
            .show_output(true)
            .apply()?;
        let config = config.reload()?;
        migrant_lib::list(&config)?;
        Ok(())
    }

    pub fn load_xmltv<R: BufRead>(&self, xmltv: XmltvReader<R>) -> Result<()> {
        let mut conn = Connection::open(&self.file)?;

        // Make sure that temporary storage is clean
        conn.execute("drop index if exists p1_channel", NO_PARAMS)?;
        conn.execute("delete from programs1", NO_PARAMS)?;

        let mut ids: HashMap<String, i64> = self
            .get_channels()?
            .into_iter()
            .map(|(id, info)| (info.alias, id))
            .collect();

        let mut ins_c = 0;
        let mut ins_p = 0;
        let mut result = Ok(());
        println!("Parsing XMLTV entries into database ...");
        // Convert xmltv into sql table
        {
            let tx = conn.transaction()?;
            for item in xmltv {
                match item {
                    Ok(XmltvItem::Channel(channel)) => {
                        match ids.entry(channel.alias) {
                            Entry::Occupied(entry) => {
                                // Chanel with this alias already exists
                                let &id = entry.get();
                                update_channel(
                                    &tx,
                                    id,
                                    entry.key(),
                                    &channel.name,
                                    &channel.icon_url,
                                )?;
                            }
                            Entry::Vacant(entry) => {
                                // First try use alias as an integer id
                                if let Ok(id) = entry.key().parse::<i64>() {
                                    update_channel(
                                        &tx,
                                        id,
                                        entry.key(),
                                        &channel.name,
                                        &channel.icon_url,
                                    )?;
                                    entry.insert(id);
                                } else {
                                    // Insert new channel and assign it new id
                                    let id = insert_channel(
                                        &tx,
                                        entry.key(),
                                        &channel.name,
                                        &channel.icon_url,
                                    )?;
                                    entry.insert(id);
                                }
                            }
                        }
                        ins_c += 1;
                    }
                    Ok(XmltvItem::Program((alias, program))) => {
                        if let Some(&id) = ids.get(&alias) {
                            insert_program(&tx, id, &program)?;
                            ins_p += 1;
                        } else {
                            eprintln!("Skip program for unknown channel {}", alias);
                        }
                    }
                    Err(e) => {
                        // Process all parsed items and return Error in the end
                        result = Err(rusqlite::Error::UserFunctionError(Box::new(e.compat())));
                        break;
                    }
                }
            }
            tx.commit()?;
        }

        println!(
            "Loaded {} channels and {} programs into sql database",
            ins_c, ins_p
        );

        // Clear old epg entries from the database
        let time = Utc::now().naive_utc() - chrono::Duration::days(20);
        self.delete_before(time.timestamp())?;
        // Merge new programs data into database
        append_programs(&mut conn)?;
        // Clean up obsolete channels
        clear_channels(&mut conn)?;
        result
    }

    pub fn get_channels(&self) -> Result<Vec<(i64, ChannelInfo)>> {
        let conn = Connection::open(&self.file)?;
        let mut stmt = conn.prepare("select id, alias, name, icon_url from channels")?;
        let it = stmt
            .query_map(NO_PARAMS, |row| {
                Ok((
                    {
                        let id: i64 = row.get(0)?;
                        id
                    },
                    ChannelInfo {
                        alias: row.get(1)?,
                        name: row.get(2)?,
                        icon_url: row.get(3)?,
                    },
                ))
            })?
            .filter_map(|item| item.ok());
        Ok(it.collect::<Vec<_>>())
    }

    pub fn get_at(&self, timestamp: i64, count: i64) -> Result<HashMap<i64, EpgNow>> {
        let conn = Connection::open(&self.file)?;
        let mut stmt = conn.prepare(
            "select
                channels.id,
                programs.begin, programs.end, programs.title, programs.description
             from channels
             join programs on programs.id in
             (select programs.id from programs where
              programs.channel=channels.id AND programs.end > ?1 order by programs.end limit ?2)",
        )?;

        let mut hash: HashMap<i64, EpgNow> = HashMap::new();

        let it = stmt.query_map(&[&timestamp, &count], |row| {
            let id: i64 = row.get(0)?;
            let program = Program {
                begin: row.get(1)?,
                end: row.get(2)?,
                title: row.get(3)?,
                description: row.get(4)?,
            };
            Ok((id, program))
        })?;

        for (id, program) in it.filter_map(|item| item.ok()) {
            hash.entry(id)
                .or_insert(EpgNow {
                    channel_id: id,
                    programs: Vec::new(),
                })
                .programs
                .push(program);
        }
        Ok(hash)
    }

    pub fn get_range(&self, id: i64, from: i64, to: i64) -> Result<Vec<Program>> {
        let conn = Connection::open(&self.file)?;
        let mut stmt = conn.prepare(
            "select programs.begin, programs.end, programs.title, programs.description
         from programs where
         programs.channel = ?1 and programs.begin >= ?2 and programs.begin < ?3",
        )?;
        let it = stmt
            .query_map(&[&id, &from, &to], |row| {
                Ok(Program {
                    begin: row.get(0)?,
                    end: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                })
            })?
            .filter_map(|item| item.ok());
        Ok(it.collect::<Vec<_>>())
    }

    pub fn delete_before(&self, timestamp: i64) -> Result<()> {
        println!("Removing programs before t={} from sqlite ...", timestamp);
        let conn = Connection::open(&self.file)?;
        let count = conn.execute(
            "delete from programs where programs.end < ?1",
            &[&timestamp],
        )?;
        println!("Deleted {} rows.", count);
        Ok(())
    }

    pub fn get_last_update(&self) -> Result<Option<UpdateStatus>> {
        let conn = Connection::open(&self.file)?;
        conn.query_row(
            "select time, status, message from update_log order by time desc limit 1",
            NO_PARAMS,
            |row| {
                let t = Utc.timestamp(row.get(0)?, 0);
                match row.get(1)? {
                    0 => Ok(UpdateStatus::new_ok(t)),
                    1 => Ok(UpdateStatus::new_fail(t, row.get(2)?)),
                    _ => Err(rusqlite::Error::UserFunctionError(
                        "Bad status value".into(),
                    )),
                }
            },
        )
        .optional()
    }

    pub fn insert_update_status(&self, entry: UpdateStatus) -> Result<()> {
        let conn = Connection::open(&self.file)?;
        if let Some(t) = conn
            .query_row(
                "select time from update_log where time=?1",
                rusqlite::params![entry.time.timestamp()],
                |row| row.get(0),
            )
            .optional()?
        {
            eprintln!("Overriding previous entry at {}", Utc.timestamp(t, 0));
        }
        conn.execute(
            "insert or replace into update_log (time, status, message) values (?1, ?2, ?3)",
            rusqlite::params![
                entry.time.timestamp(),
                (if entry.succeed { 0 } else { 1 }),
                entry.message,
            ],
        )?;
        Ok(())
    }
}

/// Insert channel into the database return assigned id
fn insert_channel(conn: &Connection, alias: &str, name: &str, icon_url: &str) -> Result<i64> {
    let mut stmt =
        conn.prepare_cached("insert into channels (alias, name, icon_url) values (?1, ?2, ?3)")?;
    let row_id = stmt.insert(&[
        &alias as &dyn ToSql,
        &name as &dyn ToSql,
        &icon_url as &dyn ToSql,
    ])?;
    Ok(row_id)
}

/// Insert or replace channel data in the database
fn update_channel(
    conn: &Connection,
    id: i64,
    alias: &str,
    name: &str,
    icon_url: &str,
) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "insert or replace into channels (id, alias, name, icon_url) values (?1, ?2, ?3, ?4)",
    )?;
    let row_id = stmt.insert(&[
        &id,
        &alias as &dyn ToSql,
        &name as &dyn ToSql,
        &icon_url as &dyn ToSql,
    ])?;
    assert_eq!(row_id, id);
    Ok(())
}

fn insert_program(conn: &Connection, channel_id: i64, program: &Program) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "insert into programs1 (channel, begin, end, title, description) \
         values (?1, ?2, ?3, ?4, ?5)",
    )?;
    stmt.execute(&[
        &channel_id,
        &program.begin,
        &program.end,
        &program.title as &dyn ToSql,
        &program.description as &dyn ToSql,
    ])?;
    Ok(())
}

fn create_indexes(conn: &Connection) -> Result<()> {
    conn.execute("create index channel on programs (channel)", NO_PARAMS)?;
    conn.execute(
        "create index channel_begin on programs (channel, begin)",
        NO_PARAMS,
    )?;
    conn.execute(
        "create index channel_end on programs (channel, end)",
        NO_PARAMS,
    )?;

    Ok(())
}

fn drop_indexes(conn: &Connection) -> Result<()> {
    conn.execute("drop index if exists channel", NO_PARAMS)?;
    conn.execute("drop index if exists channel_begin", NO_PARAMS)?;
    conn.execute("drop index if exists channel_end", NO_PARAMS)?;
    Ok(())
}

fn append_programs(conn: &mut Connection) -> Result<()> {
    conn.execute("create index p1_channel on programs1 (channel)", NO_PARAMS)?;

    let channels = {
        let mut stmt = conn.prepare("select distinct p1.channel from programs1 p1")?;
        let it = stmt
            .query_map(NO_PARAMS, |row| {
                let c: Result<i64> = row.get(0);
                c
            })?
            .filter_map(|item| item.ok());
        it.collect::<Vec<_>>()
    };
    {
        // Remove programs from database, which times conflict with new data
        let mut total = 0;
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "delete from programs where programs.channel=?1 and
                 programs.begin >= (select min(p1.begin) from programs1 p1 where p1.channel=?1)",
            )?;
            for id in channels.iter() {
                let count = stmt.execute(&[&id])?;
                total += count;
            }
        }
        println!("Deleted {} conflicting programs from sql database", total);

        // Drop indexes to speed up insert
        drop_indexes(&tx)?;
        // Copy new data into the database
        total = tx.execute(
            "insert into programs (channel, begin, end, title, description)
             select channel, \"begin\", \"end\", title, description from programs1",
            NO_PARAMS,
        )?;
        create_indexes(&tx)?;
        println!("Inserted {} new programs", total);

        tx.commit()?;
    }

    conn.execute("delete from programs1", NO_PARAMS)?;
    Ok(())
}

/// Remove channels with no programs
fn clear_channels(conn: &Connection) -> Result<()> {
    println!("Clearing channels without epg data");
    let count = conn.execute(
        "delete from channels where \
         (select count(id) from programs where programs.channel=channels.id)=0",
        NO_PARAMS,
    )?;
    println!("Removed {} rows.", count);
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Database tests run not in parallel to avoid conflicts
    use crate::db::*;
    use crate::epg::ChannelInfo;
    use crate::epg::Program;
    use rusqlite::Connection;
    use serial_test::serial;
    use std::fs;
    use std::path::Path;

    /// Wrapper for `update_channel`
    fn update_channel_info(conn: &Connection, id: i64, channel: &ChannelInfo) -> Result<()> {
        update_channel(conn, id, &channel.alias, &channel.name, &channel.icon_url)
    }

    fn open_db() -> ProgramsDatabase {
        if Path::new("test.db").exists() {
            fs::remove_file("test.db").unwrap();
        }
        ProgramsDatabase::open("test.db").unwrap()
    }

    #[test]
    #[serial]
    fn test_database() {
        let db = open_db();
        let mut conn = Connection::open(&db.file).unwrap();

        update_channel_info(
            &conn,
            1,
            &ChannelInfo {
                alias: "c1".to_string(),
                name: "ch1".to_string(),
                icon_url: String::new(),
            },
        )
        .unwrap();
        update_channel_info(
            &conn,
            2,
            &ChannelInfo {
                alias: "c2".to_string(),
                name: "ch2".to_string(),
                icon_url: String::new(),
            },
        )
        .unwrap();
        update_channel_info(
            &conn,
            3,
            &ChannelInfo {
                alias: "c3".to_string(),
                name: "ch3".to_string(),
                icon_url: String::new(),
            },
        )
        .unwrap();

        let channels = db.get_channels().unwrap();
        assert_eq!(
            channels.iter().map(|&(id, _)| id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );

        for program in vec![
            Program {
                begin: 10,
                end: 20,
                title: String::from("a"),
                description: String::new(),
            },
            Program {
                begin: 20,
                end: 25,
                title: String::from("b"),
                description: String::new(),
            },
            Program {
                begin: 25,
                end: 40,
                title: String::from("c"),
                description: String::new(),
            },
        ] {
            insert_program(&conn, 1, &program).unwrap();
        }
        for program in vec![
            Program {
                begin: 6,
                end: 17,
                title: String::from("x"),
                description: String::new(),
            },
            Program {
                begin: 17,
                end: 30,
                title: String::from("y"),
                description: String::new(),
            },
            Program {
                begin: 30,
                end: 50,
                title: String::from("z"),
                description: String::new(),
            },
        ] {
            insert_program(&conn, 2, &program).unwrap();
        }
        append_programs(&mut conn).unwrap();

        let t = 10;
        let result = db.get_at(t, 2).unwrap();
        {
            let mut ids = result.values().map(|r| r.channel_id).collect::<Vec<_>>();
            ids.sort();
            assert_eq!(ids, vec![1, 2]);
        }

        for r in db.get_at(10, 2).unwrap().values() {
            println!("{:?}", r);
            assert!(r.programs.len() <= 2);
            let p1 = r.programs.first().unwrap();
            assert!(p1.begin <= t && t < p1.end);
            let p2 = r.programs.iter().take(2).last().unwrap();
            assert!(p2.begin > t);
        }
    }

    #[test]
    #[serial]
    fn test_update_log() {
        let db = open_db();
        assert_eq!(db.get_last_update().unwrap(), None);

        let day = Utc.ymd(2021, 2, 21);

        let st1 = UpdateStatus::new_ok(day.and_hms(10, 10, 0));
        db.insert_update_status(st1.clone()).unwrap();
        assert_eq!(db.get_last_update().unwrap(), Some(st1));

        let st2 = UpdateStatus::new_fail(day.and_hms(10, 15, 0), "failure message".to_owned());
        db.insert_update_status(st2.clone()).unwrap();
        assert_eq!(db.get_last_update().unwrap(), Some(st2));

        let st3 =
            UpdateStatus::new_fail(day.and_hms(10, 15, 0), "another failure message".to_owned());
        db.insert_update_status(st3.clone()).unwrap();
        assert_eq!(db.get_last_update().unwrap(), Some(st3));
    }
}
