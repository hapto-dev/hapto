//! What the editors and a plugin's helper share on Linux: the helper's name,
//! and the frames that they send each other over their socket. Each frame is
//! a tag byte, then the payload's length as 4 bytes in little-endian order,
//! then the payload.
//!
//! The editor that starts a helper sends [`Setup`] frames first, [`Start`]
//! and then each of the page's files, and every editor then sends
//! [`Request`]s about its own page. The helper sends [`FromHelper`] frames.
//! Each kind of frame has a tag of its own, so a frame that arrives out of
//! turn reads as an unknown tag.

use std::borrow::Cow;
use std::ffi::c_ulong;
use std::io::{self, ErrorKind, Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};

use hapto_host::Size;
use serde::{Deserialize, Serialize};

/// The helper's program name. The editor looks for the program by this name
/// beside the plugin's module, and the helper puts it before each line that
/// it writes to standard error.
pub(crate) const HELPER: &str = "hapto-webview-helper";

/// The version of these frames. It leads [`Start`]'s frame, the first that
/// the editor sends, and the helper refuses any other version, because a
/// plugin and a helper from different builds may frame their messages
/// differently.
const PROTOCOL: u32 = 1;

/// A message that crosses the socket as one frame.
pub(crate) trait Frame: Sized {
    /// Writes the frame to `to`.
    ///
    /// # Errors
    ///
    /// Returns the socket's error, as when the other side has exited.
    fn write(&self, to: &mut impl Write) -> io::Result<()>;

    /// Returns the frame that `tag` and `payload` make.
    ///
    /// # Errors
    ///
    /// Returns an `InvalidData` error for an unknown tag or a payload that
    /// doesn't parse.
    fn decode(tag: u8, payload: &[u8]) -> io::Result<Self>;

    /// Reads the next frame from `from`, or returns `None` once the other
    /// side has shut the socket.
    ///
    /// # Errors
    ///
    /// Returns the socket's error, or [`decode`](Self::decode)'s.
    fn read(from: &mut impl Read) -> io::Result<Option<Self>> {
        read_frame(from)?
            .map(|(tag, payload)| Self::decode(tag, &payload))
            .transpose()
    }
}

/// What the editor sends a helper to start it.
#[derive(Debug, PartialEq)]
pub(crate) enum Setup<'a> {
    /// What every page of the plugin shares, sent first.
    Start(Start),
    /// One of the page's files, which the helper serves at the page's scheme.
    File {
        path: Cow<'a, str>,
        bytes: Cow<'a, [u8]>,
    },
}

/// What every page of the plugin shares.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Start {
    /// How many of the page's files follow.
    pub(crate) files: usize,
    /// The engine's data folder, which exists, as the bytes of its path,
    /// because a path on Linux need not be UTF-8.
    pub(crate) folder: Vec<u8>,
    pub(crate) dev: Option<String>,
    pub(crate) devtools: bool,
}

/// One of the pages that a helper shows, one for each open editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PageId(pub(crate) u32);

/// What an editor asks of the helper about its page.
#[derive(Debug, PartialEq)]
pub(crate) enum Request<'a> {
    /// Opens the page in the host's window. The helper answers
    /// [`FromHelper::Opened`] or [`FromHelper::Failed`].
    Open(PageId, Open),
    /// Runs a script in the page.
    Script(PageId, Cow<'a, str>),
    /// Sizes the page.
    Place(PageId, Place),
    /// Gives the keyboard back to the host's window.
    FocusHost(PageId),
    /// Takes the page out of the host's window. The helper answers
    /// [`FromHelper::Closed`] once the page is out, and the editor's close
    /// waits for that, because the host may destroy its window as soon as
    /// the close returns.
    Close(PageId),
}

impl Request<'_> {
    /// Returns the page that the request is about.
    pub(crate) fn page(&self) -> PageId {
        match self {
            Request::Open(page, _)
            | Request::Script(page, _)
            | Request::Place(page, _)
            | Request::FocusHost(page)
            | Request::Close(page) => *page,
        }
    }
}

/// Where the helper opens a page.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Open {
    /// The host's X11 window, which the helper shows the page in.
    pub(crate) window: c_ulong,
    pub(crate) place: Place,
}

/// The page's size in logical pixels, at the host's scale.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Place {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale: f64,
}

impl Place {
    pub(crate) fn new(size: Size, scale: f64) -> Place {
        Place {
            width: size.width,
            height: size.height,
            scale,
        }
    }
}

