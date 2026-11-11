//! The journal of a crater's changes, so a server that stops without restoring it can still put
//! the land back when it starts again.
//!
//! One file for each crater, `<world>/sculkstorm/pending-restores/<id>.log`: a header line naming
//! the dimension, then a line for every block the crater changed, in the order it changed them:
//! `x y z <placed block> <original state> .`. The closing `.` is how a line cut short by a crash
//! is told from a whole one. A single thread writes, so lines land in order.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};

/// A block the crater changed: where, what it put there and what it replaced (as `/setblock`
/// writes a state: `minecraft:oak_log[axis=y]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub pos: [i32; 3],
    pub placed: String,
    pub original: String,
}

/// The journal's header for a crater in `dimension`.
pub fn header(dimension: &str) -> String {
    format!("dimension {dimension}\n")
}

/// The line for one change.
pub fn line(pos: [i32; 3], placed: &str, original: &str) -> String {
    format!("{} {} {} {placed} {original} .\n", pos[0], pos[1], pos[2])
}

/// A journal's dimension and its whole lines, in order. A line cut short (no closing `.`, too few
/// words, a number that is not one) is dropped, as is anything after a header that is not one:
/// None for a file with no header.
pub fn parse(text: &str) -> Option<(String, Vec<Record>)> {
    let mut lines = text.lines();
    let dimension = lines.next()?.strip_prefix("dimension ")?.trim();
    if dimension.is_empty() {
        return None;
    }
    let records = lines
        .filter_map(|line| {
            let words: Vec<&str> = line.split(' ').collect();
            let [x, y, z, placed, original, "."] = words.as_slice() else {
                return None;
            };
            Some(Record {
                pos: [x.parse().ok()?, y.parse().ok()?, z.parse().ok()?],
                placed: (*placed).to_owned(),
                original: (*original).to_owned(),
            })
        })
        .collect();
    Some((dimension.to_owned(), records))
}

/// The folder a world keeps its journals in.
pub fn dir(world: &Path) -> PathBuf {
    world.join("sculkstorm").join("pending-restores")
}

enum Message {
    Append(PathBuf, String),
    Delete(PathBuf),
    /// Answers once everything before it is on disk.
    Fence(Sender<()>),
}

/// The thread that writes journals. It starts with the first message.
pub struct Writer {
    sender: OnceLock<Mutex<Sender<Message>>>,
}

impl Writer {
    pub const fn new() -> Self {
        Self {
            sender: OnceLock::new(),
        }
    }

    fn send(&self, message: Message) {
        let sender = self.sender.get_or_init(|| {
            let (tx, rx) = channel::<Message>();
            let spawned = std::thread::Builder::new()
                .name("Sculkstorm-journal".into())
                .spawn(move || {
                    for message in rx {
                        match message {
                            Message::Append(file, text) => {
                                if let Err(e) = append(&file, &text) {
                                    log::warn!(
                                        "Sculkstorm: could not write the crater journal {}: {e}",
                                        file.display()
                                    );
                                }
                            }
                            Message::Delete(file) => {
                                let _ = std::fs::remove_file(file);
                            }
                            Message::Fence(done) => {
                                let _ = done.send(());
                            }
                        }
                    }
                });
            if let Err(e) = spawned {
                log::warn!("Sculkstorm: could not start the journal thread: {e}");
            }
            Mutex::new(tx)
        });
        if let Ok(sender) = sender.lock() {
            let _ = sender.send(message);
        }
    }

    pub fn append(&self, file: &Path, text: String) {
        if !text.is_empty() {
            self.send(Message::Append(file.to_path_buf(), text));
        }
    }

    /// Deletes a journal, on the thread that writes them, so a late write cannot bring it back.
    pub fn delete(&self, file: &Path) {
        self.send(Message::Delete(file.to_path_buf()));
    }

    /// Waits until everything sent so far is on disk (or the thread is gone).
    pub fn fence(&self) {
        let (tx, rx) = channel();
        self.send(Message::Fence(tx));
        let _ = rx.recv_timeout(std::time::Duration::from_secs(10));
    }
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

fn append(file: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?
        .write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_journal_reads_back_what_was_written() {
        let mut text = header("minecraft:overworld");
        text += &line([10, 64, -3], "minecraft:air", "minecraft:grass_block");
        text += &line([10, 63, -3], "minecraft:sculk", "minecraft:dirt");
        text += &line([-5, 70, 0], "minecraft:air", "minecraft:oak_log[axis=y]");
        let (dimension, records) = parse(&text).unwrap();
        assert_eq!(dimension, "minecraft:overworld");
        assert_eq!(
            records,
            vec![
                Record {
                    pos: [10, 64, -3],
                    placed: "minecraft:air".into(),
                    original: "minecraft:grass_block".into()
                },
                Record {
                    pos: [10, 63, -3],
                    placed: "minecraft:sculk".into(),
                    original: "minecraft:dirt".into()
                },
                Record {
                    pos: [-5, 70, 0],
                    placed: "minecraft:air".into(),
                    original: "minecraft:oak_log[axis=y]".into()
                },
            ]
        );
    }

    #[test]
    fn a_line_a_crash_cut_short_is_dropped_and_the_whole_ones_before_it_stay() {
        let mut text = header("minecraft:the_nether");
        text += &line([1, 2, 3], "minecraft:air", "minecraft:netherrack");
        text += &line([4, 5, 6], "minecraft:air", "minecraft:netherrack");
        // The write stopped in the middle of the next line, at every place it could.
        let whole = line([7, 8, 9], "minecraft:sculk", "minecraft:netherrack");
        for cut in 1..whole.len() - 1 {
            let torn = format!("{text}{}", &whole[..cut]);
            let (_, records) = parse(&torn).unwrap();
            assert_eq!(records.len(), 2, "cut at {cut}: {torn:?}");
        }
        assert_eq!(parse(&format!("{text}{whole}")).unwrap().1.len(), 3);
    }

    #[test]
    fn nonsense_in_a_line_drops_it_and_a_file_without_a_header_is_none() {
        let mut text = header("minecraft:overworld");
        text += "x 1 2 minecraft:air minecraft:stone .\n";
        text += "1 2 3 minecraft:air .\n";
        text += "1 2 3 minecraft:air minecraft:stone extra .\n";
        text += &line([1, 2, 3], "minecraft:air", "minecraft:stone");
        assert_eq!(parse(&text).unwrap().1.len(), 1);
        assert_eq!(parse("1 2 3 minecraft:air minecraft:stone .\n"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("dimension \n"), None);
        assert_eq!(parse(&header("minecraft:the_end")).unwrap().1, vec![]);
    }

    #[test]
    fn the_writer_appends_in_order_and_deletes_after_its_writes() {
        let world = std::env::temp_dir().join(format!("sculkstorm-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&world);
        let file = dir(&world).join("a.log");
        let writer = Writer::new();
        writer.append(&file, header("minecraft:overworld"));
        for i in 0..200 {
            writer.append(&file, line([i, 0, 0], "minecraft:air", "minecraft:stone"));
        }
        writer.fence();
        let (dimension, records) = parse(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(dimension, "minecraft:overworld");
        assert_eq!(records.len(), 200);
        assert!(
            records
                .iter()
                .enumerate()
                .all(|(i, r)| r.pos[0] == i as i32)
        );
        writer.append(&file, line([999, 0, 0], "minecraft:air", "minecraft:stone"));
        writer.delete(&file);
        writer.fence();
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&world);
    }
}
