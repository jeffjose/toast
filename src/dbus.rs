//! Just enough of D-Bus to be a desktop's notification server: the bus
//! a desktop's programs talk to each other over, spoken here by hand, as
//! hotshot speaks it, rather than through a library several times the
//! size of toast. A connection to the session's bus, messages written
//! and read, and the kinds of value they carry.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The bus itself, which is asked for names.
const BUS: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";

/// What a name is asked for with: that another may take it from us, that
/// we take it from whoever lets it be taken, and that we are not to wait
/// in line for it.
pub const ALLOW_REPLACEMENT: u32 = 1;
pub const REPLACE_EXISTING: u32 = 2;
pub const DO_NOT_QUEUE: u32 = 4;

/// A value as a message carries it.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Byte(u8),
    Bool(bool),
    I32(i32),
    U32(u32),
    /// Numbers wider or odder than toast has a use for, kept only so
    /// that a message with one in it can be read past.
    Other(u64),
    Str(String),
    Path(String),
    Sig(String),
    /// Bytes, which are an array like any other but far too many to
    /// keep one at a time.
    Bytes(Vec<u8>),
    /// What each is, as its signature, and the values.
    Array(String, Vec<Value>),
    Struct(Vec<Value>),
    /// One of a dictionary: what it is under, and what it is.
    Entry(Box<Value>, Box<Value>),
    Variant(Box<Value>),
}

impl Value {
    pub fn text(text: &str) -> Value {
        Value::Str(text.to_owned())
    }

    /// A value that says its own kind, which is what a property is.
    pub fn any(value: Value) -> Value {
        Value::Variant(Box::new(value))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(text) | Value::Path(text) | Value::Sig(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Value::I32(number) => Some(*number),
            Value::U32(number) => Some(*number as i32),
            _ => None,
        }
    }

    /// The letters that say what kind of value it is.
    pub fn signature(&self) -> String {
        match self {
            Value::Byte(_) => "y".into(),
            Value::Bool(_) => "b".into(),
            Value::I32(_) => "i".into(),
            Value::U32(_) => "u".into(),
            Value::Other(_) => "t".into(),
            Value::Str(_) => "s".into(),
            Value::Path(_) => "o".into(),
            Value::Sig(_) => "g".into(),
            Value::Bytes(_) => "ay".into(),
            Value::Array(each, _) => format!("a{each}"),
            Value::Struct(parts) => format!("({})", parts.iter().map(Value::signature).collect::<String>()),
            Value::Entry(name, value) => format!("{{{}{}}}", name.signature(), value.signature()),
            Value::Variant(_) => "v".into(),
        }
    }

    /// Written after what is there, at the place its kind has to be at.
    fn write(&self, out: &mut Vec<u8>) {
        pad(out, boundary(self.signature().as_bytes()[0]));
        match self {
            Value::Byte(byte) => out.push(*byte),
            Value::Bool(yes) => out.extend(u32::from(*yes).to_le_bytes()),
            Value::I32(number) => out.extend(number.to_le_bytes()),
            Value::U32(number) => out.extend(number.to_le_bytes()),
            Value::Other(number) => out.extend(number.to_le_bytes()),
            Value::Str(text) | Value::Path(text) => {
                out.extend((text.len() as u32).to_le_bytes());
                out.extend(text.as_bytes());
                out.push(0);
            }
            Value::Sig(text) => {
                out.push(text.len() as u8);
                out.extend(text.as_bytes());
                out.push(0);
            }
            Value::Bytes(bytes) => {
                out.extend((bytes.len() as u32).to_le_bytes());
                out.extend(bytes);
            }
            // How many bytes its values take is said before them, and
            // is not known until they are written.
            Value::Array(each, values) => {
                let length = out.len();
                out.extend(0u32.to_le_bytes());
                pad(out, boundary(each.as_bytes()[0]));
                let began = out.len();
                values.iter().for_each(|value| value.write(out));
                let took = (out.len() - began) as u32;
                out[length..length + 4].copy_from_slice(&took.to_le_bytes());
            }
            Value::Struct(parts) => parts.iter().for_each(|part| part.write(out)),
            Value::Entry(name, value) => {
                name.write(out);
                value.write(out);
            }
            Value::Variant(inner) => {
                Value::Sig(inner.signature()).write(out);
                inner.write(out);
            }
        }
    }
}