/// What the helper sends the editors.
#[derive(Debug, PartialEq)]
pub(crate) enum FromHelper {
    /// The page is in the host's window.
    Opened(PageId),
    /// The page did not open, and why.
    Failed(PageId, String),
    /// The page is out of the host's window.
    Closed(PageId),
    /// A message that the page posted.
    Message(PageId, String),
    /// The helper couldn't start, and why. It exits after this.
    Stopped(String),
}

const START: u8 = b'i';
const FILE: u8 = b'f';
const OPEN: u8 = b'o';
const SCRIPT: u8 = b's';
const PLACE: u8 = b'p';
const FOCUS_HOST: u8 = b'k';
const CLOSE: u8 = b'c';
const OPENED: u8 = b'O';
const FAILED: u8 = b'F';
const CLOSED: u8 = b'C';
const MESSAGE: u8 = b'M';
const STOPPED: u8 = b'X';

impl Frame for Setup<'_> {
    fn write(&self, to: &mut impl Write) -> io::Result<()> {
        match self {
            Setup::Start(start) => write_frame(
                to,
                START,
                &[&PROTOCOL.to_le_bytes(), &serde_json::to_vec(start)?],
            ),
            Setup::File { path, bytes } => {
                let length = u32::try_from(path.len()).map_err(io::Error::other)?;
                write_frame(to, FILE, &[&length.to_le_bytes(), path.as_bytes(), bytes])
            }
        }
    }

    fn decode(tag: u8, payload: &[u8]) -> io::Result<Self> {
        match tag {
            START => {
                let (protocol, json) = split_u32(payload)?;
                if protocol != PROTOCOL {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        format!(
                            "the plugin's frames are version {protocol} and this helper's \
                             are version {PROTOCOL}, so the two come from different builds"
                        ),
                    ));
                }
                Ok(Setup::Start(serde_json::from_slice(json)?))
            }
            FILE => {
                let (length, rest) = split_u32(payload)?;
                let length = usize::try_from(length).map_err(io::Error::other)?;
                let (path, bytes) = rest.split_at_checked(length).ok_or_else(truncated)?;
                Ok(Setup::File {
                    path: Cow::Owned(text(path)?),
                    bytes: Cow::Owned(bytes.to_vec()),
                })
            }
            other => Err(unknown(other)),
        }
    }
}

impl Frame for Request<'_> {
    fn write(&self, to: &mut impl Write) -> io::Result<()> {
        let page = self.page().0.to_le_bytes();
        match self {
            Request::Open(_, open) => write_frame(to, OPEN, &[&page, &serde_json::to_vec(open)?]),
            Request::Script(_, script) => write_frame(to, SCRIPT, &[&page, script.as_bytes()]),
            Request::Place(_, place) => {
                write_frame(to, PLACE, &[&page, &serde_json::to_vec(place)?])
            }
            Request::FocusHost(_) => write_frame(to, FOCUS_HOST, &[&page]),
            Request::Close(_) => write_frame(to, CLOSE, &[&page]),
        }
    }

    fn decode(tag: u8, payload: &[u8]) -> io::Result<Self> {
        let (page, rest) = split_page(payload)?;
        match tag {
            OPEN => Ok(Request::Open(page, serde_json::from_slice(rest)?)),
            SCRIPT => Ok(Request::Script(page, Cow::Owned(text(rest)?))),
            PLACE => Ok(Request::Place(page, serde_json::from_slice(rest)?)),
            FOCUS_HOST => Ok(Request::FocusHost(page)),
            CLOSE => Ok(Request::Close(page)),
            other => Err(unknown(other)),
        }
    }
}

impl Frame for FromHelper {
    fn write(&self, to: &mut impl Write) -> io::Result<()> {
        match self {
            FromHelper::Opened(page) => write_frame(to, OPENED, &[&page.0.to_le_bytes()]),
            FromHelper::Failed(page, reason) => {
                write_frame(to, FAILED, &[&page.0.to_le_bytes(), reason.as_bytes()])
            }
            FromHelper::Closed(page) => write_frame(to, CLOSED, &[&page.0.to_le_bytes()]),
            FromHelper::Message(page, message) => {
                write_frame(to, MESSAGE, &[&page.0.to_le_bytes(), message.as_bytes()])
            }
            FromHelper::Stopped(reason) => write_frame(to, STOPPED, &[reason.as_bytes()]),
        }
    }

