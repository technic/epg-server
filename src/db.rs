use crate::epg::{ChannelInfo, EpgNow, Program};
use crate::xmltv::XmltvItem;
use crate::xmltv::XmltvReader;
use chrono::prelude::*;
use failure::Fail;
use mysql::prelude::*;
use mysql::{
    params, Opts, Params as DBParams, Pool, PooledConn as Connection, Result as DBResult,
    Value as DBValue,
};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::io::BufRead;
use std::time::SystemTime;

pub struct ProgramsDatabase {
    pool: Pool,
}

struct BatchInserter {
    batch_size: u32,
    cols: Vec<String>,
    base_stmt: String,
    row_stmt: String,
    accumulated: u32,
    stmt: String,
    params: Vec<DBValue>,
}

impl BatchInserter {
    pub fn new(table: &str, cols: &[&str], batch_size: u32) -> Self {
        let base_stmt = format!("INSERT INTO {} ({}) VALUES ", table, cols.join(","));
        let row_stmt = format!(
            "({})",
            cols.iter().map(|_| "?").collect::<Vec<_>>().join(",")
        );
        Self {
            batch_size,
            cols: cols.iter().map(|&s| s.to_owned()).collect(),
            base_stmt: base_stmt.clone(),
            row_stmt,
            accumulated: 0,
            stmt: base_stmt,
            params: Vec::new(),
        }
    }

    pub fn insert<Q, P>(&mut self, conn: &mut Q, params: P) -> DBResult<()>
    where
        Q: Queryable,
        P: Into<mysql::Params>,
    {
        if !self.params.is_empty() {
            self.stmt.push(',');
        }
        self.stmt.push_str(&self.row_stmt);
        match params.into().into_positional(&self.cols).unwrap() {
            DBParams::Positional(vec) => {
                self.params.extend(vec.into_iter());
            }
            _ => unreachable!(),
        }
        self.accumulated += 1;

        if self.accumulated >= self.batch_size {
            return self.execute(conn);
        }
        Ok(())
    }

    fn execute<Q: Queryable>(&mut self, conn: &mut Q) -> DBResult<()> {
        let stmt = conn.prep(&self.stmt)?;
        let mut tmp = Vec::new();
        std::mem::swap(&mut tmp, &mut self.params);
        conn.exec_drop(stmt, tmp)?;

        // reset:
        self.accumulated = 0;
        self.stmt = self.base_stmt.clone();

        Ok(())
    }
}