/// The multiple of bytes a kind of value has to begin at.
fn boundary(kind: u8) -> usize {
    match kind {
        b'y' | b'g' | b'v' => 1,
        b'n' | b'q' => 2,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 4,
    }
}

fn pad(out: &mut Vec<u8>, to: usize) {
    while out.len() % to != 0 {
        out.push(0);
    }
}

/// How many letters of a signature the first whole kind in it takes.
fn whole(signature: &[u8]) -> Option<usize> {
    match *signature.first()? {
        b'a' => Some(1 + whole(&signature[1..])?),
        open @ (b'(' | b'{') => {
            let close = if open == b'(' { b')' } else { b'}' };
            let mut at = 1;
            while *signature.get(at)? != close {
                at += whole(&signature[at..])?;
            }
            Some(at + 1)
        }
        _ => Some(1),
    }
}

/// Reads values of the kinds a signature says, one after another.
pub fn read(signature: &str, data: &[u8], at: &mut usize) -> Option<Vec<Value>> {
    let mut signature = signature.as_bytes();
    let mut values = Vec::new();
    while !signature.is_empty() {
        let took = whole(signature)?;
        values.push(read_one(&signature[..took], data, at)?);
        signature = &signature[took..];
    }
    Some(values)
}

fn read_one(signature: &[u8], data: &[u8], at: &mut usize) -> Option<Value> {
    let kind = signature[0];
    *at = at.checked_next_multiple_of(boundary(kind))?;
    let mut take = |count: usize| -> Option<&[u8]> {
        let bytes = data.get(*at..at.checked_add(count)?)?;
        *at += count;
        Some(bytes)
    };
    let number = |bytes: &[u8]| u32::from_le_bytes(bytes.try_into().expect("four bytes were taken"));
    Some(match kind {
        b'y' => Value::Byte(take(1)?[0]),
        b'b' => Value::Bool(number(take(4)?) != 0),
        b'i' => Value::I32(number(take(4)?) as i32),
        b'u' | b'h' => Value::U32(number(take(4)?)),
        b'n' | b'q' => Value::Other(u64::from(u16::from_le_bytes(take(2)?.try_into().ok()?))),
        b'x' | b't' | b'd' => Value::Other(u64::from_le_bytes(take(8)?.try_into().ok()?)),
        b's' | b'o' => {
            let length = number(take(4)?) as usize;
            let text = String::from_utf8_lossy(take(length + 1)?.split_last()?.1).into_owned();
            if kind == b's' { Value::Str(text) } else { Value::Path(text) }
        }
        b'g' => {
            let length = usize::from(take(1)?[0]);
            Value::Sig(String::from_utf8_lossy(take(length + 1)?.split_last()?.1).into_owned())
        }
        b'a' => {
            let length = number(take(4)?) as usize;
            let each = &signature[1..];
            *at = at.checked_next_multiple_of(boundary(each[0]))?;
            let end = at.checked_add(length).filter(|end| *end <= data.len())?;
            if each == b"y" {
                let bytes = data[*at..end].to_vec();
                *at = end;
                return Some(Value::Bytes(bytes));
            }
            let mut values = Vec::new();
            while *at < end {
                values.push(read_one(each, data, at)?);
            }
            Value::Array(String::from_utf8_lossy(each).into_owned(), values)
        }
        b'(' => Value::Struct(read(std::str::from_utf8(&signature[1..signature.len() - 1]).ok()?, data, at)?),
        b'{' => {
            let mut parts = read(std::str::from_utf8(&signature[1..signature.len() - 1]).ok()?, data, at)?.into_iter();
            Value::Entry(Box::new(parts.next()?), Box::new(parts.next()?))
        }
        b'v' => {
            let Value::Sig(inner) = read_one(b"g", data, at)? else { return None };
            Value::Variant(Box::new(read_one(inner.as_bytes(), data, at)?))
        }
        _ => return None,
    })
}

/// What a message is: something asked, its answer, a refusal, or
/// something said to whoever is listening.
pub const CALL: u8 = 1;
pub const RETURN: u8 = 2;
pub const ERROR: u8 = 3;
pub const SIGNAL: u8 = 4;

