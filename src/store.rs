use bson::{from_bson, to_bson, Bson, Document};
use epg::{Channel, Program};
use mongodb::db::ThreadedDatabase;
use mongodb::error::Error as MongoError;
use mongodb::{bson, doc};
use mongodb::{Client, ThreadedClient};
use std::collections::HashMap;
use std::env;
use std::iter::FromIterator;

fn create_client() -> Result<Client, MongoError> {
    let client: Client = Client::connect("localhost", 27017).unwrap();
    let db = client.db("epg"); // Database with credentials
    let password = env::var("MONGO_PASS").unwrap_or("test".to_string());
    db.auth("rust", &password).unwrap();
    Ok(client)
}

/// Removes all programs with starting date before `timestamp`.
pub fn remove_before(timestamp: i64) -> Result<(), MongoError> {
    println!("Removing programs before t={} from mongodb ...", timestamp);
    let client = create_client().unwrap();
    let coll = client.db("epg").collection("channels");
    let result = coll
        .update_many(
            doc! {},
            doc! {
                "$pull" : {"programs" : {"begin": {"$lt": timestamp}}}
            },
            None,
        )
        .unwrap();
    println!("mongo {:?}", result);
    Ok(())
}

pub fn save_to_db(channels: &HashMap<i64, Channel>) -> Result<(), MongoError> {
    println!("Serializing channels to mongodb ...");
    let coll = create_client()?.db("epg").collection("channels");
    for channel in channels.values() {
        let serialized = to_bson(&channel)?;
        if let Bson::Document(document) = serialized {
            coll.insert_one(document, None)?;
        }
    }
    println!("{} channels saved to db!", channels.len());
    Ok(())
}

/// Loads all channels from the database.
pub fn load_db() -> Result<HashMap<i64, Channel>, MongoError> {
    println!("Loading channels from mongodb ...");
    let client = create_client()?;
    let coll = client.db("epg").collection("channels");
    let cursor = coll.find(None, None)?;
    let channels = HashMap::from_iter(cursor.filter_map(|item: Result<Document, MongoError>| {
        item.ok()
            .and_then(|doc| from_bson::<Channel>(Bson::Document(doc)).ok())
            .map(|channel| (channel.id, channel))
    }));
    println!("Loaded {} channels from db", channels.len());
    Ok(channels)
}

#[cfg(test)]
mod tests {
    use epg::Channel;
    use epg::Program;
    use std::collections::HashMap;
    use store::{load_db, remove_before, save_to_db};

    fn sample_data() -> HashMap<i64, Channel> {
        let data: HashMap<i64, Channel> = [
            Channel {
                id: 1,
                name: String::from("ch1"),
                icon_url: String::new(),
                programs: vec![
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
                ],
            },
            Channel {
                id: 2,
                name: String::from("ch2"),
                icon_url: String::new(),
                programs: vec![
                    Program {
                        begin: 100,
                        end: 300,
                        title: String::from("p one"),
                        description: String::new(),
                    },
                    Program {
                        begin: 300,
                        end: 400,
                        title: String::from("p two"),
                        description: String::new(),
                    },
                ],
            },
        ]
        .iter()
        .cloned()
        .map(|item| (item.id, item))
        .collect();
        data
    }

    #[test]
    fn mongodb() {
        let data = sample_data();
        save_to_db(&data).unwrap();

        remove_before(21).unwrap();

        let data = load_db().unwrap();
        println!("Loaded {:?}", data);

        let ch1 = data.get(&1).unwrap();
        assert_eq!(ch1.id, 1);
        assert_eq!(ch1.name, "ch1");
        assert_eq!(ch1.programs.len(), 1);
        assert_eq!(ch1.programs.first().unwrap().begin, 25);

        let ch2 = data.get(&2).unwrap();
        assert_eq!(ch2.id, 2);
        assert_eq!(ch2.name, "ch2");
        assert_eq!(ch2.programs, data.get(&2).unwrap().programs);
    }
}