    fn decode(tag: u8, payload: &[u8]) -> io::Result<Self> {
        if tag == STOPPED {
            return Ok(FromHelper::Stopped(text(payload)?));
        }
        let (page, rest) = split_page(payload)?;
        match tag {
            OPENED => Ok(FromHelper::Opened(page)),
            FAILED => Ok(FromHelper::Failed(page, text(rest)?)),
            CLOSED => Ok(FromHelper::Closed(page)),
            MESSAGE => Ok(FromHelper::Message(page, text(rest)?)),
            other => Err(unknown(other)),
        }
    }
}

/// Starts a thread named `name` that reads frames from `from`, sends each to
/// the receiver that this returns, and calls `wake` after each. The thread
/// ends when the socket ends or a read fails, and returns the read's error.
/// The receiver then reports that it is disconnected.
///
/// # Errors
///
/// Returns why the thread didn't start.
pub(crate) fn read_on_a_thread<F: Frame + Send + 'static>(
    name: &str,
    mut from: impl Read + Send + 'static,
    wake: impl Fn() + Send + 'static,
) -> io::Result<(Receiver<F>, JoinHandle<io::Result<()>>)> {
    let (sender, received) = mpsc::channel();
    let reader = thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let read = loop {
                match F::read(&mut from) {
                    Ok(Some(frame)) => {
                        if sender.send(frame).is_err() {
                            break Ok(());
                        }
                        wake();
                    }
                    Ok(None) => break Ok(()),
                    Err(error) => break Err(error),
                }
            };
            drop(sender);
            wake();
            read
        })?;
    Ok((received, reader))
}

/// Writes one frame, whose payload is `parts` one after another, and flushes
/// it. Each part goes through `write_all`, as the editor's socket needs (see
/// `Connection` on Linux).
fn write_frame(to: &mut impl Write, tag: u8, parts: &[&[u8]]) -> io::Result<()> {
    let length = parts.iter().map(|part| part.len()).sum::<usize>();
    let length = u32::try_from(length).map_err(io::Error::other)?;
    to.write_all(&[tag])?;
    to.write_all(&length.to_le_bytes())?;
    for part in parts {
        to.write_all(part)?;
    }
    to.flush()
}

/// Reads one frame's tag and payload, or returns `None` when the socket ends
/// before a frame starts.
fn read_frame(from: &mut impl Read) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut tag = [0];
    // A signal can interrupt the wait for a frame. `read_exact` retries the
    // reads below by itself.
    loop {
        match from.read(&mut tag) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let mut length = [0; 4];
    from.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length);
    // The payload grows only as its bytes arrive, so a wrong length can't
    // make this side allocate memory for bytes that never come.
    let mut payload = Vec::new();
    from.by_ref()
        .take(u64::from(length))
        .read_to_end(&mut payload)?;
    if u32::try_from(payload.len()).ok() != Some(length) {
        return Err(truncated());
    }
    Ok(Some((tag[0], payload)))
}

/// Splits the page's id at the start of `payload` from the rest.
fn split_page(payload: &[u8]) -> io::Result<(PageId, &[u8])> {
    let (page, rest) = split_u32(payload)?;
    Ok((PageId(page), rest))
}

/// Splits the 4-byte little-endian number at the start of `payload` from the
/// rest.
fn split_u32(payload: &[u8]) -> io::Result<(u32, &[u8])> {
    let (number, rest) = payload.split_first_chunk::<4>().ok_or_else(truncated)?;
    Ok((u32::from_le_bytes(*number), rest))
}

fn text(bytes: &[u8]) -> io::Result<String> {
    String::from_utf8(bytes.to_vec()).map_err(|error| io::Error::new(ErrorKind::InvalidData, error))
}

fn truncated() -> io::Error {
    io::Error::new(ErrorKind::InvalidData, "a frame shorter than its fields")
}