#[derive(Clone, Debug, Default)]
pub struct Message {
    pub kind: u8,
    /// Whether whoever asked wants no answer.
    pub quiet: bool,
    pub serial: u32,
    pub answers: Option<u32>,
    pub path: String,
    pub interface: String,
    pub member: String,
    pub error: String,
    pub destination: String,
    pub sender: String,
    pub body: Vec<Value>,
}

impl Message {
    pub fn call(destination: &str, path: &str, interface: &str, member: &str, body: Vec<Value>) -> Message {
        Message { kind: CALL, destination: destination.into(), path: path.into(), interface: interface.into(), member: member.into(), body, ..Message::default() }
    }

    pub fn signal(path: &str, interface: &str, member: &str, body: Vec<Value>) -> Message {
        Message { kind: SIGNAL, path: path.into(), interface: interface.into(), member: member.into(), body, ..Message::default() }
    }

    /// As bytes, to be sent under a number of its own.
    fn packed(&self, serial: u32) -> Vec<u8> {
        let mut body = Vec::new();
        self.body.iter().for_each(|value| value.write(&mut body));
        let signature: String = self.body.iter().map(Value::signature).collect();
        let field = |code: u8, value: Value| Value::Struct(vec![Value::Byte(code), Value::any(value)]);
        let mut fields = Vec::new();
        for (code, text, path) in [(1, &self.path, true), (2, &self.interface, false), (3, &self.member, false), (4, &self.error, false), (6, &self.destination, false)] {
            if !text.is_empty() {
                fields.push(field(code, if path { Value::Path(text.clone()) } else { Value::Str(text.clone()) }));
            }
        }
        if let Some(answers) = self.answers {
            fields.push(field(5, Value::U32(answers)));
        }
        if !signature.is_empty() {
            fields.push(field(8, Value::Sig(signature)));
        }
        let mut out = vec![b'l', self.kind, u8::from(self.quiet), 1];
        out.extend((body.len() as u32).to_le_bytes());
        out.extend(serial.to_le_bytes());
        Value::Array("(yv)".into(), fields).write(&mut out);
        pad(&mut out, 8);
        out.extend(body);
        out
    }

    /// From bytes: the message, if all of one is there, and how many
    /// bytes it took.
    fn unpacked(data: &[u8]) -> Result<Option<(Message, usize)>> {
        if data.len() < 16 {
            return Ok(None);
        }
        if data[0] != b'l' {
            return Err("the bus speaks with its bytes the other way round, which toast does not".into());
        }
        let number = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().expect("four bytes")) as usize;
        let (body, fields) = (number(4), number(12));
        let begins = (16 + fields).next_multiple_of(8);
        if data.len() < begins + body {
            return Ok(None);
        }
        let mut message = Message { kind: data[1], quiet: data[2] & 1 != 0, serial: number(8) as u32, ..Message::default() };
        let mut at = 12;
        let mut signature = String::new();
        if let Some(Value::Array(_, fields)) = read_one(b"a(yv)", &data[..begins], &mut at) {
            for field in fields {
                let Value::Struct(parts) = field else { continue };
                let [Value::Byte(code), Value::Variant(value)] = &parts[..] else { continue };
                let text = value.as_str().unwrap_or("").to_owned();
                match code {
                    1 => message.path = text,
                    2 => message.interface = text,
                    3 => message.member = text,
                    4 => message.error = text,
                    5 => message.answers = value.as_i32().map(|serial| serial as u32),
                    6 => message.destination = text,
                    7 => message.sender = text,
                    8 => signature = text,
                    _ => {}
                }
            }
        }
        message.body = read(&signature, &data[begins..begins + body], &mut 0).unwrap_or_default();
        Ok(Some((message, begins + body)))
    }
}

/// The writing half of a connection, which another thread can have a
/// copy of: the one that draws says what was clicked through it, as the
/// connection that owns the names, which is who programs listen to.
#[derive(Clone)]
pub struct Writer(Arc<Mutex<(UnixStream, u32)>>);

impl Writer {
    /// Sends a message, and gives the number it went under. A message is
    /// written whole before another can be begun.
    pub fn send(&self, message: &Message) -> Result<u32> {
        let mut held = self.0.lock().map_err(|_| "the bus was dropped mid-message")?;
        held.1 += 1;
        let serial = held.1;
        held.0.write_all(&message.packed(serial))?;
        Ok(serial)
    }
}