impl ProgramsDatabase {
    pub fn open(url: &str) -> DBResult<Self> {
        let opts = Opts::from(url);
        let pool = Pool::new(opts.clone())?;

        let db = Self { pool };

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

        db.run_migrations(&opts)
            .or_else(|e| {
                if e.is_migration_complete() {
                    println!("All migrations complete!");
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .unwrap();
        // .map_err(|e| {
        //     println!("Migration failed: {}", e.display_chain());
        //     // rusqlite::Error::UserFunctionError(Box::new(MigrantError {
        //     //     message: e.description().to_string(),
        //     // }))
        //     // FIXME!!!!
        //     panic!(e.description());
        // })?;
        Ok(db)
    }

    fn run_migrations(&self, opts: &Opts) -> std::result::Result<(), migrant_lib::errors::Error> {
        let mut builder = migrant_lib::Settings::configure_mysql();
        builder
            .database_host(&opts.get_ip_or_hostname())
            .database_port(opts.get_tcp_port());
        if let Some(name) = opts.get_db_name() {
            builder.database_name(name);
        }
        if let Some(user) = opts.get_user() {
            builder.database_user(user);
        }
        if let Some(pass) = opts.get_pass() {
            builder.database_password(pass);
        }
        let settings = builder.build()?;

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
        config.use_migrations(&[make_migration!("20200503211649_create")])?;
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

    pub fn load_xmltv<R: BufRead>(&self, xmltv: XmltvReader<R>) -> DBResult<()> {
        let mut conn = self.pool.get_conn()?;
        println!("{} Start", chrono::Local::now());

        // Make sure that temporary storage is clean
        conn.query_drop("drop index if exists p1_channel on programs1")?;
        conn.query_drop("delete from programs1")?;

        let mut ids: HashMap<String, i64> = self
            .get_channels()?
            .into_iter()
            .map(|(id, info)| (info.alias, id))
            .collect();

        let mut ins_c = 0;
        let mut ins_p = 0;
        let mut result = Ok(());
        let t = SystemTime::now();
        println!("Parsing XMLTV entries into database ...");
        // Convert xmltv into sql table
        conn.query_drop("LOCK TABLES `programs1` WRITE")?;
        {
            let mut tx = conn.start_transaction(mysql::TxOpts::default())?;

            let mut b = BatchInserter::new(
                "programs1",
                &["channel", "begin", "end", "title", "description"],
                20,
            );

            for item in xmltv {
                match item {
                    Ok(XmltvItem::Channel(channel)) => {
                        match ids.entry(channel.alias) {
                            Entry::Occupied(entry) => {
                                // Chanel with this alias already exists
                                let &id = entry.get();
                                update_channel(
                                    &mut tx,
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
                                        &mut tx,
                                        id,
                                        entry.key(),
                                        &channel.name,
                                        &channel.icon_url,
                                    )?;
                                    entry.insert(id);
                                } else {
                                    // Insert new channel and assign it new id
                                    let id = insert_channel(
                                        &mut tx,
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
                            b.insert(
                                &mut tx,
                                (
                                    id,
                                    program.begin,
                                    program.end,
                                    &program.title,
                                    &program.description,
                                ),
                            )?;
                            ins_p += 1;
                        } else {
                            eprintln!("Skip program for unknown channel {}", alias);
                        }
                    }
                    Err(e) => {
                        // Process all parsed items and return Error in the end
                        // result = Err(e);
                        // result = Err(rusqlite::Error::UserFunctionError(Box::new(e.compat())));
                        panic!(e.compat());
                        break;
                    }
                }
                if ins_p % 10000 == 1 {
                    println!("{} {}", chrono::Local::now(), ins_p);
                }
            }
            tx.commit()?;
        }

        print!("{} ", chrono::Local::now());
        println!(
            "Loaded {} channels and {} programs into SQL database in {}s",
            ins_c,
            ins_p,
            t.elapsed().unwrap().as_secs_f32()
        );

        // Clear old epg entries from the database
        let time = Utc::now().naive_utc() - chrono::Duration::days(20);
        self.delete_before(time.timestamp()).unwrap();
        // Merge new programs data into database
        append_programs(&mut conn)?;
        // Clean up obsolete channels
        clear_channels(&mut conn)?;
        conn.query_drop("UNLOCK TABLES")?;
        result
    }

    pub fn get_channels(&self) -> DBResult<Vec<(i64, ChannelInfo)>> {
        let mut conn = self.pool.get_conn()?;
        let result = conn.query_map(
            "select id, alias, name, icon_url from channels",
            |(id, alias, name, icon_url)| {
                (
                    id,
                    ChannelInfo {
                        alias,
                        name,
                        icon_url,
                    },
                )
            },
        )?;
        Ok(result)
        // let it = conn
        //     .exec_iter(stmt, ())?
        //     .map(|res| {
        //         res.and_then(|row| {
        //             (
        //                 {
        //                     let id: i64 = row.get(0)?;
        //                     id
        //                 },
        //                 ChannelInfo {
        //                     alias: row.get(1)?,
        //                     name: row.get(2)?,
        //                     icon_url: row.get(3)?,
        //                 },
        //             )
        //         })
        //     })
        //     .filter_map(|item| item.ok());
        // Ok(it.collect::<Vec<_>>())
    }

    pub fn get_at(&self, timestamp: i64, count: i64) -> DBResult<Vec<EpgNow>> {
        let mut conn = self.pool.get_conn()?;
        let stmt = conn.prep(
            "select
                channels.id,
                programs.begin, programs.end, programs.title, programs.description
             from channels
             join programs on programs.id in
             (select programs.id from programs where
              programs.channel=channels.id AND programs.end > :ts order by programs.end limit :count)",
        )?;

        let mut hash: HashMap<i64, EpgNow> = HashMap::new();

        let it = conn.exec_map(
            stmt,
            params! {"ts"=> timestamp, count},
            |(id, begin, end, title, description)| {
                (
                    id,
                    Program {
                        begin,
                        end,
                        title,
                        description,
                    },
                )
            },
        )?;

        for (id, program) in it.into_iter() {
            hash.entry(id)
                .or_insert(EpgNow {
                    channel_id: id,
                    programs: Vec::new(),
                })
                .programs
                .push(program);
        }
        Ok(hash.into_iter().map(|(_id, value)| value).collect())
    }

    pub fn get_range(&self, id: i64, from: i64, to: i64) -> DBResult<Vec<Program>> {
        let mut conn = self.pool.get_conn()?;
        let stmt = conn.prep(
            "select programs.begin, programs.end, programs.title, programs.description
         from programs where
         programs.channel = :id and programs.begin >= :from and programs.begin < :to",
        )?;
        let res = conn.exec_map(
            stmt,
            params! { id, from, to },
            |(begin, end, title, description)| Program {
                begin,
                end,
                title,
                description,
            },
        )?;
        Ok(res)
    }

    pub fn delete_before(&self, timestamp: i64) -> DBResult<()> {
        print!("{} ", chrono::Local::now());
        println!("Removing programs before t={} from database ...", timestamp);
        let mut conn = self.pool.get_conn()?;
        conn.exec_drop("delete from programs where programs.end < ?", (timestamp,))?;
        print!("{} ", chrono::Local::now());
        println!("Deleted {} rows.", conn.affected_rows());
        Ok(())
    }
}

/// Insert channel into the database return assigned id
fn insert_channel<Q: Queryable>(
    conn: &mut Q,
    alias: &str,
    name: &str,
    icon_url: &str,
) -> DBResult<i64> {
    let stmt = conn.prep("insert into channels (alias, name, icon_url) values (?, ?, ?)")?;
    let res = conn.exec_iter(stmt, (alias, name, icon_url))?;
    let row_id = res.last_insert_id().unwrap();
    Ok(row_id as i64)
}

/// Insert or replace channel data in the database
fn update_channel<Q: Queryable>(
    conn: &mut Q,
    id: i64,
    alias: &str,
    name: &str,
    icon_url: &str,
) -> DBResult<()> {
    let stmt = conn.prep(
        "insert into channels (id, alias, name, icon_url) values (:id, :alias, :name, :icon_url)
         on duplicate key update alias=:alias, name=:name, icon_url=:icon_url",
    )?;
    conn.exec_drop(stmt, params! {id, alias, name, icon_url})
}

fn insert_program<Q: Queryable>(conn: &mut Q, channel_id: i64, program: &Program) -> DBResult<()> {
    let stmt = conn.prep(
        "insert into programs1 (channel, begin, end, title, description) \
         values (?, ?, ?, ?, ?)",
    )?;
    conn.exec_drop(
        stmt,
        (
            channel_id,
            program.begin,
            program.end,
            &program.title,
            &program.description,
        ),
    )?;
    Ok(())
}

fn create_indexes<Q: Queryable>(conn: &mut Q) -> DBResult<()> {
    conn.query_drop("create index channel on programs (channel)")?;
    conn.query_drop("create index channel_begin on programs (channel, begin)")?;
    conn.query_drop("create index channel_end on programs (channel, end)")?;
    Ok(())
}

fn drop_indexes<Q: Queryable>(conn: &mut Q) -> DBResult<()> {
    conn.query_drop("drop index if exists channel on programs")?;
    conn.query_drop("drop index if exists channel_begin on programs")?;
    conn.query_drop("drop index if exists channel_end on programs")?;
    Ok(())
}

fn append_programs(conn: &mut Connection) -> DBResult<()> {
    conn.query_drop("create index p1_channel on programs1 (channel)")?;

    let channels: Vec<i64> = {
        let stmt = conn.prep("select distinct p1.channel from programs1 p1")?;
        conn.exec(stmt, ())?
    };
    // {
    // Remove programs from database, which times conflict with new data
    let mut total = 0;
    let mut tx = conn.start_transaction(mysql::TxOpts::default())?;
    //{
    let stmt = tx.prep(
        "delete from programs where programs.channel=:id and
                 programs.begin >= (select min(p1.begin) from programs1 p1 where p1.channel=:id)",
    )?;
    for id in channels.iter() {
        tx.exec_drop(stmt.clone(), params! {id})?;
        total += tx.affected_rows();
    }
    //}
    print!("{} ", chrono::Local::now());
    println!("Deleted {} conflicting programs from sql database", total);

    // Drop indexes to speed up insert
    drop_indexes(&mut tx)?;
    // Copy new data into the database
    tx.query_drop(
        "insert into programs (channel, begin, end, title, description)
             select channel, begin, end, title, description from programs1",
    )?;
    print!("{} ", chrono::Local::now());
    println!("Inserted {} new programs", tx.affected_rows());
    create_indexes(&mut tx)?;
    tx.commit()?;
    println!("{} committed", chrono::Local::now());
    // }

    conn.query_drop("delete from programs1")?;
    Ok(())
}

/// Remove channels with no programs
fn clear_channels(conn: &mut Connection) -> DBResult<()> {
    print!("{} ", chrono::Local::now());
    println!("Clearing channels without epg data");
    conn.query_drop(
        "delete from channels where \
         (select count(id) from programs where programs.channel=channels.id)=0",
    )?;
    print!("{} ", chrono::Local::now());
    println!("Removed {} rows.", conn.affected_rows());
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::db::*;
    use crate::epg::ChannelInfo;
    use crate::epg::Program;
    use std::fs;
    use std::path::Path;

    /// Wrapper for `update_channel`
    fn update_channel_info(conn: &mut Connection, id: i64, channel: &ChannelInfo) -> DBResult<()> {
        update_channel(conn, id, &channel.alias, &channel.name, &channel.icon_url)
    }

    // #[test]
    fn test_database() {
        if Path::new("test.db").exists() {
            fs::remove_file("test.db").unwrap();
        }
        let db = ProgramsDatabase::open("test.db").unwrap();
        let mut conn = db.pool.get_conn().unwrap();

        update_channel_info(
            &mut conn,
            1,
            &ChannelInfo {
                alias: "c1".to_string(),
                name: "ch1".to_string(),
                icon_url: String::new(),
            },
        )
        .unwrap();
        update_channel_info(
            &mut conn,
            2,
            &ChannelInfo {
                alias: "c2".to_string(),
                name: "ch2".to_string(),
                icon_url: String::new(),
            },
        )
        .unwrap();
        update_channel_info(
            &mut conn,
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
            insert_program(&mut conn, 1, &program).unwrap();
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
            insert_program(&mut conn, 2, &program).unwrap();
        }
        append_programs(&mut conn).unwrap();

        let t = 10;
        let result = db.get_at(t, 2).unwrap();
        {
            let mut ids = result.iter().map(|r| r.channel_id).collect::<Vec<_>>();
            ids.sort();
            assert_eq!(ids, vec![1, 2]);
        }

        for r in db.get_at(10, 2).unwrap().iter() {
            println!("{:?}", r);
            assert!(r.programs.len() <= 2);
            let p1 = r.programs.first().unwrap();
            assert!(p1.begin <= t && t < p1.end);
            let p2 = r.programs.iter().take(2).last().unwrap();
            assert!(p2.begin > t);
        }
    }
}
