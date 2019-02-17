use epg::{ChannelInfo, EpgNow, Program};
use rusqlite::types::ToSql;
use rusqlite::{Connection, Result, NO_PARAMS};
use std::collections::HashMap;

pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute(
        "create table if not exists channels \
         (id integer primary key, name text, icon_url text)",
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
    Ok(())
}

pub fn get_channels(conn: &Connection) -> Result<Vec<ChannelInfo>> {
    let mut stmt = conn
        .prepare("select id, name, icon_url from channels")
        .unwrap();
    let it = stmt
        .query_map(NO_PARAMS, |row| ChannelInfo {
            id: row.get(0),
            name: row.get(1),
            icon_url: row.get(2),
        })?
        .filter_map(|item| item.ok());
    Ok(it.collect::<Vec<_>>())
}

pub fn get_at(conn: &Connection, timestamp: i64, count: i64) -> Result<Vec<EpgNow>> {
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
        let id: i64 = row.get(0);
        let program = Program {
            begin: row.get(1),
            end: row.get(2),
            title: row.get(3),
            description: row.get(4),
        };
        (id, program)
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
    Ok(hash.into_iter().map(|(_id, value)| value).collect())
}

pub fn get_range(conn: &Connection, id: i64, from: i64, to: i64) -> Result<Vec<Program>> {
    let mut stmt = conn.prepare(
        "select programs.begin, programs.end, programs.title, programs.description
         from programs where
         programs.channel = ?1 and programs.begin >= ?2 and programs.begin < ?3",
    )?;
    let it = stmt
        .query_map(&[&id, &from, &to], |row| Program {
            begin: row.get(0),
            end: row.get(1),
            title: row.get(2),
            description: row.get(3),
        })?
        .filter_map(|item| item.ok());
    Ok(it.collect::<Vec<_>>())
}

pub fn insert_channel(conn: &Connection, channel: &ChannelInfo) -> Result<()> {
    conn.execute(
        "insert or replace into channels (id, name, icon_url) \
         values (?1, ?2, ?3)",
        &[
            &channel.id,
            &channel.name as &ToSql,
            &channel.icon_url as &ToSql,
        ],
    )?;
    Ok(())
}

pub fn insert_program(conn: &Connection, channel: i64, program: &Program) -> Result<()> {
    conn.execute(
        "insert into programs1 (channel, begin, end, title, description) \
         values (?1, ?2, ?3, ?4, ?5)",
        &[
            &channel,
            &program.begin,
            &program.end,
            &program.title as &ToSql,
            &program.description as &ToSql,
        ],
    )?;
    Ok(())
}

pub fn create_indexes(conn: &Connection) -> Result<()> {
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

pub fn drop_indexes(conn: &Connection) -> Result<()> {
    conn.execute("drop index if exists channel", NO_PARAMS)?;
    conn.execute("drop index if exists channel_begin", NO_PARAMS)?;
    conn.execute("drop index if exists channel_end", NO_PARAMS)?;
    Ok(())
}

pub fn clear_programs_tmp(conn: &Connection) -> Result<()> {
    conn.execute("delete from programs1", NO_PARAMS)?;
    Ok(())
}

pub fn append_programs(conn: &mut Connection) -> Result<()> {
    let channels = {
        let mut stmt = conn.prepare("select distinct p1.channel from programs1 p1")?;
        let it = stmt
            .query_map(NO_PARAMS, |row| {
                let c: i64 = row.get(0);
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

        // Copy new data into the database
        total = tx.execute(
            "insert into programs (channel, begin, end, title, description)
             select channel, \"begin\", \"end\", title, description from programs1",
            NO_PARAMS,
        )?;
        println!("Inserted {} new programs", total);

        tx.commit()?;
    }
    clear_programs_tmp(&conn)?;
    Ok(())
}

pub fn delete_before(conn: &Connection, timestamp: i64) -> Result<()> {
    println!("Removing programs before t={} from sqlite ...", timestamp);
    let count = conn.execute(
        "delete from programs where programs.end < ?1",
        &[&timestamp],
    )?;
    println!("Deleted {} rows.", count);
    Ok(())
}

#[cfg(test)]
mod tests {
    use db::*;
    use epg::ChannelInfo;
    use epg::Program;
    use rusqlite::Connection;
    use std::fs;

    #[test]
    fn test_database() {
        fs::remove_file("test.db").unwrap();
        let mut db = Connection::open("test.db").unwrap();
        create_tables(&db).unwrap();

        insert_channel(
            &db,
            &ChannelInfo {
                id: 1,
                name: "ch1".to_string(),
                icon_url: String::new(),
            },
        )
        .unwrap();
        insert_channel(
            &db,
            &ChannelInfo {
                id: 2,
                name: "ch2".to_string(),
                icon_url: String::new(),
            },
        )
        .unwrap();
        insert_channel(
            &db,
            &ChannelInfo {
                id: 3,
                name: "ch3".to_string(),
                icon_url: String::new(),
            },
        )
        .unwrap();

        let channels = get_channels(&db).unwrap();
        assert_eq!(
            channels.iter().map(|c| c.id).collect::<Vec<_>>(),
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
            insert_program(&db, 1, &program).unwrap();
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
            insert_program(&db, 2, &program).unwrap();
        }
        append_programs(&mut db).unwrap();

        let t = 10;
        let result = get_at(&db, t, 2).unwrap();
        {
            let mut ids = result.iter().map(|r| r.channel_id).collect::<Vec<_>>();
            ids.sort();
            assert_eq!(ids, vec![1, 2]);
        }

        for r in get_at(&db, 10, 2).unwrap().iter() {
            println!("{:?}", r);
            assert!(r.programs.len() <= 2);
            let p1 = r.programs.first().unwrap();
            assert!(p1.begin <= t && t < p1.end);
            let p2 = r.programs.iter().take(2).last().unwrap();
            assert!(p2.begin > t);
        }
    }
}