/// A connection to the session's bus.
pub struct Bus {
    stream: UnixStream,
    writer: Writer,
    /// What has come and not been made a message of yet; and messages
    /// that came while an answer to something else was waited for.
    unread: Vec<u8>,
    waiting: VecDeque<Message>,
    /// What the bus calls this connection.
    pub name: String,
}

impl Bus {
    /// Connects to the bus the session says is its own, or the one at
    /// the usual place for the user.
    pub fn session() -> Result<Bus> {
        // SAFETY: getuid asks a number of the system and changes nothing.
        let uid = unsafe { libc::getuid() };
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_else(|_| format!("unix:path=/run/user/{uid}/bus"));
        let mut stream = None;
        for way in address.split(';') {
            let Some(rest) = way.strip_prefix("unix:") else { continue };
            for part in rest.split(',') {
                stream = match part.split_once('=') {
                    Some(("path", path)) => UnixStream::connect(path).ok(),
                    Some(("abstract", name)) => {
                        use std::os::linux::net::SocketAddrExt;
                        std::os::unix::net::SocketAddr::from_abstract_name(name).ok().and_then(|address| UnixStream::connect_addr(&address).ok())
                    }
                    _ => None,
                };
                if stream.is_some() {
                    break;
                }
            }
            if stream.is_some() {
                break;
            }
        }
        let mut stream = stream.ok_or("there is no session bus to be found")?;
        // Who this is, said the one way every bus takes: by the number
        // the system knows the user by, in the letters of its digits.
        let who: String = uid.to_string().bytes().map(|digit| format!("{digit:02x}")).collect();
        stream.write_all(format!("\0AUTH EXTERNAL {who}\r\n").as_bytes())?;
        let mut answer = [0u8; 256];
        let got = stream.read(&mut answer)?;
        if !answer[..got].starts_with(b"OK") {
            return Err("the session bus would not have toast".into());
        }
        stream.write_all(b"BEGIN\r\n")?;
        let writer = Writer(Arc::new(Mutex::new((stream.try_clone()?, 0))));
        let mut bus = Bus { stream, writer, unread: Vec::new(), waiting: VecDeque::new(), name: String::new() };
        let hello = bus.ask(Message::call(BUS, BUS_PATH, BUS, "Hello", Vec::new()))?;
        bus.name = hello.body.first().and_then(Value::as_str).unwrap_or("").to_owned();
        Ok(bus)
    }

    /// Sends a message, and gives the number it went under.
    pub fn send(&mut self, message: &Message) -> Result<u32> {
        self.writer.send(message)
    }

    pub fn writer(&self) -> Writer {
        self.writer.clone()
    }

    /// Asks the bus for a name. What it said: 1 that the name is ours, 2
    /// that it will be when whoever has it lets go, 3 that it will not
    /// be, 4 that it was already.
    pub fn request_name(&mut self, name: &str, flags: u32) -> Result<u32> {
        let asked = self.ask(Message::call(BUS, BUS_PATH, BUS, "RequestName", vec![Value::text(name), Value::U32(flags)]))?;
        Ok(asked.body.first().and_then(Value::as_i32).unwrap_or(3) as u32)
    }

    /// Who has a name, by the name the bus calls them.
    pub fn owner(&mut self, name: &str) -> Option<String> {
        let asked = self.ask(Message::call(BUS, BUS_PATH, BUS, "GetNameOwner", vec![Value::text(name)])).ok()?;
        asked.body.first().and_then(Value::as_str).map(str::to_owned)
    }

    /// Hear the signals a rule matches.
    pub fn listen(&mut self, rule: &str) -> Result<()> {
        self.ask(Message::call(BUS, BUS_PATH, BUS, "AddMatch", vec![Value::text(rule)])).map(drop)
    }

    /// Answers something asked: with values, or with a refusal.
    pub fn answer(&mut self, asked: &Message, body: Vec<Value>) -> Result<()> {
        if !asked.quiet {
            self.send(&Message { kind: RETURN, answers: Some(asked.serial), destination: asked.sender.clone(), body, ..Message::default() })?;
        }
        Ok(())
    }

    pub fn refuse(&mut self, asked: &Message, why: &str) -> Result<()> {
        if !asked.quiet {
            self.send(&Message { kind: ERROR, answers: Some(asked.serial), destination: asked.sender.clone(), error: "org.freedesktop.DBus.Error.UnknownMethod".into(), body: vec![Value::text(why)], ..Message::default() })?;
        }
        Ok(())
    }

