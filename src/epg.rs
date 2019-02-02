use chrono::prelude::*;
use std::fmt;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Program {
    pub begin: i64,
    pub end: i64,
    pub title: String,
    pub description: String,
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

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Channel {
    #[serde(rename = "_id")]
    pub id: i64,
    pub name: String,
    pub icon_url: String,
    pub programs: Vec<Program>,
}

impl Channel {
    pub fn sort_programs(&mut self) {
        self.programs.sort_by(|a, b| a.begin.cmp(&b.begin));
    }

    pub fn prepend_old_programs(&mut self, programs: &[Program]) {
        let before = self
            .programs
            .first()
            .map(|p| p.begin)
            .unwrap_or(i64::max_value());
        let index = programs
            .binary_search_by_key(&before, |p| p.begin)
            .unwrap_or_else(|i| i);
        // TODO: overlap check
        let mut result = programs[0..index].to_vec();
        result.append(&mut self.programs);
        self.programs = result;
    }

    pub fn insert_one(&mut self, program: Program) {
        let index = self
            .programs
            .binary_search_by_key(&program.begin, |p| p.begin)
            .unwrap_or_else(|i| i);
        // TODO: overlap checks
        self.programs.insert(index, program);
    }

    pub fn programs_range(&self, from: i64, to: i64) -> &[Program] {
        let index_from = self
            .programs
            .binary_search_by(|p| p.begin.cmp(&from))
            .unwrap_or_else(|i| i);

        let index_to = self
            .programs
            .binary_search_by(|p| p.begin.cmp(&to))
            .unwrap_or_else(|i| i);

        &self.programs[index_from..index_to]
    }

    pub fn programs_at(&self, from: i64, count: usize) -> &[Program] {
        let idx = self
            .programs
            .binary_search_by(|p| p.begin.cmp(&from))
            .unwrap_or_else(|i| i);
        use std::cmp;
        if idx > 0 {
            let a = idx - 1;
            let b = cmp::min(a + count, self.programs.len());
            assert!(a < self.programs.len());
            if self.programs[a].end >= from {
                return &self.programs[a..b];
            }
        }
        return &[];
    }
}

#[cfg(test)]
mod tests {
    use epg::Channel;
    use epg::Program;

    fn sample_channel() -> Channel {
        Channel {
            id: 0,
            name: String::new(),
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
        }
    }

    #[test]
    fn channel_programs_at() {
        let channel = sample_channel();
        {
            let programs = channel.programs_at(15, 2);
            assert_eq!(programs.len(), 2);
            assert_eq!(programs[0].title, "a");
            assert_eq!(programs[1].title, "b");
        }
        {
            let programs = channel.programs_at(21, 2);
            assert_eq!(programs.len(), 2);
            assert_eq!(programs[0].title, "b");
            assert_eq!(programs[1].title, "c");
        }
        {
            let programs = channel.programs_at(0, 1);
            assert_eq!(programs.len(), 0);
        }
        {
            let programs = channel.programs_at(100, 1);
            assert_eq!(programs.len(), 0);
        }
    }

    #[test]
    fn channel_insert_one() {
        {
            let mut channel = sample_channel();
            channel.insert_one(Program {
                begin: 45,
                end: 50,
                title: String::from("x"),
                description: String::new(),
            });
            assert_eq!(channel.programs[3].title, "x")
        }
        {
            let mut channel = sample_channel();
            channel.insert_one(Program {
                begin: 0,
                end: 10,
                title: String::from("x"),
                description: String::new(),
            });
            assert_eq!(channel.programs[0].title, "x")
        }
    }

    #[test]
    fn channel_prepend() {
        {
            let mut channel = sample_channel();
            channel.prepend_old_programs(&[
                Program {
                    begin: 0,
                    end: 5,
                    title: String::from("x"),
                    description: String::new(),
                },
                Program {
                    begin: 5,
                    end: 10,
                    title: String::from("y"),
                    description: String::new(),
                },
            ]);
            assert_eq!(
                channel
                    .programs
                    .iter()
                    .map(|p| p.clone().title)
                    .collect::<Vec<_>>(),
                ["x", "y", "a", "b", "c"]
            );
        }
        {
            let mut channel = sample_channel();
            channel.prepend_old_programs(&[
                Program {
                    begin: 6,
                    end: 11,
                    title: String::from("x"),
                    description: String::new(),
                },
                Program {
                    begin: 10,
                    end: 12,
                    title: String::from("y"),
                    description: String::new(),
                },
            ]);
            assert_eq!(
                channel
                    .programs
                    .iter()
                    .map(|p| p.clone().title)
                    .collect::<Vec<_>>(),
                ["x", "a", "b", "c"]
            );
        }
    }

    //    #[test]
    //    fn channel_programs_range() {
    //        panic!("Make this test fail");
    //    }
}