fn unknown(tag: u8) -> io::Error {
    io::Error::new(
        ErrorKind::InvalidData,
        format!("a frame of unknown kind {tag}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes each of `sent` to one buffer and checks that it reads back as
    /// it was written, then that the buffer ends.
    fn round_trip<F: Frame + PartialEq + std::fmt::Debug>(sent: &[F]) {
        let mut socket = Vec::new();
        for frame in sent {
            frame.write(&mut socket).expect("a write to memory");
        }
        let mut socket = socket.as_slice();
        for frame in sent {
            assert_eq!(F::read(&mut socket).expect("a frame").as_ref(), Some(frame));
        }
        assert_eq!(F::read(&mut socket).expect("the end"), None);
    }

    #[test]
    fn every_frame_reads_back_as_it_was_written() {
        let size = Size {
            width: 400,
            height: 300,
        };
        let place = Place::new(size, 1.5);
        let page = PageId(7);
        round_trip(&[
            Setup::Start(Start {
                files: 1,
                folder: b"/home/a/.local/share/dev.hapto.gain/WebKitGTK\xff".to_vec(),
                dev: None,
                devtools: true,
            }),
            Setup::File {
                path: Cow::Borrowed("assets/index.js"),
                bytes: Cow::Borrowed(b"console.log(1)"),
            },
        ]);
        round_trip(&[
            Request::Open(
                page,
                Open {
                    window: 0x0040_0005,
                    place,
                },
            ),
            Request::Script(page, Cow::Borrowed("window.__hapto.receive('{}')")),
            Request::Place(page, place),
            Request::FocusHost(page),
            Request::Close(page),
        ]);
        round_trip(&[
            FromHelper::Opened(page),
            FromHelper::Failed(page, String::from("the host's window is gone")),
            FromHelper::Message(page, String::from(r#"{"type":"ready","protocol":3}"#)),
            FromHelper::Closed(page),
            FromHelper::Stopped(String::from("GTK did not start")),
        ]);
    }

    #[test]
    fn the_reader_passes_on_each_frame_wakes_after_each_and_then_disconnects() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let sent = [
            FromHelper::Opened(PageId(1)),
            FromHelper::Message(PageId(1), String::from("{}")),
        ];
        let mut socket = Vec::new();
        for frame in &sent {
            frame.write(&mut socket).expect("a write to memory");
        }
        let wakes = Arc::new(AtomicUsize::new(0));
        let woken = Arc::clone(&wakes);
        let (received, reader) =
            read_on_a_thread::<FromHelper>(HELPER, io::Cursor::new(socket), move || {
                woken.fetch_add(1, Ordering::Relaxed);
            })
            .expect("a thread");
        assert_eq!(received.iter().collect::<Vec<_>>(), sent);
        let read = reader
            .join()
            .expect("the reader ends at the end of the socket");
        assert!(read.is_ok());
        assert_eq!(
            wakes.load(Ordering::Relaxed),
            3,
            "a wake for each frame and the end"
        );
    }

    #[test]
    fn a_frame_cut_short_out_of_turn_or_of_another_version_is_an_error() {
        let mut socket = Vec::new();
        FromHelper::Message(PageId(1), String::from("cut"))
            .write(&mut socket)
            .expect("a write");
        let mut cut = &socket[..socket.len() - 1];
        assert!(FromHelper::read(&mut cut).is_err());
        let mut request = Vec::new();
        Request::FocusHost(PageId(1))
            .write(&mut request)
            .expect("a write");
        assert!(Setup::read(&mut request.as_slice()).is_err());
        let mut later = vec![START, 6, 0, 0, 0];
        later.extend_from_slice(&(PROTOCOL + 1).to_le_bytes());
        later.extend_from_slice(b"{}");
        let refused = Setup::read(&mut later.as_slice()).expect_err("another version");
        assert!(
            refused.to_string().contains("different builds"),
            "{refused}"
        );
    }

    #[test]
    fn a_length_longer_than_what_arrives_is_an_error() {
        let frame = [MESSAGE, 0xff, 0xff, 0xff, 0xff, 1, 0, 0, 0, b'a'];
        assert!(FromHelper::read(&mut frame.as_slice()).is_err());
    }

    #[test]
    fn a_signal_while_a_frame_is_awaited_does_not_end_the_read() {
        /// Fails its first read as a signal interrupts it, then reads on.
        struct Interrupted<'a>(bool, &'a [u8]);
        impl Read for Interrupted<'_> {
            fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
                if std::mem::replace(&mut self.0, false) {
                    return Err(ErrorKind::Interrupted.into());
                }
                self.1.read(into)
            }
        }
        let mut socket = Vec::new();
        FromHelper::Closed(PageId(2))
            .write(&mut socket)
            .expect("a write");
        let mut from = Interrupted(true, &socket);
        assert_eq!(
            FromHelper::read(&mut from).expect("a frame"),
            Some(FromHelper::Closed(PageId(2)))
        );
    }
}