    /// The next message, waited for no longer than a while; none, if
    /// none came in it.
    pub fn next(&mut self, wait: Option<Duration>) -> Result<Option<Message>> {
        if let Some(message) = self.waiting.pop_front() {
            return Ok(Some(message));
        }
        self.read(wait)
    }

    fn read(&mut self, wait: Option<Duration>) -> Result<Option<Message>> {
        self.stream.set_read_timeout(wait)?;
        loop {
            if let Some((message, took)) = Message::unpacked(&self.unread)? {
                self.unread.drain(..took);
                return Ok(Some(message));
            }
            let mut more = [0u8; 16384];
            match self.stream.read(&mut more) {
                Ok(0) => return Err("the session bus has gone".into()),
                Ok(got) => self.unread.extend_from_slice(&more[..got]),
                Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Ok(None),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Asks something and waits for the answer to it; whatever else
    /// comes meanwhile is kept for later. A refusal is an error.
    pub fn ask(&mut self, message: Message) -> Result<Message> {
        let serial = self.send(&message)?;
        let began = std::time::Instant::now();
        loop {
            let Some(got) = self.read(Some(Duration::from_secs(5)))? else {
                if began.elapsed() > Duration::from_secs(5) {
                    return Err(format!("nothing answered {}", message.member).into());
                }
                continue;
            };
            match (got.answers == Some(serial), got.kind) {
                (true, ERROR) => return Err(format!("{}: {}", got.error, got.body.first().and_then(Value::as_str).unwrap_or("")).into()),
                (true, _) => return Ok(got),
                _ => self.waiting.push_back(got),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_written_where_their_kind_has_to_be_and_read_back_the_same() {
        let values = vec![
            Value::Byte(7),
            Value::text("hotshot"),
            Value::Array("{sv}".into(), vec![Value::Entry(Box::new(Value::text("label")), Box::new(Value::any(Value::text("Select an area"))))]),
            Value::Struct(vec![Value::I32(-3), Value::Bytes(vec![1, 2, 3]), Value::Array("v".into(), vec![Value::any(Value::U32(9))])]),
            Value::Array("(iiay)".into(), Vec::new()),
            Value::Path("/MenuBar".into()),
        ];
        let signature: String = values.iter().map(Value::signature).collect();
        assert_eq!(signature, "ysa{sv}(iayav)a(iiay)o");
        let mut bytes = Vec::new();
        values.iter().for_each(|value| value.write(&mut bytes));
        // A string begins at a multiple of four, after its length.
        assert_eq!(&bytes[..16], &[7, 0, 0, 0, 7, 0, 0, 0, b'h', b'o', b't', b's', b'h', b'o', b't', 0]);
        assert_eq!(read(&signature, &bytes, &mut 0), Some(values));
        // Cut short, or of a kind there is no such thing as: nothing.
        assert_eq!(read(&signature, &bytes[..bytes.len() - 3], &mut 0), None);
        assert_eq!(read("z", &bytes, &mut 0), None);
    }

    #[test]
    fn a_message_is_packed_and_unpacked_and_not_before_all_of_it_has_come() {
        let asked = Message::call("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher", "RegisterStatusNotifierItem", vec![Value::text(":1.42")]);
        let bytes = asked.packed(5);
        assert_eq!((bytes[0], bytes[1], bytes.len() % 4), (b'l', CALL, 2));
        assert!(Message::unpacked(&bytes[..bytes.len() - 1]).unwrap().is_none());
        let (read, took) = Message::unpacked(&bytes).unwrap().unwrap();
        assert_eq!((took, read.serial, read.kind, read.member.as_str(), read.path.as_str(), read.destination.as_str()), (bytes.len(), 5, CALL, "RegisterStatusNotifierItem", "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher"));
        assert_eq!(read.body, vec![Value::text(":1.42")]);
        // An answer says what it answers, and has no path of its own.
        let answer = Message { kind: RETURN, answers: Some(5), body: vec![Value::Bool(true)], ..Message::default() };
        let (read, _) = Message::unpacked(&answer.packed(6)).unwrap().unwrap();
        assert_eq!((read.answers, read.path.as_str(), read.body), (Some(5), "", vec![Value::Bool(true)]));
    }
}
